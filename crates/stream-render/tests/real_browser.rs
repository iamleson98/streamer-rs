//! A render with a REAL `chromium-headless-shell`: the worker opens a stand-in for the render page
//! (`tests/fixtures/render.html`, which follows the page contract) on its own local server, the
//! page reads a fake recording through the API proxy (a manifest and a ranged file read that
//! redirects to a fake storage), works until the heartbeat has brought it a fresh token through the
//! DevTools protocol, then streams its output into the sink; the worker remuxes with fake `ffmpeg`
//! and `ffprobe` scripts and uploads.
//!
//! Ignored by default: it needs the browser. Run it with the headless shell's path in
//! `RENDER_TEST_BROWSER` (and `RENDER_TEST_NO_SANDBOX=1` where the container cannot give the
//! browser user namespaces):
//!
//! ```text
//! RENDER_TEST_BROWSER=/path/to/headless_shell cargo test -p stream-render --test real_browser -- --ignored --nocapture
//! ```

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{Value, json};
use stream_common::api::ApiClient;
use stream_common::key::Secret;
use stream_render::chrome::Chrome;
use stream_render::chromium::ChromiumEngine;
use stream_render::config::Settings;
use stream_render::job::Worker;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const FILE: &[u8] = b"0123456789abcdefghij";
const RENDER_TOKEN: &str = "render-token-0001";
const PROBE_3S: &str = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"duration":"3.000000"}}"#;

#[derive(Default)]
struct Site {
    manifest_tokens: Vec<String>,
    heartbeats: usize,
    uploaded: Vec<(u32, usize)>,
    finishes: Vec<Value>,
    claimed: bool,
}

type SiteState = Arc<Mutex<Site>>;

async fn serve(app: Router) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
#[ignore = "needs a real chromium-headless-shell: set RENDER_TEST_BROWSER"]
async fn a_real_browser_renders_through_the_worker() {
    stream_common::redact::init_logging();
    let browser = std::env::var("RENDER_TEST_BROWSER").expect("RENDER_TEST_BROWSER");
    let no_sandbox = std::env::var("RENDER_TEST_NO_SANDBOX").is_ok_and(|v| v == "1");

    let site = SiteState::default();
    let storage = serve(Router::new().route(
        "/object",
        get(|headers: HeaderMap| async move {
            let range = headers[header::RANGE].to_str().unwrap();
            assert_eq!(range, "bytes=2-5");
            (
                StatusCode::PARTIAL_CONTENT,
                [(header::CONTENT_RANGE, "bytes 2-5/20")],
                FILE[2..=5].to_vec(),
            )
        }),
    ))
    .await;
    let upload_storage = serve(
        Router::new()
            .route(
                "/part/{n}",
                put(|State(site): State<SiteState>, UrlPath(n): UrlPath<u32>, body: Bytes| async move {
                    site.lock().unwrap().uploaded.push((n, body.len()));
                    ([("etag", format!("\"e{n}\""))], StatusCode::OK)
                }),
            )
            .with_state(site.clone()),
    )
    .await;
    let api = Router::new()
        .route(
            "/api/v4/lms/stream_worker/render/claim",
            post(|State(site): State<SiteState>| async move {
                let mut s = site.lock().unwrap();
                if std::mem::replace(&mut s.claimed, true) {
                    return StatusCode::NO_CONTENT.into_response();
                }
                Json(json!({
                    "request_id": "request000000000000000000a", "recording_id": "recording1", "duration_ms": 3000,
                    "render_token": RENDER_TOKEN, "token_expires_at": 1, "lease_until": 1,
                }))
                .into_response()
            }),
        )
        .route(
            "/api/v4/lms/stream_worker/render/{id}/heartbeat",
            post(|State(site): State<SiteState>| async move {
                let mut s = site.lock().unwrap();
                s.heartbeats += 1;
                Json(json!({
                    "lease_until": 1, "stop": false,
                    "render_token": format!("render-token-fresh-{}", s.heartbeats),
                    "render_token_expires_at": 2,
                }))
            }),
        )
        .route(
            "/api/v4/lms/recordings/{id}/manifest",
            get(|State(site): State<SiteState>, headers: HeaderMap| async move {
                let token = headers[header::AUTHORIZATION].to_str().unwrap().to_owned();
                site.lock().unwrap().manifest_tokens.push(token);
                Json(json!({"files": [], "duration_ms": 3000}))
            }),
        )
        .route(
            "/api/v4/lms/recordings/{id}/files/{n}",
            get(move || async move {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, format!("http://{storage}/object?sig=abc"))],
                )
            }),
        )
        .route(
            "/api/v4/lms/stream_worker/render/{id}/uploads",
            post(move |Json(body): Json<Value>| async move {
                let size = body["size_bytes"].as_i64().unwrap();
                let part_bytes: i64 = 1 << 20;
                let parts: Vec<_> = (1..=(size + part_bytes - 1) / part_bytes)
                    .map(|n| json!({"number": n, "url": format!("http://{upload_storage}/part/{n}?sig=abc")}))
                    .collect();
                Json(json!({"part_bytes": part_bytes, "parts": parts}))
            }),
        )
        .route(
            "/api/v4/lms/stream_worker/render/{id}/finish",
            post(|State(site): State<SiteState>, Json(body): Json<Value>| async move {
                site.lock().unwrap().finishes.push(body);
                Json(json!({"status": "OK"}))
            }),
        )
        .with_state(site.clone());
    let api_addr = serve(api).await;
    let api_base = format!("http://{api_addr}/api/v4");

    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render.html"),
        web.join("200.html"),
    )
    .unwrap();
    // Arguments: -nostdin -v error -y -i RAW ... OUT.
    script(
        &dir.path().join("ffmpeg"),
        "for a; do last=$a; done; cp \"$6\" \"$last\"",
    );
    script(&dir.path().join("ffprobe"), &format!("echo '{PROBE_3S}'"));
    let mut settings = Settings::new("worker-1");
    settings.web_dir = web;
    settings.work_dir = dir.path().join("work");
    settings.ffmpeg = dir.path().join("ffmpeg");
    settings.ffprobe = dir.path().join("ffprobe");
    settings.heartbeat_interval = Duration::from_millis(400);
    settings.status_interval = Duration::from_millis(200);
    settings.poll_interval = Duration::from_millis(100);
    std::fs::create_dir(&settings.work_dir).unwrap();

    let shutdown = CancellationToken::new();
    let worker = Worker {
        engine: ChromiumEngine::new(Chrome::new(
            browser.into(),
            no_sandbox,
            settings.browser_start_timeout,
        )),
        settings,
        api_base_url: api_base.clone(),
        api: ApiClient::new(
            &api_base,
            Secret::new("worker-token"),
            Duration::from_secs(5),
        )
        .unwrap(),
        storage: reqwest::Client::new(),
        shutdown: shutdown.clone(),
    };
    let started = Instant::now();
    let run = tokio::spawn(async move { worker.run().await });
    tokio::time::timeout(Duration::from_secs(90), async {
        while site.lock().unwrap().finishes.is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the render did not finish");
    let elapsed = started.elapsed();
    shutdown.cancel();
    run.await.unwrap();
    eprintln!("real browser render: claim to finish in {elapsed:?}");

    let site = site.lock().unwrap();
    let finish = &site.finishes[0];
    assert!(finish.get("failure_code").is_none(), "failed: {finish}");
    let total = (2 << 20) + 12345;
    assert_eq!(finish["size_bytes"], total);
    assert_eq!(finish["parts"].as_array().unwrap().len(), 3);
    assert_eq!(site.uploaded.iter().map(|(_, n)| n).sum::<usize>(), total);
    // The page read with the render token at first and with a fresh one after a heartbeat.
    assert_eq!(site.manifest_tokens[0], format!("Bearer {RENDER_TOKEN}"));
    assert!(
        site.manifest_tokens
            .last()
            .unwrap()
            .starts_with("Bearer render-token-fresh-"),
        "{:?}",
        site.manifest_tokens
    );
    assert!(
        std::fs::read_dir(dir.path().join("work"))
            .unwrap()
            .next()
            .is_none(),
        "the render's directory is deleted"
    );
}
