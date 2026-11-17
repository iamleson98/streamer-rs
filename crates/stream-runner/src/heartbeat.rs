//! The heartbeat of a claimed stream, kept for as long as the runner holds it: it reports the
//! position, learns when the teacher asked to stop, and gives up when the site has not answered for
//! the whole lease (the site gives the request up by then, so streaming on would be streaming a
//! request that is no longer ours).

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use stream_common::api::{ApiClient, ApiError, HeartbeatInput};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;

use crate::config::Settings;

/// Why the heartbeat ended the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The site said stop: the teacher cancelled, or the request is no longer the runner's.
    Requested,
    /// The site was not reached for the whole lease.
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
    /// Starts reporting; the first heartbeat goes out at once.
    pub fn start(
        client: ApiClient,
        settings: &Settings,
        request_id: String,
        duration_ms: i64,
    ) -> Self {
        let position_ms = Arc::new(AtomicI64::new(0));
        let stopped = CancellationToken::new();
        let reason = Arc::new(Mutex::new(None));
        let task = tokio::spawn(beat(
            client,
            settings.clone(),
            request_id,
            duration_ms,
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

async fn beat(
    client: ApiClient,
    settings: Settings,
    request_id: String,
    duration_ms: i64,
    position_ms: Arc<AtomicI64>,
    stopped: CancellationToken,
    reason: Arc<Mutex<Option<StopReason>>>,
) {
    let stop = |why| {
        *reason.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        stopped.cancel();
    };
    let mut ticks = interval(settings.heartbeat_interval);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The claim itself counts as a heartbeat.
    let mut last_ok = Instant::now();
    loop {
        ticks.tick().await;
        let input = HeartbeatInput {
            worker_id: settings.worker_id.clone(),
            position_ms: position_ms
                .load(Ordering::Relaxed)
                .clamp(0, duration_ms.max(0)),
        };
        match client.heartbeat(&request_id, &input).await {
            Ok(answer) if answer.stop => return stop(StopReason::Requested),
            Ok(_) => last_ok = Instant::now(),
            // The site does not know the request: it is no longer ours.
            Err(ApiError::NotFound) => return stop(StopReason::Requested),
            Err(error) => tracing::warn!(%error, request_id, "heartbeat failed"),
        }
        if last_ok.elapsed() >= settings.lease_timeout {
            tracing::error!(
                request_id,
                "no heartbeat got through for the whole lease; stopping the stream"
            );
            return stop(StopReason::LeaseLost);
        }
    }
}
