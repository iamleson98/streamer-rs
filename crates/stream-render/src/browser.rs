//! What the job needs of a browser, so the job can be tested with a double that posts to the sink
//! the way the page does. The real implementation is `crate::chrome`.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;

use tokio::sync::watch;

/// Why the browser could not be used. The messages carry no page address and no token.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BrowserError {
    #[error("the browser could not be started")]
    Start,
    #[error("the browser is gone")]
    Gone,
    #[error("the browser did not answer in time")]
    Timeout,
    #[error("the browser's answer was not understood")]
    Protocol,
}

/// What the page shows the driver (`window.__render`).
#[derive(Debug, Clone, PartialEq)]
pub struct PageStatus {
    pub state: String,
    /// 0 to 1.
    pub progress: f64,
    /// Where the page spent its last stretch (ms per bucket, event counts); empty until it reports.
    pub perf: BTreeMap<String, f64>,
}

/// Most entries of the page's timing report that are kept, and the longest key.
const PERF_MAX_ENTRIES: usize = 24;
const PERF_MAX_KEY: usize = 24;

/// The numbers of the page's timing report (`perf`). The keys come from the page, so only short
/// ascii identifiers with a finite number as the value are kept; anything else is dropped.
pub fn parse_perf(value: &serde_json::Value) -> BTreeMap<String, f64> {
    let Some(map) = value.as_object() else {
        return BTreeMap::new();
    };
    map.iter()
        .filter(|(key, _)| {
            !key.is_empty()
                && key.len() <= PERF_MAX_KEY
                && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        })
        .filter_map(|(key, value)| {
            let n = value.as_f64().filter(|n| n.is_finite())?;
            Some((key.clone(), n))
        })
        .take(PERF_MAX_ENTRIES)
        .collect()
}

/// A timing report as one compact JSON line (`{"audio":12,"frames":150}`).
pub fn perf_line(perf: &BTreeMap<String, f64>) -> String {
    serde_json::to_string(perf).unwrap_or_default()
}

/// Starts browsers.
pub trait Browser: Send + Sync {
    type Session: BrowserSession;

    /// Opens `page_url` in a new browser that keeps its profile under `profile_dir`.
    fn launch(
        &self,
        page_url: &str,
        profile_dir: &Path,
    ) -> impl Future<Output = Result<Self::Session, BrowserError>> + Send;
}

/// One browser with the render page open.
pub trait BrowserSession: Send {
    /// Reads `window.__render`; `None` while the page has not set it up yet.
    fn status(&mut self) -> impl Future<Output = Result<Option<PageStatus>, BrowserError>> + Send;

    /// Hands the page a fresh render token (`window.__setRenderToken`).
    fn set_token(&mut self, token: &str) -> impl Future<Output = Result<(), BrowserError>> + Send;

    /// Turns true when the browser process has exited, for any reason.
    fn exited(&self) -> watch::Receiver<bool>;

    /// Kills the browser and everything it started, and waits until it is gone.
    fn close(self) -> impl Future<Output = ()> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_keeps_only_short_identifiers_with_numbers() {
        let perf = parse_perf(&json!({
            "screen_decode": 1200.0,
            "frames": 150,
            "bad key": 1,
            "Upper": 2,
            "text": "secret",
            "nan": null,
            "a_very_long_key_that_is_not_one_of_ours": 3
        }));
        assert_eq!(
            perf_line(&perf),
            r#"{"frames":150.0,"screen_decode":1200.0}"#
        );
    }

    #[test]
    fn it_reads_nothing_from_a_report_that_is_not_an_object() {
        assert!(parse_perf(&json!([1, 2])).is_empty());
        assert!(parse_perf(&serde_json::Value::Null).is_empty());
    }
}
