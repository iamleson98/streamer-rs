//! The loop: claim a stream, open its key, check the file, copy it to the platform with ffmpeg
//! while heartbeating, resume after a failure, and report how it ended.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use stream_common::api::{ApiClient, ApiError, FinishInput, StreamClaim};
use stream_common::codes::Failure;
use stream_common::destination::{Ingest, Platform};
use stream_common::key::{KeyError, RunnerPrivateKey, Secret};
use stream_common::media::is_allowed_media_url;
use stream_common::redact::{Redactor, register_secret};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
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
/// How much of the file may be missing at the end of a run that ffmpeg calls successful (the file's
/// length is the request's, to within the render's rounding).
const END_SLACK_MS: i64 = 5_000;
/// A line of ffmpeg's output is kept up to this long; the rest of it is dropped.
const MAX_LINE_BYTES: usize = 8 * 1024;

/// How a claimed stream ended.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The whole file was streamed.
    Done,
    /// The teacher asked to stop.
    Stopped,
    /// The site has given the request up (the lease ran out) or says it is not ours: it knows, so
    /// nothing is reported.
    Lost,
    /// The runner was asked to shut down before it started: the stream goes back to the site.
    Released,
    Failed(Failure),
}

enum AttemptEnd {
    Exited {
        success: bool,
        lines: Vec<String>,
    },
    /// ffmpeg made no progress, or ran far past the file's length, and was stopped.
    Stalled,
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
        // The platform's name comes from the site: logged quoted, so it cannot forge a line.
        tracing::info!(request_id = %claim.request_id, platform = ?claim.platform, "claimed a stream");
        let heartbeat = Heartbeat::start(
            self.client.clone(),
            &self.settings,
            claim.request_id.clone(),
            claim.duration_ms,
            claim.lease_until,
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
                tracing::info!(request_id = %claim.request_id, "left the stream; the site already knows");
                return;
            }
            Outcome::Released => {
                self.release(&claim.request_id).await;
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

    /// Reports how the stream ended. Once the runner is shutting down it is one short attempt: the
    /// process is about to be stopped, and the site's lease sweep covers a report that is lost.
    async fn finish(&self, request_id: &str, input: &FinishInput) {
        for attempt in 1..=FINISH_ATTEMPTS {
            let call = self.client.finish(request_id, input);
            let result = if self.shutdown.is_cancelled() {
                tokio::time::timeout(self.settings.shutdown_report_timeout, call)
                    .await
                    .unwrap_or(Err(ApiError::Unavailable))
            } else {
                call.await
            };
            match result {
                Ok(()) => return,
                // The site gave the request up or never had it: there is nothing to report to.
                Err(ApiError::Conflict | ApiError::NotFound) => {
                    tracing::info!(request_id, "the site no longer holds the stream");
                    return;
                }
                Err(ApiError::Unavailable)
                    if attempt < FINISH_ATTEMPTS && !self.shutdown.is_cancelled() =>
                {
                    tokio::time::sleep(self.settings.finish_retry_delay).await;
                }
                Err(error) => {
                    tracing::error!(%error, request_id, "could not report how the stream ended");
                    return;
                }
            }
        }
    }

    /// Gives a stream that was claimed but never started back to the site, once and quickly.
    async fn release(&self, request_id: &str) {
        let call = self.client.release(request_id, &self.settings.worker_id);
        match tokio::time::timeout(self.settings.shutdown_report_timeout, call).await {
            Ok(Ok(())) => tracing::info!(request_id, "gave the stream back before starting it"),
            Ok(Err(ApiError::Conflict | ApiError::NotFound)) => {
                tracing::info!(request_id, "the site no longer holds the stream");
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, request_id, "could not give the stream back; the site's lease sweep does");
            }
            Err(_) => {
                tracing::warn!(
                    request_id,
                    "giving the stream back took too long; the site's lease sweep does"
                );
            }
        }
    }

    /// The address ffmpeg sends to: the platform's own (a constant, or TikTok's checked server
    /// address) and the key.
    fn output_url(&self, platform: Platform, secret: &Secret) -> Result<Ingest, KeyError> {
        let ingest = platform.ingest(secret)?;
        #[cfg(feature = "test-ingest")]
        if let Some(base) = &self.settings.test_ingest_url {
            return Ok(Ingest {
                output_url: Secret::new(format!("{base}/{}", ingest.key.expose())),
                key: ingest.key,
            });
        }
        Ok(ingest)
    }

    async fn serve(&self, claim: &StreamClaim, heartbeat: &Heartbeat) -> Outcome {
        let Some(platform) = Platform::parse(&claim.platform) else {
            tracing::error!(platform = ?claim.platform, "a platform this runner does not know");
            return Outcome::Failed(Failure::StreamFailed);
        };
        let mp4_url = claim.mp4_url.expose();
        if !is_allowed_media_url(mp4_url, &self.settings.media_hosts) {
            tracing::error!(
                "the file's address is not https on an allowed media host; not opening it"
            );
            return Outcome::Failed(Failure::StreamFailed);
        }
        let secret = match self
            .key
            .open_secret(&claim.key_sealed, &claim.destination_id)
        {
            Ok(key) => key,
            // The key does not open with this runner's private key: the site was given another
            // public key than this runner holds the private half of. That is the operator's to fix,
            // not a refusal by the platform.
            Err(KeyError::Open) => {
                tracing::error!(
                    "the sealed stream key cannot be opened with this runner's private key: \
                     the site's runner public key and this runner's private key do not match"
                );
                return Outcome::Failed(Failure::StreamFailed);
            }
            Err(error) => {
                tracing::error!(%error, "the stream key could not be used");
                return Outcome::Failed(Failure::AuthRejected);
            }
        };
        let ingest = match self.output_url(platform, &secret) {
            Ok(ingest) => ingest,
            Err(error) => {
                tracing::error!(%error, "the stream key could not be used");
                return Outcome::Failed(Failure::AuthRejected);
            }
        };
        let output_url = ingest.output_url;
        // From here every line the process writes is cleaned of the key and the file's address (the
        // server's address is not a secret, and a log that names it says where a connection failed).
        let _key_guard = register_secret(ingest.key.expose());
        let _url_guard = register_secret(mp4_url);
        let mut redactor = Redactor::new();
        redactor.add(ingest.key.expose());
        redactor.add(mp4_url);

        let stopped = heartbeat.stopped();
        let gate = tokio::select! {
            () = self.shutdown.cancelled() => return Outcome::Released,
            result = self.check_file(mp4_url, claim.duration_ms) => result,
            () = stopped.cancelled() => return stop_outcome(heartbeat),
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
        let mut started = false;
        loop {
            if !started && self.shutdown.is_cancelled() {
                return Outcome::Released;
            }
            started = true;
            let args = ffmpeg::build_args(mp4_url, output_url, start_ms);
            let end = match self
                .attempt(&args, start_ms, duration_ms, &position, &stopped, redactor)
                .await
            {
                Ok(end) => end,
                Err(error) => {
                    tracing::error!(%error, "ffmpeg could not be started");
                    return Outcome::Failed(Failure::StreamFailed);
                }
            };
            let failure = match end {
                AttemptEnd::Interrupted(Interrupt::Heartbeat) => return stop_outcome(heartbeat),
                AttemptEnd::Interrupted(Interrupt::Shutdown) => {
                    return Outcome::Failed(Failure::StreamFailed);
                }
                AttemptEnd::Stalled => Failure::IngestUnreachable,
                AttemptEnd::Exited { success: true, .. } => {
                    // The storage or the network may have ended the file early without an error
                    // ffmpeg calls one: only the position says whether all of it was sent.
                    if position.load(Ordering::Relaxed) >= duration_ms - END_SLACK_MS {
                        return Outcome::Done;
                    }
                    tracing::warn!("ffmpeg ended before the end of the file");
                    Failure::IngestUnreachable
                }
                AttemptEnd::Exited {
                    success: false,
                    lines,
                } => Failure::classify(&lines),
            };

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
    /// when the heartbeat or the shutdown says so, or when it makes no progress for `stall_timeout`
    /// or runs `overrun_slack` past the length of what is left to send.
    async fn attempt(
        &self,
        args: &[String],
        start_ms: i64,
        duration_ms: i64,
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

        let remaining =
            Duration::from_millis(u64::try_from((duration_ms - start_ms).max(0)).unwrap_or(0));
        let waited = tokio::select! {
            status = child.wait() => Waited::Exited(status),
            () = stalled(position, self.settings.stall_timeout, remaining + self.settings.overrun_slack) => {
                tracing::warn!("ffmpeg is not making progress; stopping it");
                Waited::Stalled
            }
            () = stopped.cancelled() => Waited::Interrupted(Interrupt::Heartbeat),
            () = self.shutdown.cancelled() => Waited::Interrupted(Interrupt::Shutdown),
        };
        let (status, cut_short) = match waited {
            Waited::Exited(status) => (status, None),
            Waited::Stalled => (self.stop_child(&mut child).await, Some(None)),
            Waited::Interrupted(why) => (self.stop_child(&mut child).await, Some(Some(why))),
        };
        // The readers end with the process's pipes.
        let _ = progress.await;
        let lines = errors.await.unwrap_or_default();
        match cut_short {
            Some(Some(why)) => Ok(AttemptEnd::Interrupted(why)),
            Some(None) => Ok(AttemptEnd::Stalled),
            None => Ok(AttemptEnd::Exited {
                success: status?.success(),
                lines,
            }),
        }
    }

    /// Interrupts ffmpeg, and kills it when it does not end within `stop_grace`.
    async fn stop_child(
        &self,
        child: &mut tokio::process::Child,
    ) -> std::io::Result<std::process::ExitStatus> {
        ffmpeg::interrupt(child);
        match tokio::time::timeout(self.settings.stop_grace, child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                tracing::warn!("ffmpeg did not end after the interrupt; killing it");
                child.kill().await.and(child.wait().await)
            }
        }
    }
}

enum Waited {
    Exited(std::io::Result<std::process::ExitStatus>),
    Stalled,
    Interrupted(Interrupt),
}

/// Resolves when `position` has not moved for `stall`, or `overall` has passed.
async fn stalled(position: &AtomicI64, stall: Duration, overall: Duration) {
    let began = tokio::time::Instant::now();
    let mut last = position.load(Ordering::Relaxed);
    let mut last_moved = began;
    let every = (stall / 4).clamp(Duration::from_millis(20), Duration::from_secs(1));
    loop {
        tokio::time::sleep(every).await;
        let now = tokio::time::Instant::now();
        let at = position.load(Ordering::Relaxed);
        if at > last {
            last = at;
            last_moved = now;
        }
        if now.duration_since(last_moved) >= stall || now.duration_since(began) >= overall {
            return;
        }
    }
}

fn stop_outcome(heartbeat: &Heartbeat) -> Outcome {
    match heartbeat.reason() {
        Some(StopReason::LeaseLost | StopReason::NotHeld) => Outcome::Lost,
        _ => Outcome::Stopped,
    }
}

/// Reads one line of `reader` without its newline, as text (bytes that are not UTF-8 are replaced),
/// keeping at most `MAX_LINE_BYTES` of it. It reads on to the end of the line whatever its length and
/// returns `None` at the end of the input, so the pipe is always emptied: a full pipe would block
/// ffmpeg.
async fn next_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut Vec<u8>,
) -> std::io::Result<Option<String>> {
    line.clear();
    let mut any = false;
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            break;
        }
        any = true;
        let newline = chunk.iter().position(|b| *b == b'\n');
        let take = newline.unwrap_or(chunk.len());
        let room = MAX_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..take.min(room)]);
        reader.consume(newline.map_or(take, |i| i + 1));
        if newline.is_some() {
            return Ok(Some(String::from_utf8_lossy(line).into_owned()));
        }
    }
    Ok(any.then(|| String::from_utf8_lossy(line).into_owned()))
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
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        while let Ok(Some(line)) = next_line(&mut reader, &mut buf).await {
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
        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::new();
        while let Ok(Some(line)) = next_line(&mut reader, &mut buf).await {
            tracing::warn!(line = ?redactor.redact(&line), "ffmpeg");
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

    async fn lines_of(input: &[u8]) -> Vec<String> {
        let mut reader = BufReader::with_capacity(16, input);
        let mut buf = Vec::new();
        let mut lines = Vec::new();
        while let Some(line) = next_line(&mut reader, &mut buf).await.unwrap() {
            lines.push(line);
        }
        lines
    }

    #[tokio::test]
    async fn lines_that_are_not_utf8_do_not_end_the_reading() {
        let lines = lines_of(b"one\n\xff\xfe broken \xc3\n\nlast, no newline").await;
        assert_eq!(
            lines,
            [
                "one",
                "\u{fffd}\u{fffd} broken \u{fffd}",
                "",
                "last, no newline"
            ]
        );
    }

    #[tokio::test]
    async fn a_very_long_line_is_cut_and_the_rest_is_still_read() {
        let mut input = vec![b'x'; MAX_LINE_BYTES * 3 + 5];
        input.extend_from_slice(b"\nnext\n");
        let lines = lines_of(&input).await;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines[1], "next");
    }

    #[tokio::test(start_paused = true)]
    async fn a_position_that_stops_moving_is_a_stall() {
        let position = AtomicI64::new(0);
        let began = tokio::time::Instant::now();
        stalled(&position, Duration::from_secs(8), Duration::from_secs(3600)).await;
        assert_eq!(began.elapsed(), Duration::from_secs(8));
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_that_outlasts_the_file_is_stopped_even_while_it_moves() {
        let position = Arc::new(AtomicI64::new(0));
        let mover = position.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                mover.fetch_add(100, Ordering::Relaxed);
            }
        });
        let began = tokio::time::Instant::now();
        stalled(&position, Duration::from_secs(60), Duration::from_secs(30)).await;
        assert!(
            began.elapsed() >= Duration::from_secs(30) && began.elapsed() < Duration::from_secs(32)
        );
        task.abort();
    }
}
