//! Downloads the video files ahead of the frame being drawn, in the order they are needed, within
//! a byte budget. A file that starts within a few seconds of the frame is always taken, budget or
//! not: the files a lesson plays at once (a screen and a camera) must never wait for each other.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use replay_model::manifest::RecordingFile;
use tokio_util::sync::CancellationToken;

use super::probe;
use super::reader::{ReadError, RecordingReader};

/// How far ahead (ms) a file is taken whatever the budget says.
const ALWAYS_AHEAD_MS: f64 = 10_000.0;

/// The place of a piece that has finished: no file is near it.
const FINISHED: i64 = i64::MIN;

/// What is downloaded and not yet deleted, against what the render may hold.
#[derive(Debug)]
pub struct Budget {
    cap: u64,
    used: AtomicU64,
    /// Where each piece of the video is on the lesson's timeline (ms).
    now_ms: Vec<AtomicI64>,
}

impl Budget {
    /// A budget of `cap` bytes for pieces that begin at `starts_ms` on the lesson's timeline.
    pub fn new(cap: u64, starts_ms: &[f64]) -> Arc<Self> {
        Arc::new(Self {
            cap,
            used: AtomicU64::new(0),
            now_ms: starts_ms
                .iter()
                .map(|ms| AtomicI64::new(*ms as i64))
                .collect(),
        })
    }

    /// Where piece `piece` is on the lesson's timeline.
    pub fn set_now(&self, piece: usize, ms: f64) {
        if let Some(now) = self.now_ms.get(piece) {
            now.store(ms as i64, Ordering::Relaxed);
        }
    }

    /// Piece `piece` has no more to draw.
    pub fn finish(&self, piece: usize) {
        if let Some(now) = self.now_ms.get(piece) {
            now.store(FINISHED, Ordering::Relaxed);
        }
    }

    pub fn release(&self, bytes: u64) {
        self.used.fetch_sub(
            bytes.min(self.used.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
    }

    fn may_take(&self, bytes: u64, start_ms: f64) -> bool {
        let near = self.now_ms.iter().any(|now| {
            let now = now.load(Ordering::Relaxed);
            now != FINISHED && start_ms - now as f64 <= ALWAYS_AHEAD_MS
        });
        near || self.used.load(Ordering::Relaxed) + bytes <= self.cap
    }
}

/// A file downloaded and probed. Dropped, it is deleted and its bytes given back.
#[derive(Debug)]
pub struct ReadyFile {
    pub index: u32,
    pub path: PathBuf,
    /// When its first picture is, in seconds into the file.
    pub first_picture_s: f64,
    pub bytes: u64,
    budget: Option<Arc<Budget>>,
}

impl Drop for ReadyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(budget) = &self.budget {
            budget.release(self.bytes);
        }
    }
}

impl ReadyFile {
    #[cfg(test)]
    pub fn for_test(index: u32, path: PathBuf) -> Self {
        Self {
            index,
            path,
            first_picture_s: 0.0,
            bytes: 0,
            budget: None,
        }
    }
}

/// The files the prefetcher has finished, shared by the pieces of the video that use them: a file
/// stays until the last piece that needs it lets it go.
#[derive(Debug)]
pub struct Library {
    state: Mutex<LibraryState>,
    wake: Condvar,
}

#[derive(Debug)]
struct LibraryState {
    ready: HashMap<u32, Arc<ReadyFile>>,
    /// How many pieces have yet to let each file go.
    users: HashMap<u32, usize>,
    error: Option<ReadError>,
    ended: bool,
}

impl Library {
    pub fn new(users: HashMap<u32, usize>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(LibraryState {
                ready: HashMap::new(),
                users,
                error: None,
                ended: false,
            }),
            wake: Condvar::new(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, LibraryState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish(&self, result: Result<ReadyFile, ReadError>) {
        let mut state = self.state();
        match result {
            Ok(file) => {
                state.ready.insert(file.index, Arc::new(file));
            }
            Err(error) => state.error = Some(error),
        }
        self.wake.notify_all();
    }

    /// No more files will come.
    pub fn end(&self) {
        self.state().ended = true;
        self.wake.notify_all();
    }

    /// The file `index`, waiting for the prefetcher if it is not here yet.
    pub fn take(
        &self,
        index: u32,
        cancel: &AtomicBool,
    ) -> Result<Arc<ReadyFile>, super::compose::ComposeError> {
        use super::compose::ComposeError;
        let mut state = self.state();
        loop {
            if let Some(file) = state.ready.get(&index) {
                return Ok(file.clone());
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(ComposeError::Cancelled);
            }
            if let Some(error) = state.error {
                return Err(ComposeError::Unreadable(error));
            }
            if state.ended {
                return Err(ComposeError::Failed("a video file never arrived".into()));
            }
            state = self
                .wake
                .wait_timeout(state, Duration::from_millis(200))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// A piece is done with file `index`; the last to be done deletes it.
    pub fn release(&self, index: u32) {
        let mut state = self.state();
        if let Some(users) = state.users.get_mut(&index) {
            *users = users.saturating_sub(1);
            if *users == 0 {
                state.ready.remove(&index);
            }
        }
    }
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
    pub library: Arc<Library>,
    pub stop: CancellationToken,
}

impl Prefetch {
    /// Downloads the files into `dir` and sends each when it is whole. Ends after the last, or when
    /// `stop` is cancelled; an error is sent as the last message.
    pub async fn run(self) {
        let library = self.library.clone();
        self.download().await;
        library.end();
    }

    async fn download(self) {
        let Self {
            reader,
            files,
            dir,
            ffprobe,
            probe_timeout,
            budget,
            library,
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
                    budget: Some(budget.clone()),
                })
            };
            let result = tokio::select! {
                r = work => r,
                _ = stop.cancelled() => return,
            };
            let failed = result.is_err();
            budget.used.fetch_add(file.size, Ordering::Relaxed);
            library.publish(result);
            if failed {
                return;
            }
        }
    }
}
