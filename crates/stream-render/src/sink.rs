//! The sink: where the render page sends its output. Parts arrive as `POST /__sink/part/{n}`
//! (octet-stream, n from 1) and are appended in order to one raw file. The page awaits each answer,
//! and the answer comes only once the part is in the file: that is the backpressure, so the worker
//! never holds more than a chunk of the output in memory. The end is `POST /__sink/done` with the
//! totals, or `POST /__sink/error` with one code of a closed list.

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{Mutex, mpsc};

/// The page cuts its output in exact parts of this size (the last one is smaller); nothing bigger
/// is accepted.
pub const PART_MAX_BYTES: u64 = 16 << 20;

/// The failure codes the site's render `finish` accepts from a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishCode {
    RecordingUnreadable,
    RenderFailed,
}

impl FinishCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecordingUnreadable => "recording_unreadable",
            Self::RenderFailed => "render_failed",
        }
    }

    /// Maps the page's error code. Only an unreadable recording is told apart: the others (an
    /// unsupported browser, a decode or encode failure, a sink failure) are the render's own failure,
    /// and a code that is not on the page's list is one too.
    pub fn from_page_code(code: &str) -> Self {
        match code {
            "recording_unreadable" => Self::RecordingUnreadable,
            _ => Self::RenderFailed,
        }
    }
}

/// What the page reported at the end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkEvent {
    Done {
        bytes: u64,
        duration_ms: u64,
        frames: u64,
    },
    Failed(FinishCode),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    parts: u64,
    bytes: u64,
    duration_ms: u64,
    #[serde(default)]
    frames: u64,
}

#[derive(Debug, Deserialize)]
pub struct PageError {
    #[serde(default)]
    code: String,
}

#[derive(Debug)]
struct Inner {
    out: Option<BufWriter<File>>,
    next_part: u64,
    bytes: u64,
    /// Done, failed or broken: nothing more is accepted.
    closed: bool,
}

/// The shared state of one render's sink.
#[derive(Debug)]
pub struct Sink {
    state: Mutex<Inner>,
    events: mpsc::Sender<SinkEvent>,
    max_total_bytes: u64,
}

impl Sink {
    /// Creates the raw file at `path` (it must not exist). The first end event goes to `events`.
    pub async fn create(
        path: &Path,
        events: mpsc::Sender<SinkEvent>,
        max_total_bytes: u64,
    ) -> std::io::Result<Arc<Self>> {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
        Ok(Arc::new(Self {
            state: Mutex::new(Inner {
                out: Some(BufWriter::with_capacity(256 << 10, file)),
                next_part: 1,
                bytes: 0,
                closed: false,
            }),
            events,
            max_total_bytes,
        }))
    }

    /// Reports the end. Only the first event is of interest: the receiver is read once.
    fn emit(&self, event: SinkEvent) {
        let _ = self.events.try_send(event);
    }

    async fn break_with(&self, state: &mut Inner, code: StatusCode) -> StatusCode {
        state.closed = true;
        state.out = None;
        self.emit(SinkEvent::Failed(FinishCode::RenderFailed));
        code
    }

    /// Appends part `n`, one chunk at a time, and answers when the last chunk is in the file.
    pub async fn part(&self, n: u64, body: Body) -> StatusCode {
        let mut state = self.state.lock().await;
        if state.closed {
            return StatusCode::CONFLICT;
        }
        if n != state.next_part {
            // A part out of order is the page's mistake, and a part that comes twice means it lost
            // track: either way the file can no longer be trusted.
            return self.break_with(&mut state, StatusCode::CONFLICT).await;
        }
        let mut stream = body.into_data_stream();
        let mut written: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                return self.break_with(&mut state, StatusCode::BAD_REQUEST).await;
            };
            written += chunk.len() as u64;
            if written > PART_MAX_BYTES || state.bytes + written > self.max_total_bytes {
                return self
                    .break_with(&mut state, StatusCode::PAYLOAD_TOO_LARGE)
                    .await;
            }
            let wrote = match state.out.as_mut() {
                Some(out) => out.write_all(&chunk).await,
                None => Err(std::io::ErrorKind::BrokenPipe.into()),
            };
            if wrote.is_err() {
                return self
                    .break_with(&mut state, StatusCode::INTERNAL_SERVER_ERROR)
                    .await;
            }
        }
        let flushed = match state.out.as_mut() {
            Some(out) => out.flush().await,
            None => Err(std::io::ErrorKind::BrokenPipe.into()),
        };
        if written == 0 {
            return self.break_with(&mut state, StatusCode::BAD_REQUEST).await;
        }
        if flushed.is_err() {
            return self
                .break_with(&mut state, StatusCode::INTERNAL_SERVER_ERROR)
                .await;
        }
        state.bytes += written;
        state.next_part += 1;
        StatusCode::NO_CONTENT
    }

    /// The page says it is finished: the totals must be what arrived.
    pub async fn done(&self, totals: Totals) -> StatusCode {
        let mut state = self.state.lock().await;
        if state.closed {
            return StatusCode::CONFLICT;
        }
        let parts = state.next_part - 1;
        if parts == 0 || totals.parts != parts || totals.bytes != state.bytes {
            return self.break_with(&mut state, StatusCode::CONFLICT).await;
        }
        let flushed = match state.out.take() {
            Some(mut out) => out.shutdown().await,
            None => Err(std::io::ErrorKind::BrokenPipe.into()),
        };
        state.closed = true;
        if flushed.is_err() {
            self.emit(SinkEvent::Failed(FinishCode::RenderFailed));
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        self.emit(SinkEvent::Done {
            bytes: state.bytes,
            duration_ms: totals.duration_ms,
            frames: totals.frames,
        });
        StatusCode::NO_CONTENT
    }

    /// The page says it failed.
    pub async fn error(&self, error: PageError) -> StatusCode {
        let mut state = self.state.lock().await;
        state.closed = true;
        state.out = None;
        self.emit(SinkEvent::Failed(FinishCode::from_page_code(&error.code)));
        StatusCode::NO_CONTENT
    }
}

pub async fn part_route(
    State(sink): State<Arc<Sink>>,
    UrlPath(n): UrlPath<u64>,
    body: Body,
) -> StatusCode {
    sink.part(n, body).await
}

pub async fn done_route(State(sink): State<Arc<Sink>>, Json(totals): Json<Totals>) -> StatusCode {
    sink.done(totals).await
}

pub async fn error_route(
    State(sink): State<Arc<Sink>>,
    Json(error): Json<PageError>,
) -> StatusCode {
    sink.error(error).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tokio::sync::oneshot;

    async fn new_sink() -> (
        Arc<Sink>,
        mpsc::Receiver<SinkEvent>,
        tempfile::TempDir,
        std::path::PathBuf,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raw.mp4");
        let (tx, rx) = mpsc::channel(4);
        let sink = Sink::create(&path, tx, 100 << 20).await.unwrap();
        (sink, rx, dir, path)
    }

    fn totals(parts: u64, bytes: u64) -> Totals {
        Totals {
            parts,
            bytes,
            duration_ms: 3000,
            frames: 90,
        }
    }

    #[tokio::test]
    async fn parts_in_order_become_one_file_and_done_reports_the_totals() {
        let (sink, mut rx, _dir, path) = new_sink().await;
        assert_eq!(
            sink.part(1, Body::from("hello ")).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            sink.part(2, Body::from("world")).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(sink.done(totals(2, 11)).await, StatusCode::NO_CONTENT);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
        assert_eq!(
            rx.try_recv().unwrap(),
            SinkEvent::Done {
                bytes: 11,
                duration_ms: 3000,
                frames: 90
            }
        );
    }

    #[tokio::test]
    async fn a_part_out_of_order_breaks_the_render() {
        let (sink, mut rx, _dir, path) = new_sink().await;
        assert_eq!(sink.part(2, Body::from("x")).await, StatusCode::CONFLICT);
        assert_eq!(
            rx.try_recv().unwrap(),
            SinkEvent::Failed(FinishCode::RenderFailed)
        );
        assert_eq!(sink.part(1, Body::from("x")).await, StatusCode::CONFLICT);
        assert_eq!(std::fs::read(&path).unwrap(), b"");

        let (sink, mut rx, _dir, _path) = new_sink().await;
        assert_eq!(sink.part(1, Body::from("a")).await, StatusCode::NO_CONTENT);
        assert_eq!(sink.part(1, Body::from("a")).await, StatusCode::CONFLICT);
        assert_eq!(
            rx.try_recv().unwrap(),
            SinkEvent::Failed(FinishCode::RenderFailed)
        );
    }

    #[tokio::test]
    async fn an_oversized_part_is_refused_before_it_is_all_written() {
        let (sink, mut rx, _dir, path) = new_sink().await;
        let big = vec![7u8; (PART_MAX_BYTES + 1) as usize];
        assert_eq!(
            sink.part(1, Body::from(big)).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            SinkEvent::Failed(FinishCode::RenderFailed)
        );
        assert!(std::fs::metadata(&path).unwrap().len() <= PART_MAX_BYTES);

        let exact = vec![1u8; PART_MAX_BYTES as usize];
        let (sink, _rx, _dir, _path) = new_sink().await;
        assert_eq!(
            sink.part(1, Body::from(exact)).await,
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn the_whole_file_has_a_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        let sink = Sink::create(&dir.path().join("raw.mp4"), tx, 10)
            .await
            .unwrap();
        assert_eq!(
            sink.part(1, Body::from("123456")).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            sink.part(2, Body::from("123456")).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            SinkEvent::Failed(FinishCode::RenderFailed)
        );
    }

    #[tokio::test]
    async fn an_empty_part_and_a_broken_upload_fail() {
        let (sink, mut rx, _dir, _path) = new_sink().await;
        assert_eq!(sink.part(1, Body::empty()).await, StatusCode::BAD_REQUEST);
        assert_eq!(
            rx.try_recv().unwrap(),
            SinkEvent::Failed(FinishCode::RenderFailed)
        );

        let (sink, mut rx, _dir, _path) = new_sink().await;
        let broken = Body::from_stream(futures_util::stream::iter(vec![
            Ok(Bytes::from_static(b"abc")),
            Err(std::io::Error::other("reset")),
        ]));
        assert_eq!(sink.part(1, broken).await, StatusCode::BAD_REQUEST);
        assert_eq!(
            rx.try_recv().unwrap(),
            SinkEvent::Failed(FinishCode::RenderFailed)
        );
    }

    #[tokio::test]
    async fn the_answer_waits_for_the_whole_part_to_be_in_the_file() {
        let (sink, _rx, _dir, path) = new_sink().await;
        let (release, wait) = oneshot::channel::<()>();
        let body = Body::from_stream(futures_util::stream::unfold(
            (0u8, Some(wait)),
            |(step, wait)| async move {
                match step {
                    0 => Some((
                        Ok::<_, std::io::Error>(Bytes::from_static(b"first ")),
                        (1, wait),
                    )),
                    1 => {
                        let _ = wait?.await;
                        Some((Ok(Bytes::from_static(b"second")), (2, None)))
                    }
                    _ => None,
                }
            },
        ));
        let worker = sink.clone();
        let mut handle = tokio::spawn(async move { worker.part(1, body).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !handle.is_finished(),
            "answered before the part was complete"
        );
        release.send(()).unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), &mut handle)
                .await
                .unwrap()
                .unwrap(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"first second");
    }

    #[tokio::test]
    async fn done_with_other_totals_is_a_failure() {
        for wrong in [totals(1, 4), totals(2, 3), totals(0, 0)] {
            let (sink, mut rx, _dir, _path) = new_sink().await;
            assert_eq!(
                sink.part(1, Body::from("abc")).await,
                StatusCode::NO_CONTENT
            );
            assert_eq!(sink.done(wrong).await, StatusCode::CONFLICT);
            assert_eq!(
                rx.try_recv().unwrap(),
                SinkEvent::Failed(FinishCode::RenderFailed)
            );
        }
        let (sink, mut rx, _dir, _path) = new_sink().await;
        assert_eq!(sink.done(totals(0, 0)).await, StatusCode::CONFLICT);
        assert_eq!(
            rx.try_recv().unwrap(),
            SinkEvent::Failed(FinishCode::RenderFailed)
        );
    }

    #[tokio::test]
    async fn the_pages_error_codes_map_to_the_finish_codes() {
        for (page, expected) in [
            ("recording_unreadable", FinishCode::RecordingUnreadable),
            ("unsupported_browser", FinishCode::RenderFailed),
            ("decode_failed", FinishCode::RenderFailed),
            ("encode_failed", FinishCode::RenderFailed),
            ("sink_failed", FinishCode::RenderFailed),
            ("render_failed", FinishCode::RenderFailed),
            ("something the page never says", FinishCode::RenderFailed),
            ("", FinishCode::RenderFailed),
        ] {
            let (sink, mut rx, _dir, _path) = new_sink().await;
            let status = sink.error(PageError { code: page.into() }).await;
            assert_eq!(status, StatusCode::NO_CONTENT);
            assert_eq!(
                rx.try_recv().unwrap(),
                SinkEvent::Failed(expected),
                "{page}"
            );
            assert_eq!(sink.part(1, Body::from("x")).await, StatusCode::CONFLICT);
        }
    }

    #[test]
    fn the_finish_codes_are_the_ones_the_site_accepts_from_a_worker() {
        assert_eq!(
            FinishCode::RecordingUnreadable.as_str(),
            "recording_unreadable"
        );
        assert_eq!(FinishCode::RenderFailed.as_str(), "render_failed");
    }
}
