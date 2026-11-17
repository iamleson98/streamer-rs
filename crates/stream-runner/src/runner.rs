//! The loop: claim a stream, open its key, check the file, copy it to the platform with ffmpeg
//! while heartbeating, resume after a failure, and report how it ended.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use stream_common::api::{ApiClient, ApiError, FinishInput, StreamClaim};
use stream_common::codes::Failure;
use stream_common::destination::Platform;
use stream_common::key::RunnerPrivateKey;
use stream_common::redact::{Redactor, register_secret};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Settings;
use crate::ffmpeg;
use crate::heartbeat::{Heartbeat, StopReason};
use crate::probe;

/// The longest wait between claims after the site failed to answer.
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How many of ffmpeg's last error lines are kept to tell what went wrong.
const KEPT_STDERR_LINES: usize = 20;
/// How many times a claim is finished before the runner lets the site's lease sweep take over.
const FINISH_ATTEMPTS: usize = 3;

/// How a claimed stream ended.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The whole file was streamed.
    Done,
    /// The site asked to stop.
    Stopped,
    /// The site could not be reached for the whole lease; the site has given the request up.
    Lost,
    Failed(Failure),
}

enum AttemptEnd {
    Exited { success: bool, lines: Vec<String> },
    Interrupted(Interrupt),
}

enum Interrupt {
    Heartbeat,
    Shutdown,
}

#[derive(Debug)]
pub struct Runner {
    settings: Settings,
    client: ApiClient,
    key: RunnerPrivateKey,
    shutdown: CancellationToken,
}

impl Runner {
    pub fn new(
        settings: Settings,
        client: ApiClient,
        key: RunnerPrivateKey,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            settings,
            client,
            key,
            shutdown,
        }
    }

    /// Claims and streams until `shutdown` is cancelled.
    pub async fn run(&self) {
        let mut failures = 0u32;
        while !self.shutdown.is_cancelled() {
            match self.client.claim(&self.settings.worker_id).await {
                Ok(Some(claim)) => {
                    failures = 0;
                    self.handle(claim).await;
                }
                Ok(None) => {
                    failures = 0;
                    self.sleep(self.settings.poll_interval).await;
                }
                Err(error) => {
                    failures = failures.saturating_add(1);
                    let wait = if matches!(error, ApiError::Unauthorized) {
                        MAX_BACKOFF
                    } else {
                        self.backoff(failures)
                    };
                    tracing::warn!(%error, wait_secs = wait.as_secs(), "claim failed");
                    self.sleep(wait).await;
                }
            }
        }
    }

    fn backoff(&self, failures: u32) -> Duration {
        let factor = 1u32 << failures.saturating_sub(1).min(6);
        (self.settings.poll_interval * factor).min(MAX_BACKOFF)
    }

    /// Sleeps unless the runner is asked to shut down first.
    async fn sleep(&self, wait: Duration) {
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = self.shutdown.cancelled() => {}
        }
    }

    async fn handle(&self, claim: StreamClaim) {
        tracing::info!(request_id = %claim.request_id, platform = %claim.platform, "claimed a stream");
        let heartbeat = Heartbeat::start(
            self.client.clone(),
            &self.settings,
            claim.request_id.clone(),
            claim.duration_ms,
        );
        let outcome = self.serve(&claim, &heartbeat).await;
        let streamed_ms = heartbeat
            .position()
            .load(Ordering::Relaxed)
            .clamp(0, claim.duration_ms.max(0));
        drop(heartbeat);

        let (failure_code, streamed_ms) = match outcome {
            Outcome::Done => (None, claim.duration_ms.max(0)),
            Outcome::Stopped => (None, streamed_ms),
            Outcome::Failed(failure) => (Some(failure.wire_code().to_owned()), streamed_ms),
            Outcome::Lost => {
                tracing::warn!(request_id = %claim.request_id, "left the stream; the site's lease sweep ends it");
                return;
            }
        };
        match &outcome {
            Outcome::Failed(failure) => {
                tracing::warn!(request_id = %claim.request_id, failure = failure.name(), streamed_ms, "stream failed");
            }
            other => {
                tracing::info!(request_id = %claim.request_id, ?other, streamed_ms, "stream ended")
            }
        }
        let input = FinishInput {
            worker_id: self.settings.worker_id.clone(),
            failure_code,
            streamed_ms,
        };
        self.finish(&claim.request_id, &input).await;
    }

    async fn finish(&self, request_id: &str, input: &FinishInput) {
        for attempt in 1..=FINISH_ATTEMPTS {
            match self.client.finish(request_id, input).await {
                Ok(()) => return,
                Err(ApiError::Unavailable) if attempt < FINISH_ATTEMPTS => {
                    tokio::time::sleep(self.settings.finish_retry_delay).await;
                }
                Err(error) => {
                    tracing::error!(%error, request_id, "could not report how the stream ended");
                    return;
                }
            }
        }
    }

    async fn serve(&self, claim: &StreamClaim, heartbeat: &Heartbeat) -> Outcome {
        let Some(platform) = Platform::parse(&claim.platform) else {
            tracing::error!(platform = %claim.platform, "a platform this runner does not know");
            return Outcome::Failed(Failure::StreamFailed);
        };
        let key = match self.key.open(&claim.key_sealed, &claim.destination_id) {
            Ok(key) => key,
            Err(error) => {
                tracing::error!(%error, "the stream key could not be used");
                return Outcome::Failed(Failure::AuthRejected);
            }
        };
        let output_url = platform.output_url(&key);
        let mp4_url = claim.mp4_url.expose();
        // From here every line the process writes is cleaned of both.
        let _key_guard = register_secret(key.expose());
        let _url_guard = register_secret(mp4_url);
        let mut redactor = Redactor::new();
        redactor.add(key.expose());
        redactor.add(mp4_url);

        let stopped = heartbeat.stopped();
        let gate = tokio::select! {
            result = self.check_file(mp4_url, claim.duration_ms) => result,
            () = stopped.cancelled() => return stop_outcome(heartbeat),
            () = self.shutdown.cancelled() => return Outcome::Failed(Failure::StreamFailed),
        };
        if let Err(failure) = gate {
            tracing::warn!(failure = failure.name(), "the file is not fit to stream");
            return Outcome::Failed(failure);
        }
        self.copy_to_ingest(
            mp4_url,
            output_url.expose(),
            claim.duration_ms,
            heartbeat,
            &redactor,
        )
        .await
    }

    /// ffprobe, tried twice: a file that cannot be read once may be read a moment later, a missing
    /// or unfit one is not.
    async fn check_file(&self, mp4_url: &str, duration_ms: i64) -> Result<(), Failure> {
        let mut last = Failure::InputError;
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(self.settings.probe_retry_delay).await;
            }
            match probe::probe(&self.settings.ffprobe, mp4_url, self.settings.probe_timeout).await {
                Ok(output) => return output.check(duration_ms),
                Err(Failure::FileMissing) => return Err(Failure::FileMissing),
                Err(failure) => last = failure,
            }
        }
        Err(last)
    }

    async fn copy_to_ingest(
        &self,
        mp4_url: &str,
        output_url: &str,
        duration_ms: i64,
        heartbeat: &Heartbeat,
        redactor: &Redactor,
    ) -> Outcome {
        let position = heartbeat.position();
        let stopped = heartbeat.stopped();
        let mut failures: Vec<Instant> = Vec::new();
        let mut start_ms = 0;
        loop {
            let args = ffmpeg::build_args(mp4_url, output_url, start_ms);
            let end = match self
                .attempt(&args, start_ms, &position, &stopped, redactor)
                .await
            {
                Ok(end) => end,
                Err(error) => {
                    tracing::error!(%error, "ffmpeg could not be started");
                    return Outcome::Failed(Failure::StreamFailed);
                }
            };
            let lines = match end {
                AttemptEnd::Interrupted(Interrupt::Heartbeat) => return stop_outcome(heartbeat),
                AttemptEnd::Interrupted(Interrupt::Shutdown) => {
                    return Outcome::Failed(Failure::StreamFailed);
                }
                AttemptEnd::Exited { success: true, .. } => {
                    position.store(duration_ms, Ordering::Relaxed);
                    return Outcome::Done;
                }
                AttemptEnd::Exited {
                    success: false,
                    lines,
                } => lines,
            };

            let failure = Failure::classify(&lines);
            let now = Instant::now();
            failures.retain(|at| now.duration_since(*at) < self.settings.resume_window);
            failures.push(now);
            if !failure.is_resumable() || failures.len() > self.settings.max_resumes {
                return Outcome::Failed(failure);
            }
            start_ms = position.load(Ordering::Relaxed).max(0);
            tracing::warn!(
                failure = failure.name(),
                resume_at_ms = start_ms,
                "ffmpeg failed; resuming"
            );
            tokio::select! {
                () = tokio::time::sleep(self.settings.resume_delay) => {}
                () = stopped.cancelled() => return stop_outcome(heartbeat),
                () = self.shutdown.cancelled() => return Outcome::Failed(Failure::StreamFailed),
            }
        }
    }

    /// One ffmpeg run. It ends when ffmpeg exits, or is interrupted (and given `stop_grace` to end)
    /// when the heartbeat or the shutdown says so.
    async fn attempt(
        &self,
        args: &[String],
        start_ms: i64,
        position: &Arc<AtomicI64>,
        stopped: &CancellationToken,
        redactor: &Redactor,
    ) -> std::io::Result<AttemptEnd> {
        let mut child = Command::new(&self.settings.ffmpeg)
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let progress = spawn_progress_reader(child.stdout.take(), start_ms, position.clone());
        let errors = spawn_error_reader(child.stderr.take(), redactor.clone());

        let waited = tokio::select! {
            status = child.wait() => Ok(status),
            why = async {
                tokio::select! {
                    () = stopped.cancelled() => Interrupt::Heartbeat,
                    () = self.shutdown.cancelled() => Interrupt::Shutdown,
                }
            } => Err(why),
        };
        let (status, interrupted) = match waited {
            Ok(status) => (status, None),
            Err(why) => {
                ffmpeg::interrupt(&child);
                let status =
                    match tokio::time::timeout(self.settings.stop_grace, child.wait()).await {
                        Ok(status) => status,
                        Err(_) => {
                            tracing::warn!("ffmpeg did not end after the interrupt; killing it");
                            child.kill().await.and(child.wait().await)
                        }
                    };
                (status, Some(why))
            }
        };
        // The readers end with the process's pipes.
        let _ = progress.await;
        let lines = errors.await.unwrap_or_default();
        if let Some(why) = interrupted {
            return Ok(AttemptEnd::Interrupted(why));
        }
        Ok(AttemptEnd::Exited {
            success: status?.success(),
            lines,
        })
    }
}

fn stop_outcome(heartbeat: &Heartbeat) -> Outcome {
    match heartbeat.reason() {
        Some(StopReason::LeaseLost) => Outcome::Lost,
        _ => Outcome::Stopped,
    }
}

/// Reads `-progress` and keeps `position` at the video time reached (`start_ms` is where this run
/// began in the file; ffmpeg counts from zero after a seek).
fn spawn_progress_reader(
    stdout: Option<tokio::process::ChildStdout>,
    start_ms: i64,
    position: Arc<AtomicI64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Some(stdout) = stdout else { return };
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(ms) = ffmpeg::parse_progress_line(&line) {
                position.store(start_ms + ms, Ordering::Relaxed);
            }
        }
    })
}

/// Logs ffmpeg's error lines (redacted) and returns the last few as read, for `Failure::classify`;
/// they are not logged or sent as read.
fn spawn_error_reader(
    stderr: Option<tokio::process::ChildStderr>,
    redactor: Redactor,
) -> JoinHandle<Vec<String>> {
    tokio::spawn(async move {
        let mut kept: Vec<String> = Vec::new();
        let Some(stderr) = stderr else { return kept };
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::warn!(line = %redactor.redact(&line), "ffmpeg");
            if kept.len() == KEPT_STDERR_LINES {
                kept.remove(0);
            }
            kept.push(line);
        }
        kept
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_between_failed_claims_doubles_up_to_a_minute() {
        let settings = Settings::new("w");
        let client = ApiClient::new(
            "http://127.0.0.1:1",
            stream_common::key::Secret::new("t"),
            Duration::from_secs(1),
        )
        .unwrap();
        let key = stream_common::key::RunnerPrivateKey::from_key(
            aws_lc_rs::rsa::PrivateDecryptingKey::generate(aws_lc_rs::rsa::KeySize::Rsa2048)
                .unwrap(),
        )
        .unwrap();
        let runner = Runner::new(settings, client, key, CancellationToken::new());
        let waits: Vec<u64> = (1..=6).map(|n| runner.backoff(n).as_secs()).collect();
        assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
        assert_eq!(runner.backoff(u32::MAX).as_secs(), 60);
    }
}
