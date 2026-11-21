//! The frame loop: for every output frame it replays the board and the quiz to the frame's time,
//! chooses the stage (the teacher's screen, a game or the board), draws it with the strip and the
//! camera tile, and writes the frame to the encoder. A port of the loop of `export/exporter.ts`.
//!
//! A render is cut into pieces that begin on keyframes (a multiple of 60 frames); each piece is
//! drawn by its own thread, because Skia's fonts are not `Send`, and encoded by its own x264. A
//! piece enters the board and the game at its first frame, and each video file at the keyframe
//! before the frame its grid starts on. Everything a piece waits for is a pipe (a decoder's
//! frames, the encoder's input) or a file the prefetcher is still downloading, and a stop kills
//! the children so that no wait outlasts it.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use board_draw::{BoardPainter, SkiaCanvas, StaticPictures, decode_image};
use replay_model::manifest::{FileKind, RecordingFile};
use replay_model::timeline::{StageKind, camera_at, choose_stage, screen_layers_at, shown_screen};
use replay_model::{BoardRecord, BoardReplay};
use skia_safe::Rect;
use stage_draw::card::countdown_seconds;
use stage_draw::clock::{
    FPS, HEIGHT, KEYFRAME_EVERY, WIDTH, frame_time_ms, preroll_frames, total_frames,
};
use stage_draw::layout::TILE_H;
use stage_draw::{CardText, QuizStageDraw, QuizStageFactory, StagePainter};

use super::decode::{Decoder, Start, TOLERANCE_S, file_time_s, grid_origin};
use super::encode::{Profile, encode_command};
use super::ffmpeg::Registry;
use super::fonts::FontFiles;
use super::prefetch::{Budget, Library, ReadyFile};
use super::reader::ReadError;

#[derive(Debug, thiserror::Error)]
pub enum ComposeError {
    /// The render was stopped.
    #[error("cancelled")]
    Cancelled,
    /// A recording file could not be had.
    #[error("{0}")]
    Unreadable(ReadError),
    #[error("{0}")]
    Failed(String),
}

fn failed(what: impl Into<String>) -> ComposeError {
    ComposeError::Failed(what.into())
}

/// An encoded deck page: its deck, its number and its bytes.
pub type DeckPicture = ((String, u32), Vec<u8>);

/// What every piece of the video is drawn from.
pub struct ComposeInput {
    pub ffmpeg: PathBuf,
    pub dir: PathBuf,
    pub fonts: Arc<FontFiles>,
    /// The teacher's streams.
    pub teacher: Vec<RecordingFile>,
    pub records: Arc<Vec<BoardRecord>>,
    /// The deck page pictures, encoded, by deck and page.
    pub pictures: Arc<Vec<DeckPicture>>,
    pub card: Option<CardText>,
    pub preroll_ms: u64,
    pub duration_ms: f64,
    /// Builds the game's stage on the thread of each piece.
    pub quiz: Option<QuizStageFactory>,
    pub profile: Profile,
    /// Video files as the prefetcher finishes them.
    pub library: Arc<Library>,
    pub budget: Arc<Budget>,
    pub registry: Registry,
    pub cancel: Arc<AtomicBool>,
    /// How far the render is, in ms of the video, summed over the pieces.
    pub progress: Arc<AtomicI64>,
}

/// One piece of the video: output frames `from..to` (`from` a multiple of the keyframe interval),
/// encoded to `out`.
#[derive(Debug, Clone)]
pub struct Piece {
    pub index: usize,
    pub from: i64,
    pub to: i64,
    pub out: PathBuf,
    /// x264's threads, when several pieces share the cores.
    pub threads: Option<u32>,
}

/// How much more a lesson frame costs than a frame of the countdown card (which is one picture a
/// second, and cheap to draw and to encode).
const LESSON_FRAME_WEIGHT: i64 = 4;

/// Splits `frames` output frames, the first `card_frames` of them the card, into at most `n`
/// pieces of about the same cost, each beginning on a keyframe.
pub fn plan_pieces(frames: i64, card_frames: i64, n: usize) -> Vec<(i64, i64)> {
    let block = i64::from(KEYFRAME_EVERY);
    let blocks = (frames + block - 1) / block;
    let n = (n as i64).clamp(1, blocks.max(1));
    let cost = |upto: i64| {
        let frames = (upto * block).min(frames);
        let card = frames.min(card_frames);
        card + (frames - card) * LESSON_FRAME_WEIGHT
    };
    let total = cost(blocks);
    let mut cuts = vec![0];
    for k in 1..n {
        let want = total * k / n;
        let last = *cuts.last().unwrap_or(&0);
        let floor = last + 1;
        let ceil = blocks - (n - k);
        let at = (floor..=ceil)
            .find(|b| cost(*b) >= want)
            .unwrap_or(ceil)
            .max(floor);
        cuts.push(at);
    }
    cuts.push(blocks);
    cuts.windows(2)
        .map(|w| (w[0] * block, (w[1] * block).min(frames)))
        .collect()
}

/// Whether the piece of lesson frames `from..to` opens the decoder of `file`.
pub fn needs(file: &RecordingFile, from: i64, to: i64) -> bool {
    let (j0, _) = grid_origin(file.start_ms, FPS);
    let first = from.max(0).max(j0 - 2 * i64::from(FPS));
    first < to && frame_time_ms(first) < file.end_ms
}

impl std::fmt::Debug for ComposeInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposeInput")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// One video file of the teacher: a screen or the camera.
struct Track {
    file: RecordingFile,
    size: (u32, u32),
    /// The lesson frame the decoder's first slot is, and where in the file that time is.
    j0: i64,
    origin_s: f64,
    ready: Option<Arc<ReadyFile>>,
    decoder: Option<Decoder>,
    closed: bool,
}

impl Track {
    /// A picture is there to show at lesson frame `j` (time `t`).
    fn has_picture(&self, t: f64) -> bool {
        let Some(ready) = &self.ready else {
            return false;
        };
        self.decoder.as_ref().is_some_and(|d| d.frame().is_some())
            && (t - self.file.start_ms) / 1000.0 + TOLERANCE_S >= ready.first_picture_s
    }

    fn picture(&self) -> Option<&[u8]> {
        self.decoder.as_ref().and_then(Decoder::frame)
    }
}

/// What a frame is made of, to tell when it is the one already on the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKey {
    Card(u64),
    Board {
        revision: u64,
        camera: Option<(usize, u64)>,
    },
    Screen {
        screen: (usize, u64),
        camera: Option<(usize, u64)>,
    },
}

/// Renders `piece` of the video into `piece.out` (video only; the soundtrack is joined later).
pub fn compose(input: &ComposeInput, piece: &Piece) -> Result<(), ComposeError> {
    let result = compose_piece(input, piece);
    input.budget.finish(piece.index);
    result
}

fn compose_piece(input: &ComposeInput, piece: &Piece) -> Result<(), ComposeError> {
    let cancel = input.cancel.clone();
    let fonts = input.fonts.build().map_err(|e| failed(e.to_string()))?;
    let mut quiz: Option<Box<dyn QuizStageDraw>> =
        input.quiz.as_ref().map(|make| make(fonts.clone()));
    let teacher = input.teacher.clone();
    let has_camera = replay_model::timeline::has_camera(&teacher);
    let painter = StagePainter::new(fonts.clone(), has_camera, input.card.as_ref())
        .map_err(|e| failed(e.to_string()))?;
    let layout = *painter.layout();
    let stage = layout.stage;
    let stage_size = (stage.w as u32, stage.h as u32);
    let mut frame = SkiaCanvas::new(WIDTH, HEIGHT, fonts).map_err(|e| failed(e.to_string()))?;
    let mut board_painter = BoardPainter::new(&frame, stage_size.0, stage_size.1);
    let mut board = BoardReplay::new(input.records.as_ref().clone());
    let mut pictures = StaticPictures::new();
    for ((deck, n), bytes) in input.pictures.iter() {
        if let Some(image) = decode_image(bytes) {
            pictures.insert(deck, *n, image);
        }
    }

    let card_frames = preroll_frames(input.preroll_ms) as i64;
    let frames = total_frames(input.duration_ms, input.preroll_ms) as i64;
    let (lesson_from, lesson_to) = (piece.from - card_frames, piece.to - card_frames);
    let mut tracks: Vec<Track> = teacher
        .iter()
        .filter(|f| matches!(f.kind, FileKind::Screen | FileKind::Camera))
        .filter(|f| f.end_ms > f.start_ms && f.end_ms > 0.0 && f.start_ms < input.duration_ms)
        .filter(|f| needs(f, lesson_from, lesson_to))
        .map(|f| {
            let (j0, origin_s) = grid_origin(f.start_ms, FPS);
            let size = if f.kind == FileKind::Camera {
                (layout.tile.map_or(144, |t| t.w as u32), TILE_H as u32)
            } else {
                stage_size
            };
            Track {
                file: f.clone(),
                size,
                j0,
                origin_s,
                ready: None,
                decoder: None,
                closed: false,
            }
        })
        .collect();
    let by_index: HashMap<u32, usize> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.file.index, i))
        .collect();
    let mut encoder = input
        .registry
        .spawn(
            &mut encode_command(
                &input.ffmpeg,
                input.profile,
                (WIDTH, HEIGHT),
                FPS,
                KEYFRAME_EVERY,
                piece.threads,
                &piece.out,
            ),
            &input.dir.join(format!("encode-{}.log", piece.index)),
        )
        .map_err(|_| failed("cannot start the encoder"))?;
    let mut sink = encoder
        .child
        .stdin
        .take()
        .ok_or_else(|| failed("no encoder input"))?;
    drop(encoder.child.stdout.take());
    let mut held: Option<u32> = None;
    // What the surface shows now: a frame whose key is the same is not drawn again.
    let mut last_key: Option<FrameKey> = None;
    let (mut quiz_on, mut quiz_since) = (false, 0.0);
    let stage_rect = Rect::from_xywh(
        stage.x as f32,
        stage.y as f32,
        stage.w as f32,
        stage.h as f32,
    );
    let mut log_number = 0u32;
    let mut done = 0u32;
    // A piece that begins inside the lesson finds the game as it stood: which stage took the
    // screen depends on when the game began.
    if let Some(quiz) = quiz.as_mut() {
        for lesson_frame in 0..lesson_from {
            let t = frame_time_ms(lesson_frame);
            let on = quiz.on(t);
            if on && !quiz_on {
                quiz_since = t;
            }
            quiz_on = on;
        }
    }

    // Where the time goes: waiting for decoders, drawing, writing to the encoder.
    let (mut t_decode, mut t_draw, mut t_write) = (
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
    );
    let mut mark = std::time::Instant::now();
    for i in piece.from..piece.to.min(frames) {
        if cancel.load(Ordering::Relaxed) {
            return Err(ComposeError::Cancelled);
        }
        let lesson_frame = i - card_frames;
        if lesson_frame < 0 {
            let key = Some(FrameKey::Card(countdown_seconds(
                i as u64,
                card_frames as u64,
            )));
            if key != last_key {
                painter.paint_card(frame.sk_canvas(), i as u64, card_frames as u64);
                last_key = key;
            }
        } else {
            let t = frame_time_ms(lesson_frame);
            input.budget.set_now(piece.index, t);
            // Open, advance and close the video files.
            for track in tracks.iter_mut().filter(|t| !t.closed) {
                if t >= track.file.end_ms {
                    close(track, &input.library);
                    continue;
                }
                if track.decoder.is_none() && lesson_frame >= track.j0 - 2 * i64::from(FPS) {
                    let ready = input.library.take(track.file.index, &cancel)?;
                    log_number += 1;
                    // A piece that begins inside the file enters it at the first slot it draws.
                    let first_slot = lesson_frame.max(track.j0);
                    let seek = first_slot > track.j0;
                    let origin_s = if seek {
                        file_time_s(first_slot, track.file.start_ms, FPS)
                    } else {
                        track.origin_s
                    };
                    let decoder = Decoder::spawn(
                        &input.registry,
                        &input.ffmpeg,
                        &ready.path,
                        track.size,
                        Start { origin_s, seek },
                        FPS,
                        &input
                            .dir
                            .join(format!("decode-{}-{log_number}.log", piece.index)),
                    )
                    .map_err(|_| failed("cannot start a decoder"))?;
                    track.ready = Some(ready);
                    track.decoder = Some(decoder);
                }
                if lesson_frame >= track.j0
                    && let Some(decoder) = track.decoder.as_mut()
                {
                    decoder.advance();
                }
            }

            t_decode += mark.elapsed();
            mark = std::time::Instant::now();
            board.seek(t);
            if let Some(quiz) = quiz.as_mut() {
                let on = quiz.on(t);
                if on && !quiz_on {
                    quiz_since = t;
                }
                quiz_on = on;
            }
            let covering = screen_layers_at(&teacher, t, 0.0);
            let latest_start = shown_screen(&covering, t, |_| true, None)
                .and_then(|idx| covering.iter().find(|f| f.index == idx))
                .map(|f| f.start_ms);
            let mut kind = choose_stage(latest_start, quiz_on, quiz_since);
            let mut shown: Option<usize> = None;
            if kind == StageKind::Screen {
                let ready = |index: u32| {
                    by_index
                        .get(&index)
                        .is_some_and(|p| tracks[*p].has_picture(t))
                };
                held = shown_screen(&covering, t, ready, held);
                shown = held.and_then(|idx| by_index.get(&idx).copied());
                if shown.is_none() {
                    kind = if quiz_on {
                        StageKind::Quiz
                    } else {
                        StageKind::Board
                    };
                }
            }
            let camera_track = if layout.tile.is_some() {
                camera_at(&teacher, t)
                    .and_then(|c| by_index.get(&c.index).copied())
                    .filter(|p| tracks[*p].has_picture(t))
            } else {
                None
            };
            // The picture is the same as the last one when everything it is made of is: the same
            // frame of the same screen, the same board revision (no pointer preview moving), the
            // same camera frame. A game is always drawn.
            let read = |p: usize| tracks[p].decoder.as_ref().map_or(0, |d| d.read);
            let key = match kind {
                StageKind::Quiz => None,
                StageKind::Board if board.overlays().is_empty() => Some(FrameKey::Board {
                    revision: board.scene().revision(),
                    camera: camera_track.map(|p| (p, read(p))),
                }),
                StageKind::Board => None,
                StageKind::Screen => shown.map(|p| FrameKey::Screen {
                    screen: (p, read(p)),
                    camera: camera_track.map(|p| (p, read(p))),
                }),
            };
            if key.is_none() || key != last_key {
                match kind {
                    StageKind::Screen => painter
                        .paint_screen(frame.sk_canvas(), shown.and_then(|p| tracks[p].picture())),
                    StageKind::Board => {
                        board_painter.paint(&mut frame, (0.0, 0.0), &board, &pictures);
                    }
                    StageKind::Quiz => {
                        if let Some(quiz) = quiz.as_mut() {
                            quiz.draw(frame.sk_canvas(), stage_rect, t);
                        }
                    }
                }
                painter.paint_strip(frame.sk_canvas());
                if let Some(p) = camera_track
                    && let Some(picture) = tracks[p].picture()
                {
                    painter.paint_camera(frame.sk_canvas(), picture);
                }
            }
            last_key = key;
        }
        t_draw += mark.elapsed();
        mark = std::time::Instant::now();
        // The surface's own pixels go to the encoder: N32 is BGRA, which is what ffmpeg is told.
        let written = frame
            .with_pixels(|pixels| sink.write_all(pixels))
            .ok_or_else(|| failed("cannot read the frame"))?;
        written.map_err(|_| {
            if cancel.load(Ordering::Relaxed) {
                ComposeError::Cancelled
            } else {
                failed(format!("the encoder stopped: {}", encoder.stderr_tail()))
            }
        })?;
        t_write += mark.elapsed();
        mark = std::time::Instant::now();
        done += 1;
        if done == FPS {
            input.progress.fetch_add(1000, Ordering::Relaxed);
            done = 0;
        }
    }
    tracing::info!(
        piece = piece.index,
        ?t_decode,
        ?t_draw,
        ?t_write,
        frames = piece.to - piece.from,
        "compose time split"
    );
    for track in tracks.iter_mut().filter(|t| !t.closed) {
        close(track, &input.library);
    }
    drop(sink);
    if encoder.wait_ok() {
        Ok(())
    } else if cancel.load(Ordering::Relaxed) {
        Err(ComposeError::Cancelled)
    } else {
        Err(failed(format!(
            "the encoder failed: {}",
            encoder.stderr_tail()
        )))
    }
}

/// Stops a track's decoder and lets its file go.
fn close(track: &mut Track, library: &Library) {
    track.closed = true;
    track.decoder = None;
    track.ready = None;
    library.release(track.file.index);
}

#[cfg(test)]
mod tests {
    use super::*;
    use replay_model::manifest::ManifestFile;

    fn file(kind: FileKind, start_ms: f64, end_ms: f64) -> RecordingFile {
        RecordingFile {
            index: 0,
            file: ManifestFile {
                kind,
                session_id: None,
                user_id: None,
                start_ms,
                end_ms,
                width: None,
                height: None,
                size: 1,
            },
        }
    }

    #[test]
    fn pieces_cover_every_frame_once_and_begin_on_keyframes() {
        for (frames, card, n) in [
            (2400, 1800, 4),
            (36_000, 1800, 4),
            (301, 0, 3),
            (59, 0, 4),
            (60, 0, 1),
            (1, 0, 8),
            (18_000, 1800, 32),
        ] {
            let pieces = plan_pieces(frames, card, n);
            assert!(!pieces.is_empty() && pieces.len() <= n, "{frames} {n}");
            assert_eq!(pieces[0].0, 0);
            assert_eq!(pieces.last().map(|p| p.1), Some(frames));
            for w in pieces.windows(2) {
                assert_eq!(w[0].1, w[1].0, "pieces must meet");
            }
            for (from, to) in &pieces {
                assert_eq!(from % i64::from(KEYFRAME_EVERY), 0, "{pieces:?}");
                assert!(to > from);
            }
        }
    }

    #[test]
    fn the_card_is_cheaper_than_the_lesson_when_pieces_are_balanced() {
        // 60 s of card, 20 s of lesson, 2 pieces: the cut falls in the lesson, not at its middle in
        // frames.
        let pieces = plan_pieces(2400, 1800, 2);
        assert_eq!(pieces.len(), 2);
        assert!(pieces[0].1 > 1800, "{pieces:?}");
        // A lesson with no card splits evenly.
        assert_eq!(plan_pieces(1200, 0, 2), vec![(0, 600), (600, 1200)]);
    }

    #[test]
    fn a_piece_needs_the_files_it_will_open() {
        let f = file(FileKind::Screen, 10_000.0, 20_000.0);
        // The decoder opens two seconds before the file's first frame (frame 300): from 240.
        assert!(!needs(&f, 0, 240));
        assert!(needs(&f, 0, 241));
        assert!(needs(&f, 300, 360));
        // The file ends at frame 600.
        assert!(needs(&f, 599, 660));
        assert!(!needs(&f, 600, 660));
        assert!(!needs(&f, 900, 1200));
        // The card's frames (negative lesson frames) open nothing.
        assert!(!needs(&f, -1800, -1));
        // A file from the very start.
        let g = file(FileKind::Camera, 0.0, 5_000.0);
        assert!(needs(&g, -60, 1));
    }
}
