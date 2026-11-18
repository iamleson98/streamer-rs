//! The whole worker against a fake site API, a fake storage, fake `ffmpeg` / `ffprobe` scripts and a
//! browser double that posts to the sink the way the render page does: no network and no real
//! browser.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{post, put};
use axum::{Json, Router};
use serde_json::{Value, json};
use stream_common::api::ApiClient;
use stream_common::key::Secret;
use stream_common::redact::RedactingMakeWriter;
use stream_render::browser::{Browser, BrowserError, BrowserSession, PageStatus};
use stream_render::config::Settings;
use stream_render::job::Worker;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const WORKER_TOKEN: &str = "worker-token-for-tests";
const RENDER_TOKEN: &str = "render-token-SECRET0001";
const SIGNATURE: &str = "SIGSECRET0123456789";
const REQUEST: &str = "request0000000000000000001";
const REQUEST_2: &str = "request0000000000000000002";

const PROBE_3S: &str = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"duration":"3.000000"}}"#;
const PROBE_100S: &str = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"duration":"100.000000"}}"#;

// ---- logs: every test checks that no secret reached them -------------------------------------

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn logs() -> &'static LogBuffer {
    static LOGS: OnceLock<LogBuffer> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buffer = LogBuffer::default();
        let writer = buffer.clone();
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(RedactingMakeWriter(move || writer.clone()))
                .with_ansi(false)
                .finish(),
        );
        buffer
    })
}

fn assert_logs_are_clean() {
    let text = String::from_utf8_lossy(&logs().0.lock().unwrap()).into_owned();
    for secret in [
        WORKER_TOKEN,
        "render-token-SECRET",
        "fresh-token-SECRET",
        SIGNATURE,
    ] {
        assert!(
            !text.contains(secret),
            "a secret reached the logs: {secret}"
        );
    }
}

// ---- the fake site ---------------------------------------------------------------------------

#[derive(Default)]
struct Site {
    claims: VecDeque<Value>,
    heartbeats: Vec<Value>,
    heartbeat_auth: Vec<String>,
    /// Heartbeat number (from 1) from which the site answers stop.
    stop_from: Option<usize>,
    /// Heartbeat number (from 1) from which the site answers 409: the worker does not hold it.
    conflict_from: Option<usize>,
    finish_503s: usize,
    finishes: Vec<Value>,
    uploads_asked: usize,
    parts: Vec<(u32, Vec<u8>)>,
    /// How many requests are being worked on at once, and the most there were.
    now_rendering: usize,
    most_rendering: usize,
}

type SiteState = Arc<Mutex<Site>>;

async fn serve(app: Router) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

async fn start_site() -> (SiteState, String) {
    let site = SiteState::default();
    let storage = serve(
        Router::new()
            .route(
                "/part/{n}",
                put(
                    |State(site): State<SiteState>, UrlPath(n): UrlPath<u32>, body: Bytes| async move {
                        site.lock().unwrap().parts.push((n, body.to_vec()));
                        ([("etag", format!("\"etag-{n}\""))], StatusCode::OK)
                    },
                ),
            )
            .with_state(site.clone()),
    )
    .await;
    let api = Router::new()
        .route(
            "/api/v4/lms/stream_worker/render/claim",
            post(|State(site): State<SiteState>, headers: HeaderMap| async move {
                assert_eq!(headers["authorization"], format!("Bearer {WORKER_TOKEN}"));
                let mut s = site.lock().unwrap();
                match s.claims.pop_front() {
                    Some(claim) => {
                        s.now_rendering += 1;
                        s.most_rendering = s.most_rendering.max(s.now_rendering);
                        Json(claim).into_response()
                    }
                    None => StatusCode::NO_CONTENT.into_response(),
                }
            }),
        )
        .route(
            "/api/v4/lms/stream_worker/render/{id}/heartbeat",
            post(
                |State(site): State<SiteState>, headers: HeaderMap, Json(body): Json<Value>| async move {
                    let mut s = site.lock().unwrap();
                    s.heartbeats.push(body);
                    s.heartbeat_auth
                        .push(headers["authorization"].to_str().unwrap().to_owned());
                    let n = s.heartbeats.len();
                    if s.conflict_from.is_some_and(|from| n >= from) {
                        return StatusCode::CONFLICT.into_response();
                    }
                    let stop = s.stop_from.is_some_and(|from| n >= from);
                    Json(json!({
                        "lease_until": 1, "stop": stop,
                        "render_token": format!("fresh-token-SECRET{n:04}"),
                        "render_token_expires_at": 2,
                    }))
                    .into_response()
                },
            ),
        )
        .route(
            "/api/v4/lms/stream_worker/render/{id}/uploads",
            post(
                move |State(site): State<SiteState>, Json(body): Json<Value>| async move {
                    site.lock().unwrap().uploads_asked += 1;
                    let size = body["size_bytes"].as_i64().unwrap();
                    let part_bytes = 4;
                    let count = (size + part_bytes - 1) / part_bytes;
                    let parts: Vec<_> = (1..=count)
                        .map(|n| json!({"number": n, "url": format!("http://{storage}/part/{n}?sig={SIGNATURE}")}))
                        .collect();
                    Json(json!({"part_bytes": part_bytes, "parts": parts}))
                },
            ),
        )
        .route(
            "/api/v4/lms/stream_worker/render/{id}/finish",
            post(|State(site): State<SiteState>, Json(body): Json<Value>| async move {
                let mut s = site.lock().unwrap();
                if s.finish_503s > 0 {
                    s.finish_503s -= 1;
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                s.finishes.push(body);
                s.now_rendering = s.now_rendering.saturating_sub(1);
                Json(json!({"status": "OK"})).into_response()
            }),
        )
        .with_state(site.clone());
    let addr = serve(api).await;
    (site, format!("http://{addr}/api/v4"))
}

fn claim(request: &str, duration_ms: i64) -> Value {
    json!({
        "request_id": request, "recording_id": "recording0000000000001",
        "duration_ms": duration_ms, "render_token": RENDER_TOKEN,
        "token_expires_at": 1, "lease_until": 1,
    })
}

// ---- the browser double ----------------------------------------------------------------------

#[derive(Clone)]
enum Page {
    /// Posts these parts, then done with the matching totals.
    Complete(Vec<&'static [u8]>),
    /// Posts one part, then reports this error code.
    Error(&'static str),
    /// The browser dies a moment after it started.
    Crash,
    /// Never ends.
    Hang,
    /// The browser stays up but the page no longer answers (its renderer was killed).
    Stalled,
}

#[derive(Default)]
struct Seen {
    launches: AtomicUsize,
    closed: AtomicBool,
    tokens: Mutex<Vec<String>>,
    page_urls: Mutex<Vec<String>>,
    profile_dirs: Mutex<Vec<PathBuf>>,
    /// What a page fetched from the worker's own server (the render route).
    served_page: Mutex<Option<String>>,
}

struct Stub {
    page: Page,
    /// How long the page "works" before it ends, so heartbeats happen meanwhile.
    work: Duration,
    seen: Arc<Seen>,
}

struct StubSession {
    answers: bool,
    exited: watch::Receiver<bool>,
    seen: Arc<Seen>,
    task: tokio::task::JoinHandle<()>,
}

impl Browser for Stub {
    type Session = StubSession;

    fn launch(
        &self,
        page_url: &str,
        profile_dir: &Path,
    ) -> impl Future<Output = Result<StubSession, BrowserError>> + Send {
        let (page, work, seen) = (self.page.clone(), self.work, self.seen.clone());
        let answers = !matches!(page, Page::Stalled);
        let page_url = page_url.to_owned();
        let profile_dir = profile_dir.to_owned();
        async move {
            seen.launches.fetch_add(1, Ordering::SeqCst);
            seen.page_urls.lock().unwrap().push(page_url.clone());
            seen.profile_dirs.lock().unwrap().push(profile_dir);
            let origin = page_url.split("/render").next().unwrap().to_owned();
            let (exit_tx, exited) = watch::channel(false);
            let task_seen = seen.clone();
            let task = tokio::spawn(async move {
                let http = reqwest::Client::new();
                let html = http
                    .get(format!("{origin}/render"))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                *task_seen.served_page.lock().unwrap() = Some(html);
                tokio::time::sleep(work).await;
                let sink = |path: &str| format!("{origin}/__sink{path}");
                match page {
                    Page::Complete(parts) => {
                        let mut bytes = 0;
                        for (i, part) in parts.iter().enumerate() {
                            let n = i + 1;
                            let res = http
                                .post(sink(&format!("/part/{n}")))
                                .body(part.to_vec())
                                .send()
                                .await
                                .unwrap();
                            assert_eq!(res.status(), 204);
                            bytes += part.len();
                        }
                        let done = json!({"parts": parts.len(), "bytes": bytes, "durationMs": 3000, "frames": 90});
                        http.post(sink("/done")).json(&done).send().await.unwrap();
                    }
                    Page::Error(code) => {
                        http.post(sink("/part/1"))
                            .body("partial")
                            .send()
                            .await
                            .unwrap();
                        http.post(sink("/error"))
                            .json(&json!({"code": code}))
                            .send()
                            .await
                            .unwrap();
                    }
                    Page::Crash => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let _ = exit_tx.send(true);
                    }
                    Page::Hang | Page::Stalled => std::future::pending::<()>().await,
                }
                // The double stays "alive" until closed; keeping the sender alive holds `exited` false.
                std::future::pending::<()>().await;
            });
            Ok(StubSession {
                answers,
                exited,
                seen,
                task,
            })
        }
    }
}

impl BrowserSession for StubSession {
    async fn status(&mut self) -> Result<Option<PageStatus>, BrowserError> {
        if !self.answers {
            return Err(BrowserError::Protocol);
        }
        Ok(Some(PageStatus {
            state: "rendering".into(),
            progress: 0.5,
            perf: Default::default(),
        }))
    }

    async fn set_token(&mut self, token: &str) -> Result<(), BrowserError> {
        self.seen.tokens.lock().unwrap().push(token.to_owned());
        Ok(())
    }

    fn exited(&self) -> watch::Receiver<bool> {
        self.exited.clone()
    }

    async fn close(self) {
        self.seen.closed.store(true, Ordering::SeqCst);
        self.task.abort();
    }
}

// ---- the fixture -----------------------------------------------------------------------------

struct Fixture {
    dir: tempfile::TempDir,
    site: SiteState,
    seen: Arc<Seen>,
    shutdown: CancellationToken,
    settings: Settings,
    api_base: String,
    page: Page,
    work: Duration,
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    async fn new(page: Page) -> Self {
        logs();
        let dir = tempfile::tempdir().unwrap();
        let web = dir.path().join("web");
        std::fs::create_dir(&web).unwrap();
        std::fs::write(web.join("200.html"), "<html>render page</html>").unwrap();
        script(
            &dir.path().join("ffmpeg"),
            // -nostdin -v error -y -i RAW ... OUT: RAW is the sixth argument, OUT the last.
            "for a; do last=$a; done; cp \"$6\" \"$last\"",
        );
        script(&dir.path().join("ffprobe"), &format!("echo '{PROBE_3S}'"));
        let (site, api_base) = start_site().await;
        let mut settings = Settings::new("worker-1");
        settings.web_dir = web;
        settings.work_dir = dir.path().join("work");
        settings.ffmpeg = dir.path().join("ffmpeg");
        settings.ffprobe = dir.path().join("ffprobe");
        settings.poll_interval = Duration::from_millis(50);
        settings.heartbeat_interval = Duration::from_millis(100);
        settings.status_interval = Duration::from_millis(50);
        settings.lease_timeout = Duration::from_millis(500);
        settings.deadline_grace = Duration::from_secs(30);
        settings.retry_delay = Duration::from_millis(10);
        settings.request_timeout = Duration::from_secs(5);
        std::fs::create_dir(&settings.work_dir).unwrap();
        Self {
            dir,
            site,
            seen: Arc::default(),
            shutdown: CancellationToken::new(),
            settings,
            api_base,
            page,
            work: Duration::from_millis(400),
        }
    }

    fn queue(&self, claim: Value) {
        self.site.lock().unwrap().claims.push_back(claim);
    }

    fn worker(&self) -> Worker<Stub> {
        Worker {
            settings: self.settings.clone(),
            api_base_url: self.api_base.clone(),
            api: ApiClient::new(
                &self.api_base,
                Secret::new(WORKER_TOKEN),
                Duration::from_secs(5),
            )
            .unwrap(),
            storage: reqwest::Client::new(),
            browser: Stub {
                page: self.page.clone(),
                work: self.work,
                seen: self.seen.clone(),
            },
            shutdown: self.shutdown.clone(),
        }
    }

    /// Runs the worker until `until` holds (or fails after 20 s), then shuts it down.
    async fn run_until(&self, until: impl Fn(&Site) -> bool) {
        let worker = self.worker();
        let handle = tokio::spawn(async move { worker.run().await });
        let site = self.site.clone();
        let waited = tokio::time::timeout(Duration::from_secs(20), async move {
            while !until(&site.lock().unwrap()) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        // The worker leaves its work directory clean before it takes the next claim.
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("the worker stops after shutdown")
            .unwrap();
        waited.expect("the expected state was not reached");
    }

    fn work_dir_is_empty(&self) -> bool {
        std::fs::read_dir(&self.settings.work_dir)
            .unwrap()
            .next()
            .is_none()
    }
}

fn finishes(site: &Site) -> bool {
    !site.finishes.is_empty()
}

// ---- the tests -------------------------------------------------------------------------------

#[tokio::test]
async fn a_render_is_uploaded_and_finished_with_its_parts() {
    let f = Fixture::new(Page::Complete(vec![b"hello ", b"world"])).await;
    f.queue(claim(REQUEST, 3000));
    f.run_until(finishes).await;

    let site = f.site.lock().unwrap();
    let finish = &site.finishes[0];
    assert_eq!(finish["worker_id"], "worker-1");
    assert!(finish.get("failure_code").is_none());
    assert_eq!(finish["size_bytes"], 11);
    assert_eq!(finish["rendered_ms"], 3000);
    assert_eq!(
        finish["parts"],
        json!([
            {"number": 1, "etag": "\"etag-1\""},
            {"number": 2, "etag": "\"etag-2\""},
            {"number": 3, "etag": "\"etag-3\""},
        ])
    );
    let uploaded: Vec<u8> = site.parts.iter().flat_map(|(_, b)| b.clone()).collect();
    assert_eq!(
        uploaded, b"hello world",
        "the (fake) remux copied the page's output"
    );

    // The page was opened on the worker's own server with its inputs in the fragment.
    let url = f.seen.page_urls.lock().unwrap()[0].clone();
    assert!(url.starts_with("http://127.0.0.1:"));
    assert!(url.contains(&format!("#api=/api/v4&token={RENDER_TOKEN}&recording=recording0000000000001&request={REQUEST}&sink=/__sink")));
    assert_eq!(
        f.seen.served_page.lock().unwrap().as_deref(),
        Some("<html>render page</html>")
    );
    assert!(f.seen.closed.load(Ordering::SeqCst));
    assert!(f.work_dir_is_empty(), "the render's directory is deleted");
    drop(site);
    assert_logs_are_clean();
}

#[tokio::test]
async fn heartbeats_report_progress_and_bring_the_page_fresh_tokens() {
    let mut f = Fixture::new(Page::Complete(vec![b"abcd"])).await;
    f.work = Duration::from_millis(800);
    f.queue(claim(REQUEST, 3000));
    f.run_until(finishes).await;

    let site = f.site.lock().unwrap();
    assert!(
        site.heartbeats.len() >= 3,
        "{} heartbeats",
        site.heartbeats.len()
    );
    for beat in &site.heartbeats {
        assert_eq!(beat["worker_id"], "worker-1");
    }
    assert!(
        site.heartbeats.iter().any(|b| b["position_ms"] == 1500),
        "half of 3000 ms"
    );
    assert!(
        site.heartbeat_auth
            .iter()
            .all(|a| a == &format!("Bearer {WORKER_TOKEN}"))
    );
    let tokens = f.seen.tokens.lock().unwrap();
    assert!(!tokens.is_empty(), "the page was given a fresh token");
    assert!(tokens.iter().all(|t| t.starts_with("fresh-token-SECRET")));
    drop(tokens);
    drop(site);
    assert_logs_are_clean();
}

#[tokio::test]
async fn the_pages_error_codes_become_the_finish_codes() {
    for (page_code, expected) in [
        ("recording_unreadable", "recording_unreadable"),
        ("decode_failed", "render_failed"),
        ("unsupported_browser", "render_failed"),
    ] {
        let f = Fixture::new(Page::Error(page_code)).await;
        f.queue(claim(REQUEST, 3000));
        f.run_until(finishes).await;
        let site = f.site.lock().unwrap();
        assert_eq!(site.finishes[0]["failure_code"], expected, "{page_code}");
        assert!(site.finishes[0].get("parts").is_none());
        assert_eq!(site.uploads_asked, 0);
        assert!(f.seen.closed.load(Ordering::SeqCst));
        assert!(f.work_dir_is_empty());
    }
}

#[tokio::test]
async fn a_browser_crash_fails_the_render_and_cleans_up() {
    let f = Fixture::new(Page::Crash).await;
    f.queue(claim(REQUEST, 3000));
    f.run_until(finishes).await;
    assert_eq!(
        f.site.lock().unwrap().finishes[0]["failure_code"],
        "render_failed"
    );
    assert!(f.seen.closed.load(Ordering::SeqCst));
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn a_page_that_stops_answering_is_dropped_unreported_long_before_the_deadline() {
    let f = Fixture::new(Page::Stalled).await;
    f.queue(claim(REQUEST, 3_600_000));
    let worker = f.worker();
    let handle = tokio::spawn(async move { worker.run().await });
    // Twelve failed reads at 50 ms are well under a second; the deadline is hours away.
    tokio::time::timeout(Duration::from_secs(20), async {
        while !f.seen.closed.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the browser is killed once the page stops answering");
    f.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .unwrap()
        .unwrap();
    assert!(
        f.site.lock().unwrap().finishes.is_empty(),
        "nothing is reported: the lease runs out and the site queues it again"
    );
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn a_render_past_its_deadline_is_stopped() {
    let mut f = Fixture::new(Page::Hang).await;
    f.settings.deadline_grace = Duration::from_millis(300);
    f.queue(claim(REQUEST, 1));
    f.run_until(finishes).await;
    assert_eq!(
        f.site.lock().unwrap().finishes[0]["failure_code"],
        "render_failed"
    );
    assert!(
        f.seen.closed.load(Ordering::SeqCst),
        "the browser is killed"
    );
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn a_stop_from_the_site_kills_the_browser_and_cleans_up() {
    let f = Fixture::new(Page::Hang).await;
    f.site.lock().unwrap().stop_from = Some(2);
    f.queue(claim(REQUEST, 3000));
    f.run_until(|site| site.heartbeats.len() >= 2).await;
    let site = f.site.lock().unwrap();
    assert!(
        site.finishes.is_empty(),
        "the site asked for the stop: it knows, nothing is reported"
    );
    assert!(f.seen.closed.load(Ordering::SeqCst));
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn a_conflict_on_the_heartbeat_aborts_the_render_without_a_report() {
    let f = Fixture::new(Page::Hang).await;
    f.site.lock().unwrap().conflict_from = Some(2);
    f.queue(claim(REQUEST, 3000));
    f.run_until(|site| site.heartbeats.len() >= 2).await;
    let site = f.site.lock().unwrap();
    assert!(site.finishes.is_empty(), "the site already knows");
    assert!(f.seen.closed.load(Ordering::SeqCst));
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn a_shutdown_mid_render_drops_it_unreported_and_cleans_up() {
    let f = Fixture::new(Page::Hang).await;
    f.queue(claim(REQUEST, 3000));
    let worker = f.worker();
    let handle = tokio::spawn(async move { worker.run().await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.seen.launches.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    f.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .unwrap()
        .unwrap();
    assert!(
        f.site.lock().unwrap().finishes.is_empty(),
        "the lease runs out and the site queues it again"
    );
    assert!(f.seen.closed.load(Ordering::SeqCst));
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn a_file_whose_length_is_not_the_recordings_is_not_uploaded() {
    let f = Fixture::new(Page::Complete(vec![b"abcd"])).await;
    script(
        &f.dir.path().join("ffprobe"),
        &format!("echo '{PROBE_100S}'"),
    );
    f.queue(claim(REQUEST, 3000));
    f.run_until(finishes).await;
    let site = f.site.lock().unwrap();
    assert_eq!(site.finishes[0]["failure_code"], "render_failed");
    assert_eq!(site.uploads_asked, 0);
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn a_remux_failure_fails_the_render() {
    let f = Fixture::new(Page::Complete(vec![b"abcd"])).await;
    script(
        &f.dir.path().join("ffmpeg"),
        "echo 'Invalid data found' >&2; exit 1",
    );
    f.queue(claim(REQUEST, 3000));
    f.run_until(finishes).await;
    assert_eq!(
        f.site.lock().unwrap().finishes[0]["failure_code"],
        "render_failed"
    );
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn the_finish_is_tried_again_when_the_site_is_busy() {
    let f = Fixture::new(Page::Error("render_failed")).await;
    f.site.lock().unwrap().finish_503s = 2;
    f.queue(claim(REQUEST, 3000));
    f.run_until(finishes).await;
    assert_eq!(f.site.lock().unwrap().finishes.len(), 1);
}

#[tokio::test]
async fn renders_run_one_at_a_time_and_old_work_is_cleaned_at_start() {
    let f = Fixture::new(Page::Complete(vec![b"abcd"])).await;
    std::fs::create_dir_all(f.settings.work_dir.join("left-behind")).unwrap();
    std::fs::write(f.settings.work_dir.join("left-behind/raw.mp4"), "old").unwrap();
    f.queue(claim(REQUEST, 3000));
    f.queue(claim(REQUEST_2, 3000));
    f.run_until(|s| s.finishes.len() == 2).await;
    let site = f.site.lock().unwrap();
    assert_eq!(
        site.most_rendering, 1,
        "the second claim waited for the first to finish"
    );
    assert_eq!(f.seen.launches.load(Ordering::SeqCst), 2);
    assert!(f.work_dir_is_empty());
}

#[tokio::test]
async fn a_claim_the_worker_cannot_run_is_failed_not_run() {
    let f = Fixture::new(Page::Hang).await;
    f.queue(json!({
        "request_id": "request000000000000000000a", "recording_id": "bad id",
        "duration_ms": 3000, "render_token": RENDER_TOKEN, "token_expires_at": 1, "lease_until": 1,
    }));
    f.run_until(finishes).await;
    assert_eq!(f.seen.launches.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.site.lock().unwrap().finishes[0]["failure_code"],
        "render_failed"
    );
}
