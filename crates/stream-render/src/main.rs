use std::process::ExitCode;
use std::time::Duration;

use stream_common::api::ApiClient;
use stream_common::redact::init_logging;
use stream_render::chrome::Chrome;
use stream_render::chromium::ChromiumEngine;
use stream_render::config::Config;
use stream_render::job::Worker;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();
    let config = match Config::load(|name| std::env::var(name).ok()) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "refusing to start");
            return ExitCode::from(2);
        }
    };
    let api = match ApiClient::new(
        &config.api_base_url,
        config.token,
        config.settings.request_timeout,
    ) {
        Ok(api) => api,
        Err(error) => {
            tracing::error!(%error, "refusing to start");
            return ExitCode::from(2);
        }
    };
    // Storage gets presigned addresses and no token; redirects are not followed.
    let storage = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            tracing::error!("cannot build the storage client");
            return ExitCode::from(2);
        }
    };

    let shutdown = CancellationToken::new();
    let (mut term, mut int) = match (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) {
        (Ok(term), Ok(int)) => (term, int),
        _ => {
            tracing::error!("cannot listen for signals");
            return ExitCode::from(2);
        }
    };
    let on_signal = shutdown.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        tracing::info!("shutting down");
        on_signal.cancel();
    });

    tracing::info!(worker_id = %config.settings.worker_id, "render worker started");
    let engine = ChromiumEngine::new(Chrome::new(
        config.settings.browser.clone(),
        config.settings.browser_no_sandbox,
        config.settings.browser_start_timeout,
    ));
    Worker {
        settings: config.settings,
        api_base_url: config.api_base_url,
        api,
        storage,
        engine,
        shutdown,
    }
    .run()
    .await;
    tracing::info!("render worker stopped");
    ExitCode::SUCCESS
}
