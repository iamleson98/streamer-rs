//! The loop: ask the site for a render every few seconds, run it, ask again. One render at a time.

use std::time::Duration;

use crate::browser::Browser;
use crate::job::{Worker, clean_work_dir};

/// The longest wait between claims after failed ones.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// How long to wait after `failures` failed claims in a row: the poll interval, doubled each time,
/// up to a minute.
pub fn backoff(poll: Duration, failures: u32) -> Duration {
    poll.saturating_mul(1u32 << failures.min(10))
        .min(MAX_BACKOFF.max(poll))
}

impl<B: Browser> Worker<B> {
    /// Runs until the shutdown token is cancelled.
    pub async fn run(&self) {
        clean_work_dir(&self.settings.work_dir).await;
        let mut failures: u32 = 0;
        while !self.shutdown.is_cancelled() {
            let wait = match self.api.render_claim(&self.settings.worker_id).await {
                Ok(Some(claim)) => {
                    failures = 0;
                    self.render(claim).await;
                    continue;
                }
                Ok(None) => {
                    failures = 0;
                    self.settings.poll_interval
                }
                Err(error) => {
                    tracing::warn!(%error, "claim failed");
                    failures = failures.saturating_add(1);
                    backoff(self.settings.poll_interval, failures - 1)
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.shutdown.cancelled() => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_between_failed_claims_doubles_up_to_a_minute() {
        let poll = Duration::from_secs(5);
        let waits: Vec<u64> = (0..6).map(|n| backoff(poll, n).as_secs()).collect();
        assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
        assert_eq!(backoff(poll, 1000), MAX_BACKOFF);
    }
}
