//! The heartbeat of a claimed stream, kept for as long as the runner holds it: it reports the
//! position, learns when the teacher asked to stop, and gives up in time before the site's lease on
//! the request ends (the site gives the request up then, so streaming on would be streaming a
//! request that is no longer ours).

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use stream_common::api::{ApiClient, ApiError, HeartbeatInput};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval, sleep_until};
use tokio_util::sync::CancellationToken;

use crate::config::Settings;

/// A lease further away than this is not believed (a clock far off the site's would otherwise keep a
/// stream going for as long as it likes).
const MAX_LEASE: Duration = Duration::from_secs(600);

/// Why the heartbeat ended the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The teacher asked to stop (the site said `stop`).
    Requested,
    /// The site says the request is not the runner's (any more): it knows already, so nothing is
    /// reported to it.
    NotHeld,
    /// The site was not reached before the lease ran out.
    LeaseLost,
}

#[derive(Debug)]
pub struct Heartbeat {
    position_ms: Arc<AtomicI64>,
    stopped: CancellationToken,
    reason: Arc<Mutex<Option<StopReason>>>,
    task: JoinHandle<()>,
}

impl Heartbeat {
    /// Starts reporting; the first heartbeat goes out at once. `lease_until_ms` is the lease the
    /// claim came with.
    pub fn start(
        client: ApiClient,
        settings: &Settings,
        request_id: String,
        duration_ms: i64,
        lease_until_ms: i64,
    ) -> Self {
        let position_ms = Arc::new(AtomicI64::new(0));
        let stopped = CancellationToken::new();
        let reason = Arc::new(Mutex::new(None));
        let task = tokio::spawn(beat(
            client,
            settings.clone(),
            Beat {
                request_id,
                duration_ms,
                lease_until_ms,
            },
            position_ms.clone(),
            stopped.clone(),
            reason.clone(),
        ));
        Self {
            position_ms,
            stopped,
            reason,
            task,
        }
    }

    /// Where the stream is, in video milliseconds; the streaming side writes it.
    pub fn position(&self) -> Arc<AtomicI64> {
        self.position_ms.clone()
    }

    /// Cancelled when the stream must end; `reason` says why.
    pub fn stopped(&self) -> CancellationToken {
        self.stopped.clone()
    }

    pub fn reason(&self) -> Option<StopReason> {
        *self.reason.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Beat {
    request_id: String,
    duration_ms: i64,
    lease_until_ms: i64,
}

fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// When the stream must be stopped for the lease `lease_until_ms` (epoch ms, the site's word): early
/// enough that a call that is in flight (`request_timeout`) and ffmpeg's stop (`stop_grace`) both end
/// before the site gives the request up, but not before the next heartbeat can renew it. A lease
/// that is not in the future by this host's clock (none given, or the clocks differ) falls back to
/// `lease_timeout` after `now`.
fn stop_deadline(lease_until_ms: i64, now_ms: i64, now: Instant, settings: &Settings) -> Instant {
    let remaining = lease_until_ms.saturating_sub(now_ms);
    if remaining <= 0 {
        return now + settings.lease_timeout;
    }
    let remaining = Duration::from_millis(remaining.unsigned_abs()).min(MAX_LEASE);
    let margin = settings.request_timeout + settings.stop_grace;
    now + remaining
        .saturating_sub(margin)
        .max(settings.heartbeat_interval)
}

async fn beat(
    client: ApiClient,
    settings: Settings,
    claim: Beat,
    position_ms: Arc<AtomicI64>,
    stopped: CancellationToken,
    reason: Arc<Mutex<Option<StopReason>>>,
) {
    let Beat {
        request_id,
        duration_ms,
        lease_until_ms,
    } = claim;
    let stop = |why| {
        *reason.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        stopped.cancel();
    };
    let mut ticks = interval(settings.heartbeat_interval);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The claim itself counts as a heartbeat.
    let mut deadline = stop_deadline(lease_until_ms, epoch_ms(), Instant::now(), &settings);
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = sleep_until(deadline.into()) => {
                tracing::error!(
                    request_id,
                    "no heartbeat got through before the lease ran out; stopping the stream"
                );
                return stop(StopReason::LeaseLost);
            }
        }
        let input = HeartbeatInput {
            worker_id: settings.worker_id.clone(),
            position_ms: position_ms
                .load(Ordering::Relaxed)
                .clamp(0, duration_ms.max(0)),
        };
        match client.heartbeat(&request_id, &input).await {
            Ok(answer) if answer.stop => return stop(StopReason::Requested),
            Ok(answer) => {
                deadline = stop_deadline(answer.lease_until, epoch_ms(), Instant::now(), &settings);
            }
            // The site does not know the request, or says the runner does not hold it: it is no
            // longer ours.
            Err(ApiError::NotFound | ApiError::Conflict) => return stop(StopReason::NotHeld),
            Err(error) => tracing::warn!(%error, request_id, "heartbeat failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        let mut s = Settings::new("w");
        s.request_timeout = Duration::from_secs(10);
        s.stop_grace = Duration::from_secs(10);
        s.heartbeat_interval = Duration::from_secs(15);
        s.lease_timeout = Duration::from_secs(45);
        s
    }

    #[test]
    fn the_deadline_is_the_lease_less_what_a_stop_needs() {
        let now = Instant::now();
        let at = stop_deadline(1_000_000 + 45_000, 1_000_000, now, &settings());
        assert_eq!(at - now, Duration::from_secs(25));
    }

    #[test]
    fn a_short_lease_still_leaves_time_for_the_next_heartbeat() {
        let now = Instant::now();
        let at = stop_deadline(1_000_000 + 12_000, 1_000_000, now, &settings());
        assert_eq!(at - now, Duration::from_secs(15));
    }

    #[test]
    fn a_lease_in_the_past_or_absent_falls_back_to_the_fixed_timeout() {
        let now = Instant::now();
        for lease in [0, 1, 999_999, 1_000_000] {
            let at = stop_deadline(lease, 1_000_000, now, &settings());
            assert_eq!(at - now, Duration::from_secs(45), "{lease}");
        }
    }

    #[test]
    fn a_lease_far_in_the_future_is_capped() {
        let now = Instant::now();
        let at = stop_deadline(i64::MAX, 1_000_000, now, &settings());
        assert_eq!(at - now, MAX_LEASE - Duration::from_secs(20));
    }
}
