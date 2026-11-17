//! One render, from the claim to the finish: a local server, a browser on the render page, a
//! heartbeat, then the remux, the checks and the upload. Whatever happens, the browser is killed,
//! the server stopped and the render's directory deleted before the next claim.

use std::path::{Path, PathBuf};

use stream_common::api::{ApiClient, ApiError, RenderClaim, RenderFinishInput, UploadedPart};
use stream_common::redact::register_secret;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::browser::{Browser, BrowserSession};
use crate::config::Settings;
use crate::heartbeat::{Heartbeat, HeartbeatTiming};
use crate::media;
use crate::server::{LocalServer, ServerConfig};
use crate::sink::{FinishCode, SinkEvent};
use crate::upload::{self, UploadLimits};

/// Why a render did not produce a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The render failed; the site is told with this code.
    Failed(FinishCode),
    /// The site no longer holds the render (cancelled, or the lease ran out).
    LeaseLost,
    /// The worker is shutting down: the lease is left to run out, and the site queues the render again.
    Shutdown,
}

/// A finished render, uploaded and waiting for the finish call.
#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    pub size_bytes: i64,
    pub rendered_ms: i64,
    pub parts: Vec<UploadedPart>,
}

/// Ids name directories and appear in addresses: only plain characters.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The render page's address: the inputs are in the fragment, which no server ever sees.
fn page_url(port: u16, token: &str, recording: &str, request: &str) -> String {
    format!(
        "http://127.0.0.1:{port}/render#api=/api/v4&token={}&recording={}&request={}&sink=/__sink",
        encode_component(token),
        encode_component(recording),
        encode_component(request),
    )
}

/// A render's directory, deleted when it goes out of scope as well as when `remove` is awaited.
#[derive(Debug)]
struct WorkDir {
    path: PathBuf,
    removed: bool,
}

impl WorkDir {
    async fn create(path: PathBuf) -> std::io::Result<Self> {
        let work = Self {
            path,
            removed: false,
        };
        tokio::fs::create_dir_all(&work.path).await?;
        Ok(work)
    }

    async fn remove(mut self) {
        self.removed = true;
        if let Err(error) = tokio::fs::remove_dir_all(&self.path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "cannot delete the render's directory");
        }
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if !self.removed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Removes whatever a worker that was killed left in the work directory.
pub async fn clean_work_dir(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let removed = match entry.file_type().await {
            Ok(t) if t.is_dir() => tokio::fs::remove_dir_all(&path).await,
            _ => tokio::fs::remove_file(&path).await,
        };
        if removed.is_err() {
            tracing::warn!("cannot delete what an earlier render left behind");
        }
    }
}

/// The worker: claims renders and runs them one at a time.
#[derive(Debug)]
pub struct Worker<B> {
    pub settings: Settings,
    pub api_base_url: String,
    pub api: ApiClient,
    pub storage: reqwest::Client,
    pub browser: B,
    pub shutdown: CancellationToken,
}

impl<B: Browser> Worker<B> {
    /// Runs one claimed render and tells the site how it ended.
    pub async fn render(&self, claim: RenderClaim) {
        let request = claim.request_id.clone();
        if !plain_id(&request) || !plain_id(&claim.recording_id) || claim.duration_ms <= 0 {
            tracing::error!("the site sent a render this worker cannot run");
            self.finish(&request, Err(Ended::Failed(FinishCode::RenderFailed)))
                .await;
            return;
        }
        let _token = register_secret(claim.render_token.expose());
        tracing::info!(duration_ms = claim.duration_ms, "render started");
        let heartbeat = Heartbeat::start(
            self.api.clone(),
            self.settings.worker_id.clone(),
            request.clone(),
            HeartbeatTiming {
                interval: self.settings.heartbeat_interval,
                lease_timeout: self.settings.lease_timeout,
            },
        );
        let result = match WorkDir::create(self.settings.work_dir.join(&request)).await {
            Ok(work) => {
                let result = self.produce(&claim, &work.path, &heartbeat).await;
                work.remove().await;
                result
            }
            Err(_) => Err(Ended::Failed(FinishCode::RenderFailed)),
        };
        let result = match (result, heartbeat.reason()) {
            (Err(_), Some(_)) => Err(Ended::LeaseLost),
            (other, _) => other,
        };
        heartbeat.finish().await;
        self.finish(&request, result).await;
    }

    async fn produce(
        &self,
        claim: &RenderClaim,
        dir: &Path,
        heartbeat: &Heartbeat,
    ) -> Result<Rendered, Ended> {
        let failed = Ended::Failed(FinishCode::RenderFailed);
        let (raw, finished) = (dir.join("raw.mp4"), dir.join("final.mp4"));
        let (events, mut ended) = mpsc::channel(4);
        let server = LocalServer::start(
            ServerConfig {
                web_dir: &self.settings.web_dir,
                api_base: &self.api_base_url,
                recording_id: &claim.recording_id,
                raw_path: &raw,
                max_raw_bytes: self.settings.max_raw_bytes,
            },
            events,
        )
        .await
        .map_err(|_| failed)?;
        let page = page_url(
            server.port(),
            claim.render_token.expose(),
            &claim.recording_id,
            &claim.request_id,
        );
        let page_result = self
            .run_page(claim, &page, dir, &mut ended, heartbeat)
            .await;
        server.stop().await;
        let bytes = page_result?;

        // From here on the page is done: the rest is files, and the lease is kept by the heartbeat.
        heartbeat.set_position(claim.duration_ms);
        let lost = heartbeat.lost();
        self.race(&lost, async {
            let raw_size = tokio::fs::metadata(&raw).await.map(|m| m.len());
            if raw_size.ok() != Some(bytes) {
                return Err(failed);
            }
            media::remux(
                &self.settings.ffmpeg,
                &raw,
                &finished,
                self.settings.ffmpeg_timeout,
            )
            .await
            .map_err(|error| {
                tracing::error!(%error, "remux failed");
                failed
            })?;
            let _ = tokio::fs::remove_file(&raw).await;
            let rendered_ms = media::probe(
                &self.settings.ffprobe,
                &finished,
                self.settings.probe_timeout,
            )
            .await
            .map_err(|error| {
                tracing::error!(%error, "the finished file was refused");
                failed
            })?;
            if !media::length_within_tolerance(rendered_ms, claim.duration_ms) {
                tracing::error!(
                    rendered_ms,
                    "the finished file's length is not the recording's"
                );
                return Err(failed);
            }
            let (size_bytes, parts) = upload::upload(
                &self.api,
                &self.storage,
                &self.settings.worker_id,
                &claim.request_id,
                &finished,
                UploadLimits {
                    attempts: self.settings.attempts,
                    retry_delay: self.settings.retry_delay,
                    part_timeout: self.settings.part_timeout,
                },
            )
            .await
            .map_err(|error| {
                tracing::error!(%error, "upload failed");
                failed
            })?;
            Ok(Rendered {
                size_bytes,
                rendered_ms,
                parts,
            })
        })
        .await
    }

    /// Runs `work` unless the worker is told to shut down or the lease is lost first.
    async fn race<T>(
        &self,
        lost: &CancellationToken,
        work: impl Future<Output = Result<T, Ended>>,
    ) -> Result<T, Ended> {
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => Err(Ended::Shutdown),
            _ = lost.cancelled() => Err(Ended::LeaseLost),
            result = work => result,
        }
    }

    /// Opens the page and waits for its end: done or an error from the page, a crash, the deadline,
    /// a lost lease or a shutdown. The browser is closed before this returns.
    async fn run_page(
        &self,
        claim: &RenderClaim,
        page: &str,
        dir: &Path,
        ended: &mut mpsc::Receiver<SinkEvent>,
        heartbeat: &Heartbeat,
    ) -> Result<u64, Ended> {
        let failed = Ended::Failed(FinishCode::RenderFailed);
        let mut session = self
            .browser
            .launch(page, &dir.join("profile"))
            .await
            .map_err(|error| {
                tracing::error!(%error, "the browser did not start");
                failed
            })?;
        let mut exited = session.exited();
        let mut tokens = heartbeat.tokens();
        let lost = heartbeat.lost();
        let deadline = tokio::time::sleep(self.settings.render_deadline(claim.duration_ms));
        tokio::pin!(deadline);
        let interval = self.settings.status_interval;
        let mut status = tokio::time::interval_at(Instant::now() + interval, interval);
        let mut last_progress = -1.0;

        let outcome = loop {
            tokio::select! {
                biased;
                Some(event) = ended.recv() => match event {
                    SinkEvent::Done { bytes, .. } => break Ok(bytes),
                    SinkEvent::Failed(code) => {
                        tracing::error!(code = code.as_str(), "the render page reported a failure");
                        break Err(Ended::Failed(code));
                    }
                },
                _ = self.shutdown.cancelled() => break Err(Ended::Shutdown),
                _ = lost.cancelled() => break Err(Ended::LeaseLost),
                // The guard `wait_for` returns is not `Send`: it is dropped inside the block.
                _ = async { let _ = exited.wait_for(|gone| *gone).await; } => {
                    tracing::error!("the browser exited");
                    break Err(failed);
                }
                _ = &mut deadline => {
                    tracing::error!("the render ran past its deadline");
                    break Err(failed);
                }
                _ = status.tick() => {
                    match session.status().await {
                        Ok(Some(page_status)) => {
                            if page_status.progress != last_progress {
                                last_progress = page_status.progress;
                                tracing::info!(state = page_status.state, progress = page_status.progress, "rendering");
                            }
                            heartbeat.set_position((page_status.progress * claim.duration_ms as f64) as i64);
                        }
                        Ok(None) => {}
                        Err(error) => tracing::warn!(%error, "cannot read the page's progress"),
                    }
                }
                Ok(()) = tokens.changed() => {
                    let fresh = tokens.borrow_and_update().clone();
                    if let Some(token) = fresh
                        && let Err(error) = session.set_token(token.expose()).await
                    {
                        tracing::warn!(%error, "cannot hand the page a fresh token");
                    }
                }
            }
        };
        session.close().await;
        outcome
    }

    /// Tells the site how the render ended. A render the worker was told to leave is reported as
    /// failed once (the site answers 409 when it no longer holds it); one left for a shutdown is not:
    /// the lease runs out and the site queues it again.
    async fn finish(&self, request: &str, result: Result<Rendered, Ended>) {
        let mut input = RenderFinishInput {
            worker_id: self.settings.worker_id.clone(),
            ..Default::default()
        };
        match result {
            Ok(rendered) => {
                tracing::info!(size_bytes = rendered.size_bytes, "render finished");
                input.size_bytes = Some(rendered.size_bytes);
                input.rendered_ms = Some(rendered.rendered_ms);
                input.parts = rendered.parts;
            }
            Err(Ended::Failed(code)) => {
                tracing::warn!(code = code.as_str(), "render failed");
                input.failure_code = Some(code.as_str().to_owned());
            }
            Err(Ended::LeaseLost) => {
                tracing::warn!("render dropped: the site no longer holds it");
                input.failure_code = Some(FinishCode::RenderFailed.as_str().to_owned());
                if let Err(error) = self.api.render_finish(request, &input).await {
                    tracing::warn!(%error, "the site was not told");
                }
                return;
            }
            Err(Ended::Shutdown) => {
                tracing::info!("render dropped for shutdown; the site queues it again");
                return;
            }
        }
        for attempt in 1..=self.settings.attempts {
            match self.api.render_finish(request, &input).await {
                Ok(()) => return,
                Err(ApiError::Unavailable) if attempt < self.settings.attempts => {
                    tokio::select! {
                        _ = tokio::time::sleep(self.settings.retry_delay) => {}
                        _ = self.shutdown.cancelled() => return,
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "the site did not take the finish");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_address_keeps_its_inputs_in_the_fragment() {
        let url = page_url(4242, "a b+c/d", "rec1", "req-2");
        assert_eq!(
            url,
            "http://127.0.0.1:4242/render#api=/api/v4&token=a%20b%2Bc%2Fd&recording=rec1&request=req-2&sink=/__sink"
        );
        assert!(!url.split('#').next().unwrap().contains("token"));
    }

    #[test]
    fn ids_that_name_directories_are_plain() {
        assert!(plain_id("abc-DEF_123"));
        for bad in ["", "a/b", "..", "a b", "a.b", &"x".repeat(65)] {
            assert!(!plain_id(bad), "{bad:?}");
        }
    }
}
