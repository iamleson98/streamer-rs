//! The frame loop: for every output frame it replays the board and the quiz to the frame's time,
//! chooses the stage (the teacher's screen, a game or the board), draws it with the strip and the
//! camera tile, and writes the frame to the encoder. A port of the loop of `export/exporter.ts`.
//!
//! It runs on one thread, because Skia's fonts are not `Send`. Everything it waits for is a pipe
//! (a decoder's frames, the encoder's input) or a file the prefetcher is still downloading, and a
//! stop kills the children so that no wait outlasts it.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::Receiver;

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
use stage_draw::{CardText, QuizStageDraw, StagePainter};

use super::decode::{Decoder, TOLERANCE_S, grid_origin};
use super::encode::{Profile, encode_command};
use super::ffmpeg::Registry;
use super::fonts::FontFiles;
use super::prefetch::{Budget, ReadyFile};
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

/// Everything the loop is given.
pub struct ComposeInput {
    pub ffmpeg: PathBuf,
    pub dir: PathBuf,
    /// The video file the frames are encoded to.
    pub video_out: PathBuf,
    pub fonts: Arc<FontFiles>,
    /// The teacher's streams.
    pub teacher: Vec<RecordingFile>,
    pub records: Vec<BoardRecord>,
    /// The deck page pictures, encoded, by deck and page.
    pub pictures: Vec<((String, u32), Vec<u8>)>,
    pub card: Option<CardText>,
    pub preroll_ms: u64,
    pub duration_ms: f64,
    pub quiz: Option<Box<dyn QuizStageDraw>>,
    pub profile: Profile,
    /// Video files as the prefetcher finishes them.
    pub files: Receiver<Result<ReadyFile, ReadError>>,
    pub budget: Arc<Budget>,
    pub registry: Registry,
    pub cancel: Arc<AtomicBool>,
    /// How far the file is, in ms of the video.
    pub progress: Arc<AtomicI64>,
}

impl std::fmt::Debug for ComposeInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposeInput")
            .field("dir", &self.dir)
            .finish()
    }
}

/// One video file of the teacher: a screen or the camera.
struct Track {
    file: RecordingFile,
    size: (u32, u32),
    /// The lesson frame the decoder's first slot is, and where in the file that time is.
    j0: i64,
    origin_s: f64,
    ready: Option<ReadyFile>,
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

struct Waiting {
    files: Receiver<Result<ReadyFile, ReadError>>,
    arrived: HashMap<u32, ReadyFile>,
}

impl Waiting {
    /// The file `index`, waiting for the prefetcher if it is not here yet.
    fn take(&mut self, index: u32, cancel: &AtomicBool) -> Result<ReadyFile, ComposeError> {
        loop {
            if let Some(ready) = self.arrived.remove(&index) {
                return Ok(ready);
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(ComposeError::Cancelled);
            }
            match self.files.recv() {
                Ok(Ok(ready)) => {
                    self.arrived.insert(ready.index, ready);
                }
                Ok(Err(error)) => return Err(ComposeError::Unreadable(error)),
                Err(_) => {
                    return Err(if cancel.load(Ordering::Relaxed) {
                        ComposeError::Cancelled
                    } else {
                        failed("a video file never arrived")
                    });
                }
            }
        }
    }
}

/// Renders the whole video into `input.video_out` (video only; the soundtrack is joined later).
pub fn compose(mut input: ComposeInput) -> Result<(), ComposeError> {
    let cancel = input.cancel.clone();
    let fonts = input.fonts.build().map_err(|e| failed(e.to_string()))?;
    let teacher = input.teacher.clone();
    let has_camera = replay_model::timeline::has_camera(&teacher);
    let painter = StagePainter::new(fonts.clone(), has_camera, input.card.as_ref())
        .map_err(|e| failed(e.to_string()))?;
    let layout = *painter.layout();
    let stage = layout.stage;
    let stage_size = (stage.w as u32, stage.h as u32);
    let mut frame = SkiaCanvas::new(WIDTH, HEIGHT, fonts).map_err(|e| failed(e.to_string()))?;
    let mut board_painter = BoardPainter::new(&frame, stage_size.0, stage_size.1);
    let mut board = BoardReplay::new(std::mem::take(&mut input.records));
    let mut pictures = StaticPictures::new();
    for ((deck, n), bytes) in &input.pictures {
        if let Some(image) = decode_image(bytes) {
            pictures.insert(deck, *n, image);
        }
    }

    let mut tracks: Vec<Track> = teacher
        .iter()
        .filter(|f| matches!(f.kind, FileKind::Screen | FileKind::Camera))
        .filter(|f| f.end_ms > f.start_ms && f.end_ms > 0.0 && f.start_ms < input.duration_ms)
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
    let mut waiting = Waiting {
        files: std::mem::replace(&mut input.files, std::sync::mpsc::channel().1),
        arrived: HashMap::new(),
    };

    let mut encoder = input
        .registry
        .spawn(
            &mut encode_command(
                &input.ffmpeg,
                input.profile,
                (WIDTH, HEIGHT),
                FPS,
                KEYFRAME_EVERY,
                &input.video_out,
            ),
            &input.dir.join("encode.log"),
        )
        .map_err(|_| failed("cannot start the encoder"))?;
    let mut sink = encoder
        .child
        .stdin
        .take()
        .ok_or_else(|| failed("no encoder input"))?;
    drop(encoder.child.stdout.take());

    let card_frames = preroll_frames(input.preroll_ms) as i64;
    let frames = total_frames(input.duration_ms, input.preroll_ms) as i64;
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

    for i in 0..frames {
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
            input.budget.set_now(t);
            // Open, advance and close the video files.
            for track in tracks.iter_mut().filter(|t| !t.closed) {
                if t >= track.file.end_ms {
                    close(track, &input.budget);
                    continue;
                }
                if track.decoder.is_none() && lesson_frame >= track.j0 - 2 * i64::from(FPS) {
                    let ready = waiting.take(track.file.index, &cancel)?;
                    log_number += 1;
                    let decoder = Decoder::spawn(
                        &input.registry,
                        &input.ffmpeg,
                        &ready.path,
                        track.size,
                        track.origin_s,
                        FPS,
                        &input.dir.join(format!("decode-{log_number}.log")),
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

            board.seek(t);
            if let Some(quiz) = input.quiz.as_ref() {
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
                        if let Some(quiz) = input.quiz.as_mut() {
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
        if i % i64::from(FPS) == 0 {
            input
                .progress
                .store(i * 1000 / i64::from(FPS), Ordering::Relaxed);
        }
    }
    for track in tracks.iter_mut().filter(|t| !t.closed) {
        close(track, &input.budget);
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

/// Stops a track's decoder and deletes its file.
fn close(track: &mut Track, budget: &Budget) {
    track.closed = true;
    track.decoder = None;
    if let Some(ready) = track.ready.take() {
        let _ = std::fs::remove_file(&ready.path);
        budget.release(ready.bytes);
    }
}
