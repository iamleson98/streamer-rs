//! Downloads the video files ahead of the frame being drawn, in the order they are needed, within
//! a byte budget. A file that starts within a few seconds of the frame is always taken, budget or
//! not: the files a lesson plays at once (a screen and a camera) must never wait for each other.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use replay_model::manifest::RecordingFile;
use tokio_util::sync::CancellationToken;

use super::probe;
use super::reader::{ReadError, RecordingReader};

/// How far ahead (ms) a file is taken whatever the budget says.
const ALWAYS_AHEAD_MS: f64 = 10_000.0;

/// What is downloaded and not yet deleted, against what the render may hold.
#[derive(Debug)]
pub struct Budget {
    cap: u64,
    used: AtomicU64,
    now_ms: AtomicI64,
}

impl Budget {
    pub fn new(cap: u64) -> Arc<Self> {
        Arc::new(Self {
            cap,
            used: AtomicU64::new(0),
            now_ms: AtomicI64::new(0),
        })
    }

    /// Where the compositor is on the lesson's timeline.
    pub fn set_now(&self, ms: f64) {
        self.now_ms.store(ms as i64, Ordering::Relaxed);
    }

    pub fn release(&self, bytes: u64) {
        self.used.fetch_sub(
            bytes.min(self.used.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
    }

    fn may_take(&self, bytes: u64, start_ms: f64) -> bool {
        let near = start_ms - self.now_ms.load(Ordering::Relaxed) as f64 <= ALWAYS_AHEAD_MS;
        near || self.used.load(Ordering::Relaxed) + bytes <= self.cap
    }
}

/// A file downloaded and probed.
#[derive(Debug)]
pub struct ReadyFile {
    pub index: u32,
    pub path: PathBuf,
    /// When its first picture is, in seconds into the file.
    pub first_picture_s: f64,
    pub bytes: u64,
}

/// What a prefetch downloads and where it hands it.
#[derive(Debug)]
pub struct Prefetch {
    pub reader: Arc<RecordingReader>,
    /// Video files, in the order they begin.
    pub files: Vec<RecordingFile>,
    pub dir: PathBuf,
    pub ffprobe: PathBuf,
    pub probe_timeout: Duration,
    pub budget: Arc<Budget>,
    pub tx: std::sync::mpsc::Sender<Result<ReadyFile, ReadError>>,
    pub stop: CancellationToken,
}

impl Prefetch {
    /// Downloads the files into `dir` and sends each when it is whole. Ends after the last, or when
    /// `stop` is cancelled; an error is sent as the last message.
    pub async fn run(self) {
        let Self {
            reader,
            files,
            dir,
            ffprobe,
            probe_timeout,
            budget,
            tx,
            stop,
        } = self;
        for file in files {
            while !budget.may_take(file.size, file.start_ms) {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                    _ = stop.cancelled() => return,
                }
            }
            let path = dir.join(format!("{}.webm", file.index));
            let work = async {
                reader.download_file(file.index, file.size, &path).await?;
                let first = probe::first_picture(&ffprobe, &path, probe_timeout)
                    .await
                    .map_err(|_| ReadError("a video file cannot be read"))?;
                Ok(ReadyFile {
                    index: file.index,
                    path: path.clone(),
                    first_picture_s: first,
                    bytes: file.size,
                })
            };
            let result = tokio::select! {
                r = work => r,
                _ = stop.cancelled() => return,
            };
            let failed = result.is_err();
            budget.used.fetch_add(file.size, Ordering::Relaxed);
            if tx.send(result).is_err() || failed {
                return;
            }
        }
    }
}
