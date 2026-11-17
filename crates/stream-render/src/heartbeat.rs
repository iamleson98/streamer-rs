//! The lease: while a render is held, the worker tells the site every 15 seconds how far it is. The
//! answer says whether to go on, and brings a fresh render token (the one the page reads the
//! recording with lives ten minutes). The task runs from the claim to the finish, so the lease is
//! kept through the remux and the upload too.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use stream_common::api::{ApiClient, ApiError, HeartbeatInput};
use stream_common::key::Secret;
use stream_common::redact::{Registered, register_secret};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

/// How many render tokens stay registered with the log filter.
const KEPT_TOKENS: usize = 4;

/// Why the worker no longer holds the render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseLost {
    /// The site said stop (the request was cancelled) or does not hold the request for this worker.
    Stopped,
    /// The site could not be reached for as long as the lease lasts.
    Expired,
}

#[derive(Debug, Clone, Copy)]
pub struct HeartbeatTiming {
    pub interval: Duration,
    pub lease_timeout: Duration,
}

/// A running heartbeat.
#[derive(Debug)]
pub struct Heartbeat {
    position: Arc<AtomicI64>,
    /// Cancelled when the lease is lost.
    lost: CancellationToken,
    reason: Arc<std::sync::Mutex<Option<LeaseLost>>>,
    tokens: watch::Receiver<Option<Secret>>,
    end: CancellationToken,
    task: JoinHandle<()>,
}

impl Heartbeat {
    pub fn start(
        api: ApiClient,
        worker_id: String,
        request_id: String,
        timing: HeartbeatTiming,
    ) -> Self {
        let position = Arc::new(AtomicI64::new(0));
        let lost = CancellationToken::new();
        let reason = Arc::new(std::sync::Mutex::new(None));
        let (token_tx, tokens) = watch::channel(None);
        let end = CancellationToken::new();
        let task = tokio::spawn(beat(
            api,
            worker_id,
            request_id,
            timing,
            position.clone(),
            lost.clone(),
            reason.clone(),
            token_tx,
            end.clone(),
        ));
        Self {
            position,
            lost,
            reason,
            tokens,
            end,
            task,
        }
    }

    /// Where the render is, in video milliseconds, for the next beat.
    pub fn set_position(&self, ms: i64) {
        self.position.store(ms.max(0), Ordering::Relaxed);
    }

    /// Cancelled when the lease is lost.
    pub fn lost(&self) -> CancellationToken {
        self.lost.clone()
    }

    pub fn reason(&self) -> Option<LeaseLost> {
        *self.reason.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Fresh render tokens as the site sends them.
    pub fn tokens(&self) -> watch::Receiver<Option<Secret>> {
        self.tokens.clone()
    }

    /// Stops beating and waits for the task.
    pub async fn finish(self) {
        self.end.cancel();
        let _ = self.task.await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn beat(
    api: ApiClient,
    worker_id: String,
    request_id: String,
    timing: HeartbeatTiming,
    position: Arc<AtomicI64>,
    lost: CancellationToken,
    reason: Arc<std::sync::Mutex<Option<LeaseLost>>>,
    tokens: watch::Sender<Option<Secret>>,
    end: CancellationToken,
) {
    let give_up = |why: LeaseLost| {
        *reason.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        lost.cancel();
    };
    let mut registered: Vec<Registered> = Vec::new();
    let mut last_ok = Instant::now();
    let mut ticker = tokio::time::interval_at(Instant::now() + timing.interval, timing.interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = end.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let input = HeartbeatInput {
            worker_id: worker_id.clone(),
            position_ms: position.load(Ordering::Relaxed),
        };
        match api.render_heartbeat(&request_id, &input).await {
            Ok(answer) if answer.stop => {
                tracing::warn!("the site asked the render to stop");
                give_up(LeaseLost::Stopped);
                return;
            }
            Ok(answer) => {
                last_ok = Instant::now();
                if let Some(token) = answer.render_token {
                    registered.push(register_secret(token.expose()));
                    // Older tokens are dead; the filter only needs the last few.
                    if registered.len() > KEPT_TOKENS {
                        registered.remove(0);
                    }
                    let _ = tokens.send(Some(token));
                }
            }
            Err(ApiError::Unauthorized | ApiError::NotFound | ApiError::Rejected(_)) => {
                tracing::warn!("the site no longer holds the render for this worker");
                give_up(LeaseLost::Stopped);
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "heartbeat failed");
                if last_ok.elapsed() >= timing.lease_timeout {
                    tracing::error!("no heartbeat reached the site for the length of the lease");
                    give_up(LeaseLost::Expired);
                    return;
                }
            }
        }
    }
}
