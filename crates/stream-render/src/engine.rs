//! The seam between the worker and what makes the video. The worker claims a render, keeps its
//! lease, probes and uploads the file and tells the site how it ended; an engine only turns the
//! claimed recording into one MP4 in the render's directory.

use std::path::{Path, PathBuf};

use stream_common::api::RenderClaim;
use tokio_util::sync::CancellationToken;

use crate::card::Card;
use crate::config::Settings;
use crate::heartbeat::Heartbeat;
use crate::job::Ended;

/// Everything an engine is given for one render.
#[derive(Debug)]
pub struct RenderJob<'a> {
    pub claim: &'a RenderClaim,
    /// The countdown card, already bounded.
    pub card: &'a Card,
    /// The whole file's length: the card, then the lesson.
    pub file_ms: i64,
    /// The render's own directory, created, and deleted by the worker afterwards.
    pub dir: &'a Path,
    pub settings: &'a Settings,
    pub api_base_url: &'a str,
    /// The lease: fresh render tokens, the lost-lease signal and the progress to report.
    pub heartbeat: &'a Heartbeat,
    pub shutdown: &'a CancellationToken,
}

/// Makes the video of one claimed recording.
pub trait RenderEngine: Send + Sync {
    /// Renders into `job.dir` and returns the finished file: H.264 (Main, at most 1280x720) with
    /// AAC, the moov atom first, a keyframe at least every 2 seconds. The worker probes and
    /// uploads it. An engine ends with `Ended::Failed` only for the failure codes the site knows,
    /// and stops with `Ended::Shutdown` or `Ended::LeaseLost` when told to.
    fn render(&self, job: RenderJob<'_>) -> impl Future<Output = Result<PathBuf, Ended>> + Send;
}

/// Runs `work` unless the worker is told to shut down or the lease is lost first.
pub async fn race<T>(
    shutdown: &CancellationToken,
    lost: &CancellationToken,
    work: impl Future<Output = Result<T, Ended>>,
) -> Result<T, Ended> {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err(Ended::Shutdown),
        _ = lost.cancelled() => Err(Ended::LeaseLost),
        result = work => result,
    }
}
