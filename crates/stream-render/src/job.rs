//! One render, from the claim to the finish: the engine makes the video while a heartbeat keeps the
//! lease, then the checks and the upload. Whatever happens, the engine's children are stopped and the
//! render's directory is deleted before the next claim.

use std::path::{Path, PathBuf};

use stream_common::api::{ApiClient, ApiError, RenderClaim, RenderFinishInput, UploadedPart};
use stream_common::redact::register_secret;
use tokio_util::sync::CancellationToken;

use crate::card::Card;
use crate::config::Settings;
use crate::engine::{RenderEngine, RenderJob, race};
use crate::heartbeat::{Heartbeat, HeartbeatTiming};
use crate::media;
use crate::upload::{self, UploadLimits};

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
}

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
pub struct Worker<E> {
    pub settings: Settings,
    pub api_base_url: String,
    pub api: ApiClient,
    pub storage: reqwest::Client,
    pub engine: E,
    pub shutdown: CancellationToken,
}

impl<E: RenderEngine> Worker<E> {
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
        let card = Card::from_claim(claim);
        let file_ms = card.file_ms(claim.duration_ms);
        let finished = self
            .engine
            .render(RenderJob {
                claim,
                card: &card,
                file_ms,
                dir,
                settings: &self.settings,
                api_base_url: &self.api_base_url,
                heartbeat,
                shutdown: &self.shutdown,
            })
            .await?;

        // From here on the file is made: the rest is checks and the upload, and the lease is kept
        // by the heartbeat.
        heartbeat.set_position(file_ms);
        let lost = heartbeat.lost();
        race(&self.shutdown, &lost, async {
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
            if !media::length_within_tolerance(rendered_ms, file_ms) {
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

    /// Tells the site how the render ended. A render the worker was told to leave (the site asked for a
    /// stop, or says it does not hold it, or was out of reach for the whole lease) is not reported: the
    /// site knows. One left for a shutdown is not either:
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
                tracing::info!("render dropped: the site no longer holds it");
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
    fn ids_that_name_directories_are_plain() {
        assert!(plain_id("abc-DEF_123"));
        for bad in ["", "a/b", "..", "a b", "a.b", &"x".repeat(65)] {
            assert!(!plain_id(bad), "{bad:?}");
        }
    }
}
