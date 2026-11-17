use std::process::ExitCode;

use stream_common::api::ApiClient;
use stream_common::redact::init_logging;
use stream_runner::config::Config;
use stream_runner::runner::Runner;
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
    let client = match ApiClient::new(
        &config.api_base_url,
        config.token,
        config.settings.request_timeout,
    ) {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "refusing to start");
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

    tracing::info!(worker_id = %config.settings.worker_id, "stream runner started");
    Runner::new(config.settings, client, config.private_key, shutdown)
        .run()
        .await;
    tracing::info!("stream runner stopped");
    ExitCode::SUCCESS
}
