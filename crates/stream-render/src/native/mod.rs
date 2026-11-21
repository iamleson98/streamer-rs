//! The native engine: reads the recording itself, decodes its video with ffmpeg children, draws
//! every frame with Skia and encodes them with one more child. It makes the same MP4 as the render
//! page in a headless browser, without the browser.
//!
//! The steps of a render:
//!
//! 1. the manifest, then the board log and the deck pictures (all of the board is read up front);
//! 2. in parallel: the soundtrack (the teacher's sound files mixed by one ffmpeg), and the video (a
//!    prefetcher downloads the teacher's video files in the order they are needed, the compositor
//!    draws the frames and one encoder writes them);
//! 3. one ffmpeg joins the two without re-encoding and puts the moov atom first.
//!
//! The quiz and Quest stages are not drawn yet: a lesson with a game fails here with
//! `render_failed`, which is why Chromium stays the default engine.

pub mod audio;
pub mod compose;
pub mod decode;
pub mod encode;
pub mod ffmpeg;
pub mod fonts;
pub mod prefetch;
pub mod probe;
pub mod reader;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use replay_model::manifest::{FileKind, RecordingManifest};
use replay_model::timeline::teacher_files;
use replay_model::{BoardRecord, RecordKind, parse_board_log};
use stage_draw::{CardText, Lang, Strings};
use tokio::process::Command as AsyncCommand;
use tokio_util::sync::CancellationToken;

use crate::engine::{RenderEngine, RenderJob};
use crate::job::Ended;
use crate::sink::FinishCode;
use compose::{ComposeError, ComposeInput, compose};
use ffmpeg::Registry;
use fonts::{FontError, FontFiles};
use prefetch::Budget;
use reader::{ReadError, RecordingReader};

/// The most a board log may be once unzipped.
const MAX_BOARD_UNZIPPED: u64 = 256 << 20;
/// The most deck page pictures one render reads.
const MAX_PICTURES: usize = 2_000;

/// Why a render did not finish, before it is told as a finish code.
#[derive(Debug, PartialEq, Eq)]
enum RunError {
    Unreadable,
    Failed,
}

impl From<ReadError> for RunError {
    fn from(error: ReadError) -> Self {
        tracing::error!(%error, "the recording could not be read");
        Self::Unreadable
    }
}

fn failed(what: &str) -> RunError {
    tracing::error!("{what}");
    RunError::Failed
}

/// Renders natively.
#[derive(Debug)]
pub struct NativeEngine {
    fonts: Arc<FontFiles>,
    profile: encode::Profile,
}

impl NativeEngine {
    /// Reads and checks the fonts in `font_dir`.
    pub fn new(font_dir: &Path) -> Result<Self, FontError> {
        Ok(Self {
            fonts: FontFiles::load(font_dir)?,
            profile: encode::Profile::Production,
        })
    }

    /// The same engine with another encoder profile (the lossless reference of `render-compare`).
    pub fn with_profile(mut self, profile: encode::Profile) -> Self {
        self.profile = profile;
        self
    }
}

/// Stops everything a render started when it goes out of scope.
struct StopGuard {
    registry: Registry,
    cancel: Arc<AtomicBool>,
    stop: CancellationToken,
    /// The tasks that must not outlive the render.
    tasks: std::sync::Mutex<Vec<tokio::task::AbortHandle>>,
    /// The compositor's thread, awaited after a stop so nothing writes into the directory the worker
    /// is about to delete.
    compose: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl StopGuard {
    fn track<T>(&self, handle: &tokio::task::JoinHandle<T>) {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle.abort_handle());
    }
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        for task in self.tasks.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            task.abort();
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.stop.cancel();
        self.registry.kill_all();
    }
}

impl RenderEngine for NativeEngine {
    async fn render(&self, job: RenderJob<'_>) -> Result<PathBuf, Ended> {
        let guard = StopGuard {
            registry: Registry::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: CancellationToken::new(),
            tasks: std::sync::Mutex::default(),
            compose: std::sync::Mutex::default(),
        };
        let lost = job.heartbeat.lost();
        let deadline = tokio::time::sleep(job.settings.render_deadline(job.file_ms));
        let run = self.run(&job, &guard);
        let result = tokio::select! {
            biased;
            _ = job.shutdown.cancelled() => Err(Ended::Shutdown),
            _ = lost.cancelled() => Err(Ended::LeaseLost),
            _ = deadline => {
                tracing::error!("the render ran past its deadline");
                Err(Ended::Failed(FinishCode::RenderFailed))
            }
            result = run => result.map_err(|error| Ended::Failed(match error {
                RunError::Unreadable => FinishCode::RecordingUnreadable,
                RunError::Failed => FinishCode::RenderFailed,
            })),
        };
        let compose = guard
            .compose
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        drop(guard);
        if let Some(handle) = compose {
            // Its children are killed: it ends within moments.
            let _ = tokio::time::timeout(Duration::from_secs(10), handle).await;
        }
        result
    }
}

impl NativeEngine {
    async fn run(&self, job: &RenderJob<'_>, guard: &StopGuard) -> Result<PathBuf, RunError> {
        let (claim, settings) = (job.claim, job.settings);
        let reader = Arc::new(RecordingReader::new(
            job.api_base_url,
            &claim.recording_id,
            claim.render_token.clone(),
            job.heartbeat.tokens(),
            settings.attempts,
            settings.retry_delay,
        )?);

        let manifest_json = reader.manifest().await?;
        let manifest = serde_json::to_vec(&manifest_json)
            .ok()
            .and_then(|bytes| RecordingManifest::parse(&bytes).ok())
            .ok_or_else(|| {
                tracing::error!("the manifest is not a recording's");
                RunError::Unreadable
            })?;
        let files = manifest.numbered();
        let teacher = teacher_files(&files, &manifest.teacher_id);

        let board_file = files
            .iter()
            .find(|f| f.kind == FileKind::Board)
            .map(|f| f.index);
        let records = if reader.has_board_address() || board_file.is_some() {
            let gz = reader.board_log(board_file).await?;
            tokio::task::spawn_blocking(move || read_log(&gz))
                .await
                .map_err(|_| failed("the board log reader stopped"))?
                .ok_or_else(|| {
                    tracing::error!("the board log is not a log");
                    RunError::Unreadable
                })?
        } else {
            Vec::new()
        };
        if records.iter().any(|r| r.kind == RecordKind::Quiz) {
            return Err(failed("the native engine does not draw the quiz stage yet"));
        }

        let mut pictures = Vec::new();
        for (deck, n) in deck_pages(&records).into_iter().take(MAX_PICTURES) {
            // A page that cannot be read stays unpainted on its sheet, as in the web exporter.
            if let Ok(bytes) = reader.deck_page(&deck, n).await {
                pictures.push(((deck, n), bytes));
            }
        }

        let dir = job.dir.to_path_buf();
        let preroll_ms = job.card.preroll_ms;
        let lang = Lang::of(&claim.locale);
        let card = (preroll_ms > 0).then(|| CardText {
            title: job.card.title.clone(),
            teacher: job.card.teacher.clone(),
            starts_in: Strings::of(lang)
                .get("replay.card.startsIn")
                .unwrap_or_default()
                .to_owned(),
        });

        // The soundtrack, in parallel with the video.
        let sounds: Vec<_> = teacher
            .iter()
            .filter(|f| matches!(f.kind, FileKind::Voice | FileKind::ScreenAudio))
            .filter(|f| f.end_ms > f.start_ms && f.end_ms > 0.0)
            .cloned()
            .collect();
        let mut audio = tokio::spawn(
            Soundtrack {
                reader: reader.clone(),
                sounds,
                dir: dir.clone(),
                ffmpeg: settings.ffmpeg.clone(),
                timeout: settings.ffmpeg_timeout,
                preroll_ms,
                file_ms: job.file_ms,
            }
            .make(),
        );
        guard.track(&audio);

        // The video: the prefetcher feeds the compositor.
        let mut video_files: Vec<_> = teacher
            .iter()
            .filter(|f| matches!(f.kind, FileKind::Screen | FileKind::Camera))
            .filter(|f| {
                f.end_ms > f.start_ms && f.end_ms > 0.0 && f.start_ms < manifest.duration_ms
            })
            .cloned()
            .collect();
        video_files.sort_by(|a, b| {
            a.start_ms
                .total_cmp(&b.start_ms)
                .then(a.index.cmp(&b.index))
        });
        let budget = Budget::new(settings.prefetch_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        let prefetching = tokio::spawn(
            prefetch::Prefetch {
                reader: reader.clone(),
                files: video_files,
                dir: dir.clone(),
                ffprobe: settings.ffprobe.clone(),
                probe_timeout: settings.probe_timeout,
                budget: budget.clone(),
                tx,
                stop: guard.stop.clone(),
            }
            .run(),
        );
        guard.track(&prefetching);
        let progress = Arc::new(AtomicI64::new(0));
        let video_out = dir.join("video.mp4");
        let input = ComposeInput {
            ffmpeg: settings.ffmpeg.clone(),
            dir: dir.clone(),
            video_out: video_out.clone(),
            fonts: self.fonts.clone(),
            teacher,
            records,
            pictures,
            card,
            preroll_ms,
            duration_ms: manifest.duration_ms,
            quiz: None,
            profile: self.profile,
            files: rx,
            budget,
            registry: guard.registry.clone(),
            cancel: guard.cancel.clone(),
            progress: progress.clone(),
        };
        let (done_tx, mut composed) = tokio::sync::oneshot::channel();
        let thread = tokio::task::spawn_blocking(move || {
            let _ = done_tx.send(compose(input));
        });
        *guard.compose.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread);
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let (mut video_done, mut soundtrack) = (false, None);
        // The first failure ends the render: the guard stops the other half.
        while !video_done || soundtrack.is_none() {
            tokio::select! {
                r = &mut composed, if !video_done => match r {
                    Ok(Ok(())) => video_done = true,
                    Ok(Err(ComposeError::Unreadable(error))) => return Err(error.into()),
                    Ok(Err(error)) => return Err(failed(&format!("the video failed: {error}"))),
                    Err(_) => return Err(failed("the compositor stopped")),
                },
                r = &mut audio, if soundtrack.is_none() => match r {
                    Ok(Ok(path)) => soundtrack = Some(path),
                    Ok(Err(error)) => return Err(error),
                    Err(_) => return Err(failed("the soundtrack task stopped")),
                },
                _ = tick.tick() => job.heartbeat.set_position(progress.load(Ordering::Relaxed)),
            }
        }
        let soundtrack = soundtrack.ok_or_else(|| failed("no soundtrack"))?;

        let finished = dir.join("final.mp4");
        run_ffmpeg(
            encode::mux_command(&settings.ffmpeg, &video_out, &soundtrack, &finished),
            settings.ffmpeg_timeout,
        )
        .await
        .map_err(|e| failed(&format!("the join failed: {e}")))?;
        let _ = tokio::fs::remove_file(&video_out).await;
        let _ = tokio::fs::remove_file(&soundtrack).await;
        Ok(finished)
    }
}

/// Runs an ffmpeg command to its end.
async fn run_ffmpeg(command: std::process::Command, timeout: Duration) -> Result<(), String> {
    let mut command = AsyncCommand::from(command);
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| "timed out".to_owned())?
        .map_err(|_| "cannot run ffmpeg".to_owned())?;
    if output.status.success() {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<&str> = text.lines().collect();
        Err(lines[lines.len().saturating_sub(5)..].join(" | "))
    }
}

/// The teacher's sound files and how they are mixed.
#[derive(Debug)]
struct Soundtrack {
    reader: Arc<RecordingReader>,
    sounds: Vec<replay_model::manifest::RecordingFile>,
    dir: PathBuf,
    ffmpeg: PathBuf,
    timeout: Duration,
    preroll_ms: u64,
    file_ms: i64,
}

impl Soundtrack {
    /// Downloads the sound files and mixes them into `audio.m4a`.
    async fn make(self) -> Result<PathBuf, RunError> {
        let mut placed = Vec::new();
        for sound in &self.sounds {
            let path = self.dir.join(format!("sound-{}.webm", sound.index));
            self.reader
                .download_file(sound.index, sound.size, &path)
                .await?;
            placed.push(audio::Sound {
                path,
                start_ms: sound.start_ms,
                end_ms: sound.end_ms,
            });
        }
        let out = self.dir.join("audio.m4a");
        run_ffmpeg(
            audio::command(&self.ffmpeg, &placed, self.preroll_ms, self.file_ms, &out),
            self.timeout,
        )
        .await
        .map_err(|e| failed(&format!("the soundtrack failed: {e}")))?;
        for sound in &placed {
            let _ = tokio::fs::remove_file(&sound.path).await;
        }
        Ok(out)
    }
}

/// The board log: gunzipped (at most [`MAX_BOARD_UNZIPPED`] bytes) and parsed.
fn read_log(gz: &[u8]) -> Option<Vec<BoardRecord>> {
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .take(MAX_BOARD_UNZIPPED + 1)
        .read_to_end(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_BOARD_UNZIPPED)
        .then(|| parse_board_log(&String::from_utf8_lossy(&text)))
}

/// The deck pages the board ever shows, once each, in first-seen order.
fn deck_pages(records: &[BoardRecord]) -> Vec<(String, u32)> {
    let mut seen = std::collections::HashSet::new();
    let mut pages = Vec::new();
    for envelope in records.iter().filter_map(|r| r.elements.as_ref()).flatten() {
        let deck = envelope.data.get("deck").and_then(|v| v.as_str());
        let n = envelope.data.get("n").and_then(|v| v.as_u64());
        if let (Some(deck), Some(n)) = (deck, n)
            && (1..=120).contains(&n)
            && seen.insert((deck.to_owned(), n as u32))
        {
            pages.push((deck.to_owned(), n as u32));
        }
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gz(text: &str) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn a_board_log_is_gunzipped_and_parsed_and_garbage_is_refused() {
        let line = r#"{"t":5,"k":"elements","u":"u","elements":[]}"#;
        assert!(read_log(&gz(line)).is_some());
        assert!(read_log(b"not gzip").is_none());
    }

    #[test]
    fn deck_pages_are_listed_once_in_the_order_seen() {
        let deck = "d".repeat(26);
        let line = |n: u32| {
            format!(
                r#"{{"t":1,"k":"elements","u":"u","elements":[{{"id":"a{n}","version":1,"version_nonce":1,"is_deleted":false,"data":{{"type":"sheet","deck":"{deck}","n":{n}}}}}]}}"#
            )
        };
        let text = format!("{}\n{}\n{}", line(2), line(1), line(2));
        let records = parse_board_log(&text);
        assert_eq!(deck_pages(&records), [(deck.clone(), 2), (deck, 1)]);
    }
}
