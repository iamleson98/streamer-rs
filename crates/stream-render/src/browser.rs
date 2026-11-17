//! What the job needs of a browser, so the job can be tested with a double that posts to the sink
//! the way the page does. The real implementation is `crate::chrome`.

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
