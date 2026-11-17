//! The local server of one render: the web app, the API proxy and the sink, on `127.0.0.1` at a
//! random port. It lives as long as the render and is the only address the browser is given.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::proxy::{ApiProxy, WebApp};
use crate::sink::{self, Sink, SinkEvent};

/// What one render's server serves and where it writes.
#[derive(Debug)]
pub struct ServerConfig<'a> {
    pub web_dir: &'a Path,
    /// The site's API root (`https://host/api/v4`).
    pub api_base: &'a str,
    pub recording_id: &'a str,
    /// The raw output file the sink creates.
    pub raw_path: &'a Path,
    pub max_raw_bytes: u64,
}

/// A running server. Dropping it without `stop` leaves the task to end with the runtime.
#[derive(Debug)]
pub struct LocalServer {
    addr: SocketAddr,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl LocalServer {
    /// Starts serving. The page's end events (done, error) arrive on `events`.
    pub async fn start(
        config: ServerConfig<'_>,
        events: mpsc::Sender<SinkEvent>,
    ) -> std::io::Result<Self> {
        let sink = Sink::create(config.raw_path, events, config.max_raw_bytes).await?;
        let proxy = ApiProxy::new(config.api_base, config.recording_id)
            .map_err(|_| std::io::Error::other("cannot build the API client"))?;

        let sink_routes = Router::new()
            .route("/__sink/part/{n}", post(sink::part_route))
            .route("/__sink/done", post(sink::done_route))
            .route("/__sink/error", post(sink::error_route))
            .with_state(sink);
        let api_routes = Router::new()
            .route(
                "/api/v4/{*rest}",
                get(ApiProxy::handle).head(ApiProxy::handle),
            )
            .with_state(proxy);
        let web = Router::new()
            .fallback(WebApp::handle)
            .with_state(WebApp::new(config.web_dir));
        // The sink streams its bodies itself; the JSON bodies (done, error) are a few bytes.
        let app = sink_routes
            .merge(api_routes)
            .merge(web)
            .layer(DefaultBodyLimit::max(64 << 10));

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move { stop.cancelled().await })
                .await;
        });
        Ok(Self {
            addr,
            shutdown,
            task,
        })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Stops accepting and ends the open connections within a moment.
    pub async fn stop(self) {
        self.shutdown.cancel();
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(2), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_server_serves_the_page_the_sink_and_the_proxy_on_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let web = dir.path().join("web");
        std::fs::create_dir(&web).unwrap();
        std::fs::write(web.join("200.html"), "<html>page</html>").unwrap();
        let raw = dir.path().join("raw.mp4");
        let (tx, mut rx) = mpsc::channel(4);
        let server = LocalServer::start(
            ServerConfig {
                web_dir: &web,
                api_base: "http://127.0.0.1:1/api/v4",
                recording_id: "rec1",
                raw_path: &raw,
                max_raw_bytes: 1 << 20,
            },
            tx,
        )
        .await
        .unwrap();
        let base = format!("http://127.0.0.1:{}", server.port());
        let client = reqwest::Client::new();

        let page = client.get(format!("{base}/render")).send().await.unwrap();
        assert_eq!(page.text().await.unwrap(), "<html>page</html>");

        let part = client
            .post(format!("{base}/__sink/part/1"))
            .body("abc")
            .send()
            .await
            .unwrap();
        assert_eq!(part.status(), 204);
        let done = client
            .post(format!("{base}/__sink/done"))
            .json(&serde_json::json!({"parts":1,"bytes":3,"durationMs":1000,"frames":30}))
            .send()
            .await
            .unwrap();
        assert_eq!(done.status(), 204);
        assert_eq!(
            rx.recv().await.unwrap(),
            SinkEvent::Done {
                bytes: 3,
                duration_ms: 1000,
                frames: 30
            }
        );

        let proxied = client
            .get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .send()
            .await
            .unwrap();
        assert_eq!(proxied.status(), 502);
        let other = client
            .get(format!("{base}/api/v4/users/me"))
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), 404);

        server.stop().await;
        assert!(client.get(format!("{base}/render")).send().await.is_err());
    }

    #[tokio::test]
    async fn the_sink_takes_a_part_larger_than_the_json_limit() {
        let dir = tempfile::tempdir().unwrap();
        let web = dir.path().join("web");
        std::fs::create_dir(&web).unwrap();
        std::fs::write(web.join("200.html"), "x").unwrap();
        let raw = dir.path().join("raw.mp4");
        let (tx, _rx) = mpsc::channel(4);
        let server = LocalServer::start(
            ServerConfig {
                web_dir: &web,
                api_base: "http://127.0.0.1:1/api/v4",
                recording_id: "rec1",
                raw_path: &raw,
                max_raw_bytes: 8 << 20,
            },
            tx,
        )
        .await
        .unwrap();
        let res = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{}/__sink/part/1", server.port()))
            .body(vec![1u8; 1 << 20])
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204);
        assert_eq!(std::fs::metadata(&raw).unwrap().len(), 1 << 20);
        server.stop().await;
    }
}
