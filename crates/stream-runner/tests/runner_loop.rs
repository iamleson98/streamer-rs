//! The whole loop against a fake site API and fake `ffmpeg` / `ffprobe` scripts: no network and no
//! real media. The scripts record how they were called and behave as the mode files say.

use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
use aws_lc_rs::rsa::{
    KeySize, OAEP_SHA256_MGF1SHA256, OaepPublicEncryptingKey, PrivateDecryptingKey,
};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use stream_common::api::ApiClient;
use stream_common::key::{RunnerPrivateKey, Secret};
use stream_common::redact::RedactingMakeWriter;
use stream_runner::config::Settings;
use stream_runner::runner::Runner;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "runner-token-for-tests";
const KEY: &str = "abcd-efgh-ijkl-mnop";
const LABEL: &str = "destination0000000000001";
const MEDIA_URL: &str =
    "https://store.example/files/lesson.mp4?X-Amz-Signature=SIGSECRET0123456789&X-Amz-Date=1";
const SIGNATURE: &str = "SIGSECRET0123456789";
const REQUEST_ID: &str = "request0000000000000000001";
const OUTPUT_URL: &str = "rtmps://a.rtmps.youtube.com/live2/abcd-efgh-ijkl-mnop";

const PROBE_OK: &str = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"duration":"3.000000"}}"#;
const PROBE_HEVC: &str = r#"{"streams":[{"codec_type":"video","codec_name":"hevc","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"duration":"3.000000"}}"#;

const FAKE_FFPROBE: &str = r#"#!/bin/sh
PATH=/usr/bin:/bin
[ "$1" = --selftest ] && exit 0
d=$(dirname "$0")
echo "$@" >> "$d/probe-argv.log"
case $(cat "$d/probe-mode") in
  ok) cat "$d/probe-ok.json" ;;
  hevc) cat "$d/probe-hevc.json" ;;
  notfound) echo "[https @ 0x1] HTTP error 404 Not Found" >&2; exit 1 ;;
  broken) echo "Invalid data found when processing input" >&2; exit 1 ;;
  slow) sleep 30 ;;
esac
"#;

const FAKE_FFMPEG: &str = r#"#!/bin/sh
PATH=/usr/bin:/bin
[ "$1" = --selftest ] && exit 0
d=$(dirname "$0")
trap 'echo interrupted >> "$d/interrupted"; exit 255' INT
echo "$@" >> "$d/argv.log"
for a in "$@"; do last=$a; done
runs=$(cat "$d/runs" 2>/dev/null || echo 0)
runs=$((runs + 1))
echo $runs > "$d/runs"
echo ready > "$d/ready"
case $(cat "$d/ffmpeg-mode") in
  ok)
    echo out_time_us=1000000; echo progress=continue
    sleep 0.2
    echo out_time_us=3000000; echo progress=end
    exit 0 ;;
  hang)
    i=0
    while true; do echo out_time_us=$((i * 100000)); i=$((i + 1)); sleep 0.1; done ;;
  fail_then_ok)
    if [ "$runs" -eq 1 ]; then
      echo out_time_us=5000000
      echo "[rtmp @ 0x1] Connection to $last failed: Connection reset by peer" >&2
      exit 1
    fi
    echo out_time_us=1000000; echo progress=end
    exit 0 ;;
  auth)
    echo "[rtmp @ 0x1] Unable to publish: NetStream.Publish.BadName ($last)" >&2
    exit 1 ;;
  always_fail)
    echo out_time_us=500000
    echo "[tcp @ 0x1] Connection refused ($last)" >&2
    exit 1 ;;
  early_end)
    echo out_time_us=1000000; echo progress=end
    exit 0 ;;
  stalled)
    echo out_time_us=0
    while true; do sleep 0.1; done ;;
  noisy)
    head -c 300000 /dev/zero | tr '\000' '\377' >&2; echo >&2
    echo out_time_us=3000000; echo progress=end
    exit 0 ;;
esac
"#;

#[derive(Default)]
struct ApiState {
    claims: VecDeque<Value>,
    claim_unavailable: usize,
    heartbeats: Vec<Value>,
    finishes: Vec<Value>,
    bodies: Vec<String>,
    /// Answer `stop` once a heartbeat reports at least this position (the teacher stops it there).
    stop_at_position_ms: Option<i64>,
    /// Answer 503 to heartbeats once this file exists (ffmpeg has started).
    heartbeats_unavailable_once: Option<std::path::PathBuf>,
    /// Answer every heartbeat with 409 (the runner does not hold the request).
    heartbeats_conflict: bool,
    /// Answer heartbeats with a lease this far ahead of now (otherwise a lease long past).
    lease_ahead: Option<Duration>,
    /// Answer 503 to finishes.
    finish_unavailable: bool,
    finish_calls: usize,
    releases: Vec<Value>,
    bad_token_calls: usize,
}

type Shared = Arc<Mutex<ApiState>>;

fn authorized(headers: &HeaderMap, state: &Shared) -> bool {
    let ok = headers.get("authorization").and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"));
    if !ok {
        state.lock().unwrap().bad_token_calls += 1;
    }
    ok
}

async fn claim(State(state): State<Shared>, headers: HeaderMap, body: String) -> Response {
    if !authorized(&headers, &state) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut s = state.lock().unwrap();
    s.bodies.push(body);
    if s.claim_unavailable > 0 {
        s.claim_unavailable -= 1;
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    match s.claims.pop_front() {
        Some(claim) => Json(claim).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn heartbeat(
    State(state): State<Shared>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: String,
) -> Response {
    if !authorized(&headers, &state) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut s = state.lock().unwrap();
    s.bodies.push(body.clone());
    if s.heartbeats_unavailable_once
        .as_ref()
        .is_some_and(|p| p.exists())
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if s.heartbeats_conflict {
        return StatusCode::CONFLICT.into_response();
    }
    let mut value: Value = serde_json::from_str(&body).unwrap();
    value["id"] = json!(id);
    let position = value["position_ms"].as_i64().unwrap();
    s.heartbeats.push(value);
    let stop = s.stop_at_position_ms.is_some_and(|at| position >= at);
    let lease_until = s
        .lease_ahead
        .map_or(1, |ahead| epoch_ms() + ahead.as_millis() as i64);
    Json(json!({"lease_until": lease_until, "stop": stop})).into_response()
}

async fn finish(
    State(state): State<Shared>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: String,
) -> Response {
    if !authorized(&headers, &state) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut s = state.lock().unwrap();
    s.bodies.push(body.clone());
    s.finish_calls += 1;
    if s.finish_unavailable {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut value: Value = serde_json::from_str(&body).unwrap();
    value["id"] = json!(id);
    s.finishes.push(value);
    Json(json!({"status": "OK"})).into_response()
}

async fn release(
    State(state): State<Shared>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: String,
) -> Response {
    if !authorized(&headers, &state) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut value: Value = serde_json::from_str(&body).unwrap();
    value["id"] = json!(id);
    state.lock().unwrap().releases.push(value);
    StatusCode::NO_CONTENT.into_response()
}

fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

async fn serve(state: Shared) -> SocketAddr {
    let app = Router::new()
        .route("/api/v4/lms/stream_worker/stream/claim", post(claim))
        .route(
            "/api/v4/lms/stream_worker/stream/{id}/heartbeat",
            post(heartbeat),
        )
        .route("/api/v4/lms/stream_worker/stream/{id}/finish", post(finish))
        .route(
            "/api/v4/lms/stream_worker/stream/{id}/release",
            post(release),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Write for Logs {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    state: Shared,
    public: OaepPublicEncryptingKey,
    runner_key: Option<RunnerPrivateKey>,
    logs: Logs,
    addr: SocketAddr,
    lease_timeout: Duration,
    stop_grace: Duration,
    request_timeout: Duration,
    stall_timeout: Duration,
    private_pem: String,
    _log_guard: tracing::subscriber::DefaultGuard,
}

/// Runs a script that was just written once. Another test's `fork` can still hold the write
/// handle for a moment, and executing the file then fails with "text file busy"; once this run
/// works, no handle is left and the runner can start the script.
fn run_when_not_busy(script: &std::path::Path) {
    for _ in 0..500 {
        match std::process::Command::new(script)
            .arg("--selftest")
            .status()
        {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => {
                assert!(result.unwrap().success());
                return;
            }
        }
    }
    panic!("{} stayed busy", script.display());
}

impl Fixture {
    /// `ffmpeg_mode` and `probe_mode` are what the fake tools do (see the scripts).
    async fn new(ffmpeg_mode: &str, probe_mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (name, script) in [("ffmpeg", FAKE_FFMPEG), ("ffprobe", FAKE_FFPROBE)] {
            let path = dir.path().join(name);
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            run_when_not_busy(&path);
        }
        fs::write(dir.path().join("probe-ok.json"), PROBE_OK).unwrap();
        fs::write(dir.path().join("probe-hevc.json"), PROBE_HEVC).unwrap();
        fs::write(dir.path().join("ffmpeg-mode"), ffmpeg_mode).unwrap();
        fs::write(dir.path().join("probe-mode"), probe_mode).unwrap();

        let private = PrivateDecryptingKey::generate(KeySize::Rsa2048).unwrap();
        let public = OaepPublicEncryptingKey::new(private.public_key()).unwrap();
        let der: Pkcs8V1Der<'static> = private.as_der().unwrap();
        let private_pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            STANDARD.encode(der.as_ref())
        );

        let logs = Logs::default();
        let sink = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(RedactingMakeWriter(move || sink.clone()))
            .finish();
        let _log_guard = tracing::subscriber::set_default(subscriber);

        let state: Shared = Arc::default();
        let addr = serve(state.clone()).await;
        Self {
            dir,
            state,
            public,
            runner_key: Some(RunnerPrivateKey::from_key(private).unwrap()),
            logs,
            addr,
            // Long: a loaded machine must not turn a slow heartbeat into a lost lease. The tests of
            // the lease shorten it.
            lease_timeout: Duration::from_secs(30),
            stop_grace: Duration::from_secs(3),
            request_timeout: Duration::from_secs(10),
            stall_timeout: Duration::from_secs(60),
            private_pem,
            _log_guard,
        }
    }

    fn seal(&self, plain: &str, label: &str) -> String {
        let mut out = vec![0u8; self.public.ciphertext_size()];
        let sealed = self
            .public
            .encrypt(
                &OAEP_SHA256_MGF1SHA256,
                plain.as_bytes(),
                &mut out,
                Some(label.as_bytes()),
            )
            .unwrap();
        STANDARD.encode(sealed)
    }

    /// Queues a claim for the stream of a 3 s file; `seal_label` is what the key was sealed with.
    fn queue(&self, platform: &str, seal_label: &str) {
        self.queue_with(platform, seal_label, 3000, MEDIA_URL);
    }

    fn queue_with(&self, platform: &str, seal_label: &str, duration_ms: i64, mp4_url: &str) {
        self.queue_secret(platform, KEY, seal_label, duration_ms, mp4_url);
    }

    /// Queues a claim whose sealed secret is `secret` (a key, or for TikTok a push address).
    fn queue_secret(
        &self,
        platform: &str,
        secret: &str,
        seal_label: &str,
        duration_ms: i64,
        mp4_url: &str,
    ) {
        let claim = json!({
            "request_id": REQUEST_ID,
            "platform": platform,
            "destination_id": LABEL,
            "key_sealed": self.seal(secret, seal_label),
            "mp4_url": mp4_url,
            "duration_ms": duration_ms,
            "lease_until": 1,
        });
        self.state.lock().unwrap().claims.push_back(claim);
    }

    /// Queues a claim for a lesson of `duration_ms` behind a countdown card of `preroll_ms`.
    fn queue_with_preroll(&self, duration_ms: i64, preroll_ms: i64) {
        self.queue_with("YOUTUBE", LABEL, duration_ms, MEDIA_URL);
        let mut state = self.state.lock().unwrap();
        state.claims.back_mut().unwrap()["preroll_ms"] = json!(preroll_ms);
    }

    fn settings(&self) -> Settings {
        let mut s = Settings::new("test-runner");
        s.ffmpeg = self.dir.path().join("ffmpeg");
        s.ffprobe = self.dir.path().join("ffprobe");
        s.poll_interval = Duration::from_millis(20);
        s.heartbeat_interval = Duration::from_millis(50);
        s.lease_timeout = self.lease_timeout;
        s.resume_delay = Duration::from_millis(20);
        s.resume_window = Duration::from_secs(60);
        s.stop_grace = self.stop_grace;
        s.request_timeout = self.request_timeout;
        s.stall_timeout = self.stall_timeout;
        s.media_hosts = vec!["store.example".to_owned()];
        s.probe_retry_delay = Duration::from_millis(10);
        s.finish_retry_delay = Duration::from_millis(10);
        s.probe_timeout = Duration::from_secs(10);
        s
    }

    fn file(&self, name: &str) -> String {
        fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
    }

    fn ffmpeg_runs(&self) -> usize {
        self.file("argv.log").lines().count()
    }

    /// Runs the runner until `done` holds for the API's state (or fails after 20 s), then shuts it down.
    async fn run_until(&mut self, done: impl Fn(&ApiState) -> bool) {
        let client = ApiClient::new(
            &format!("http://{}/api/v4", self.addr),
            Secret::new(TOKEN),
            Duration::from_secs(5),
        )
        .unwrap();
        let shutdown = CancellationToken::new();
        let runner = Runner::new(
            self.settings(),
            client,
            self.runner_key.take().unwrap(),
            shutdown.clone(),
        );
        let handle = tokio::spawn(async move { runner.run().await });
        for _ in 0..1000 {
            if done(&self.state.lock().unwrap()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let reached = done(&self.state.lock().unwrap());
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("the runner stops on shutdown")
            .unwrap();
        assert!(
            reached,
            "the expected state was not reached; logs:\n{}",
            self.logs.text()
        );
    }

    fn finishes(&self) -> Vec<Value> {
        self.state.lock().unwrap().finishes.clone()
    }

    /// Nothing the runner sent or logged holds the key or the presigned signature.
    fn assert_no_secret_leaked(&self) {
        let logs = self.logs.text();
        assert!(!logs.contains(KEY), "the key is in the logs:\n{logs}");
        assert!(
            !logs.contains(SIGNATURE),
            "the signature is in the logs:\n{logs}"
        );
        for body in &self.state.lock().unwrap().bodies {
            assert!(
                !body.contains(KEY) && !body.contains(SIGNATURE),
                "a secret was sent: {body}"
            );
        }
    }
}

#[tokio::test]
async fn streams_the_whole_file_and_reports_done() {
    let mut fx = Fixture::new("ok", "ok").await;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;

    let finish = &fx.finishes()[0];
    assert_eq!(finish["id"], REQUEST_ID);
    assert_eq!(finish["worker_id"], "test-runner");
    assert_eq!(finish["streamed_ms"], 3000);
    assert!(finish.get("failure_code").is_none(), "{finish}");

    let argv = fx.file("argv.log");
    assert_eq!(argv.lines().count(), 1);
    let protocols = stream_common::media::INPUT_PROTOCOLS;
    let expected = format!(
        "-nostdin -loglevel error -progress pipe:1 -protocol_whitelist {protocols} -reconnect 1 \
         -reconnect_on_network_error 1 -reconnect_delay_max 30 -readrate 1 -readrate_initial_burst 2 \
         -rw_timeout 15000000 -tls_verify 1 -ca_file /etc/ssl/certs/ca-certificates.crt -f mp4 -format_whitelist mov,mp4,m4a,3gp,3g2,mj2 \
         -i https://store.example/files/lesson.mp4?X-Amz-Signature={SIGNATURE}&X-Amz-Date=1 -map 0:v:0 -map 0:a:0 \
         -c copy -rw_timeout 15000000 -tls_verify 1 -ca_file /etc/ssl/certs/ca-certificates.crt -f flv -flvflags no_duration_filesize {OUTPUT_URL}"
    );
    assert_eq!(argv.trim(), expected);
    let probe_argv = fx.file("probe-argv.log");
    assert!(probe_argv.contains("-show_streams"));
    assert!(
        probe_argv
            .contains("-rw_timeout 15000000 -tls_verify 1 -ca_file /etc/ssl/certs/ca-certificates.crt -f mp4 -format_whitelist mov,mp4,m4a,3gp,3g2,mj2"),
        "{probe_argv}"
    );

    let s = fx.state.lock().unwrap();
    assert!(!s.heartbeats.is_empty());
    assert!(
        s.heartbeats
            .iter()
            .all(|h| h["id"] == REQUEST_ID && h["worker_id"] == "test-runner")
    );
    assert_eq!(s.bad_token_calls, 0);
    drop(s);
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn the_teacher_stopping_it_interrupts_ffmpeg_and_finishes_without_a_failure() {
    let mut fx = Fixture::new("hang", "ok").await;
    fx.state.lock().unwrap().stop_at_position_ms = Some(300);
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;

    let finish = &fx.finishes()[0];
    assert!(finish.get("failure_code").is_none(), "{finish}");
    assert!(finish["streamed_ms"].as_i64().unwrap() >= 0);
    assert_eq!(
        fx.file("interrupted").lines().count(),
        1,
        "ffmpeg got SIGINT"
    );
    assert_eq!(fx.ffmpeg_runs(), 1, "a stop is not resumed");
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn heartbeats_carry_the_position() {
    let mut fx = Fixture::new("hang", "ok").await;
    fx.state.lock().unwrap().stop_at_position_ms = Some(500);
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    let positions: Vec<i64> = fx
        .state
        .lock()
        .unwrap()
        .heartbeats
        .iter()
        .map(|h| h["position_ms"].as_i64().unwrap())
        .collect();
    assert_eq!(positions[0], 0);
    assert!(positions.iter().any(|p| *p > 0), "{positions:?}");
    assert!(positions.windows(2).all(|w| w[0] <= w[1]), "{positions:?}");
    assert!(
        positions.iter().all(|p| *p <= 3000),
        "never past the file's length: {positions:?}"
    );
}

#[tokio::test]
async fn a_failure_is_resumed_from_the_last_position_and_the_key_is_redacted_from_stderr() {
    let mut fx = Fixture::new("fail_then_ok", "ok").await;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;

    let finish = &fx.finishes()[0];
    assert!(finish.get("failure_code").is_none(), "{finish}");
    let argv = fx.file("argv.log");
    let runs: Vec<&str> = argv.lines().collect();
    assert_eq!(runs.len(), 2);
    assert!(!runs[0].contains("-ss"));
    assert!(runs[1].contains(" -ss 5.000 -i "), "{}", runs[1]);

    let logs = fx.logs.text();
    assert!(
        logs.contains("Connection to rtmps://a.rtmps.youtube.com/live2/[redacted] failed"),
        "{logs}"
    );
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn resumes_twice_then_fails_as_connection_lost() {
    let mut fx = Fixture::new("always_fail", "ok").await;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;

    assert_eq!(fx.ffmpeg_runs(), 3, "the first run and two resumes");
    let finish = &fx.finishes()[0];
    assert_eq!(finish["failure_code"], "connection_lost");
    assert_eq!(finish["streamed_ms"], 1500, "each run adds what it reached");
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn a_refused_key_is_not_retried() {
    let mut fx = Fixture::new("auth", "ok").await;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;

    assert_eq!(fx.ffmpeg_runs(), 1);
    assert_eq!(fx.finishes()[0]["failure_code"], "key_rejected");
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn a_file_the_probe_refuses_is_never_streamed() {
    let mut fx = Fixture::new("ok", "hevc").await;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert_eq!(fx.finishes()[0]["failure_code"], "render_failed");
    assert_eq!(fx.ffmpeg_runs(), 0);
    assert!(!fx.dir.path().join("runs").exists());

    let mut fx = Fixture::new("ok", "broken").await;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert_eq!(fx.finishes()[0]["failure_code"], "render_failed");
    assert_eq!(fx.ffmpeg_runs(), 0);
}

#[tokio::test]
async fn the_countdown_card_counts_as_part_of_the_file() {
    // The fake file is 3 s long: a 1 s lesson behind a 2 s card is all of it.
    let mut fx = Fixture::new("ok", "ok").await;
    fx.queue_with_preroll(1000, 2000);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    let finish = &fx.finishes()[0];
    assert_eq!(finish["streamed_ms"], 3000);
    assert!(finish.get("failure_code").is_none(), "{finish}");
    assert_eq!(fx.ffmpeg_runs(), 1);
}

#[tokio::test]
async fn a_file_with_no_card_when_one_is_due_is_never_streamed() {
    // A 3 s file for a 3 s lesson with a 1 min card: the card was not rendered.
    let mut fx = Fixture::new("ok", "ok").await;
    fx.queue_with_preroll(3000, 60_000);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert_eq!(fx.finishes()[0]["failure_code"], "render_failed");
    assert_eq!(fx.ffmpeg_runs(), 0);
}

#[tokio::test]
async fn a_file_that_is_gone_is_reported_as_missing() {
    let mut fx = Fixture::new("ok", "notfound").await;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert_eq!(fx.finishes()[0]["failure_code"], "file_missing");
    assert_eq!(fx.ffmpeg_runs(), 0);
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn a_key_that_does_not_open_is_reported_as_a_failure_before_anything_runs() {
    let mut fx = Fixture::new("ok", "ok").await;
    fx.queue("YOUTUBE", "another-destination00000001");
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert_eq!(fx.finishes()[0]["failure_code"], "stream_failed");
    assert!(fx.logs.text().contains("do not match"), "a loud log");
    assert_eq!(fx.ffmpeg_runs(), 0);
    assert!(fx.file("probe-argv.log").is_empty());
}

#[tokio::test]
async fn an_unknown_platform_fails_without_streaming() {
    let mut fx = Fixture::new("ok", "ok").await;
    fx.queue("TWITCH", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert_eq!(fx.finishes()[0]["failure_code"], "stream_failed");
    assert_eq!(fx.ffmpeg_runs(), 0);
}

/// The address the one ffmpeg run was told to send to: the last argument.
fn sent_to(fx: &Fixture) -> String {
    let argv = fx.file("argv.log");
    argv.trim().rsplit(' ').next().unwrap().to_owned()
}

#[tokio::test]
async fn facebook_is_streamed_to_its_own_rtmps_address() {
    let mut fx = Fixture::new("ok", "ok").await;
    fx.queue("FACEBOOK", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert!(fx.finishes()[0].get("failure_code").is_none());
    assert_eq!(
        sent_to(&fx),
        format!("rtmps://live-api-s.facebook.com:443/rtmp/{KEY}")
    );
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn tiktok_is_streamed_to_the_server_address_its_key_came_with() {
    let mut fx = Fixture::new("ok", "ok").await;
    let address = format!("rtmp://push-rtmp-f5-sg01.tiktokcdn.com/game/{KEY}");
    fx.queue_secret("TIKTOK", &address, LABEL, 3000, MEDIA_URL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert!(fx.finishes()[0].get("failure_code").is_none());
    assert_eq!(sent_to(&fx), address);
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn a_tiktok_address_on_another_host_is_refused_before_anything_runs() {
    let mut fx = Fixture::new("ok", "ok").await;
    fx.queue_secret(
        "TIKTOK",
        &format!("rtmp://push.tiktokcdn.com.evil.example/game/{KEY}"),
        LABEL,
        3000,
        MEDIA_URL,
    );
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert_eq!(fx.finishes()[0]["failure_code"], "key_rejected");
    assert_eq!(fx.ffmpeg_runs(), 0);
    assert!(fx.file("probe-argv.log").is_empty());
    fx.assert_no_secret_leaked();
}

#[tokio::test]
async fn a_plain_key_for_tiktok_is_refused() {
    let mut fx = Fixture::new("ok", "ok").await;
    fx.queue("TIKTOK", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert_eq!(fx.finishes()[0]["failure_code"], "key_rejected");
    assert_eq!(fx.ffmpeg_runs(), 0);
}

#[tokio::test]
async fn a_busy_site_is_waited_out() {
    let mut fx = Fixture::new("ok", "ok").await;
    fx.state.lock().unwrap().claim_unavailable = 2;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert!(fx.finishes()[0].get("failure_code").is_none());
    assert_eq!(fx.state.lock().unwrap().claim_unavailable, 0);
}

#[tokio::test]
async fn no_heartbeat_for_the_whole_lease_stops_the_stream_and_leaves_it_to_the_site() {
    let mut fx = Fixture::new("hang", "ok").await;
    fx.lease_timeout = Duration::from_millis(400);
    fx.state.lock().unwrap().heartbeats_unavailable_once = Some(fx.dir.path().join("runs"));
    fx.queue("YOUTUBE", LABEL);
    let logs = fx.logs.clone();
    fx.run_until(move |_| logs.text().contains("no heartbeat got through"))
        .await;
    // Shutting down interrupts ffmpeg if the lease did not already.
    assert_eq!(
        fx.file("interrupted").lines().count(),
        1,
        "ffmpeg got SIGINT"
    );

    assert_eq!(fx.ffmpeg_runs(), 1, "not resumed");
    assert!(
        fx.finishes().is_empty(),
        "the site has given the request up; the runner does not finish it"
    );
    assert!(fx.logs.text().contains("no heartbeat got through"));
}

#[tokio::test]
async fn a_shutdown_stops_ffmpeg_cleanly_and_reports_the_stream_as_failed() {
    let mut fx = Fixture::new("hang", "ok").await;
    fx.queue("YOUTUBE", LABEL);
    let dir = fx.dir.path().to_owned();
    let client = ApiClient::new(
        &format!("http://{}/api/v4", fx.addr),
        Secret::new(TOKEN),
        Duration::from_secs(5),
    )
    .unwrap();
    let shutdown = CancellationToken::new();
    let runner = Runner::new(
        fx.settings(),
        client,
        fx.runner_key.take().unwrap(),
        shutdown.clone(),
    );
    let handle = tokio::spawn(async move { runner.run().await });
    // `ready` is written once the fake ffmpeg has its signal handler and is about to loop.
    wait_for_file(&dir.join("ready")).await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the runner stops")
        .unwrap();

    assert_eq!(
        fx.file("interrupted").lines().count(),
        1,
        "ffmpeg got SIGINT"
    );
    assert_eq!(fx.finishes().len(), 1);
    assert_eq!(fx.finishes()[0]["failure_code"], "stream_failed");
    fx.assert_no_secret_leaked();
}

async fn wait_for_file(path: &std::path::Path) {
    for _ in 0..1000 {
        if fs::read_to_string(path).is_ok_and(|text| text.trim() == "ready") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{} never appeared", path.display());
}

#[tokio::test]
async fn a_shutdown_makes_one_short_report() {
    let mut fx = Fixture::new("hang", "ok").await;
    fx.state.lock().unwrap().finish_unavailable = true;
    fx.queue("YOUTUBE", LABEL);
    let dir = fx.dir.path().to_owned();
    let client = ApiClient::new(
        &format!("http://{}/api/v4", fx.addr),
        Secret::new(TOKEN),
        Duration::from_secs(5),
    )
    .unwrap();
    let shutdown = CancellationToken::new();
    let runner = Runner::new(
        fx.settings(),
        client,
        fx.runner_key.take().unwrap(),
        shutdown.clone(),
    );
    let handle = tokio::spawn(async move { runner.run().await });
    wait_for_file(&dir.join("ready")).await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the runner stops without retrying the report")
        .unwrap();
    assert_eq!(fx.state.lock().unwrap().finish_calls, 1);
}

#[tokio::test]
async fn a_stream_claimed_but_not_started_when_shutting_down_is_given_back() {
    let mut fx = Fixture::new("ok", "slow").await;
    fx.queue("YOUTUBE", LABEL);
    let dir = fx.dir.path().to_owned();
    let client = ApiClient::new(
        &format!("http://{}/api/v4", fx.addr),
        Secret::new(TOKEN),
        Duration::from_secs(5),
    )
    .unwrap();
    let shutdown = CancellationToken::new();
    let runner = Runner::new(
        fx.settings(),
        client,
        fx.runner_key.take().unwrap(),
        shutdown.clone(),
    );
    let handle = tokio::spawn(async move { runner.run().await });
    // The probe is running (it logs its arguments first) and ffmpeg has not started.
    for _ in 0..1000 {
        if dir.join("probe-argv.log").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the runner stops")
        .unwrap();

    let s = fx.state.lock().unwrap();
    assert_eq!(s.releases.len(), 1, "{:?}", s.releases);
    assert_eq!(s.releases[0]["id"], REQUEST_ID);
    assert_eq!(s.releases[0]["worker_id"], "test-runner");
    assert_eq!(s.finish_calls, 0, "given back, not failed");
    drop(s);
    assert_eq!(fx.ffmpeg_runs(), 0);
}

#[tokio::test]
async fn a_request_the_site_says_is_not_ours_is_left_without_a_report_or_an_error() {
    let mut fx = Fixture::new("hang", "ok").await;
    fx.state.lock().unwrap().heartbeats_conflict = true;
    fx.queue("YOUTUBE", LABEL);
    let logs = fx.logs.clone();
    fx.run_until(move |_| logs.text().contains("the site already knows"))
        .await;
    let text = fx.logs.text();
    assert!(!text.contains("ERROR"), "{text}");
    assert_eq!(fx.state.lock().unwrap().finish_calls, 0);
}

#[tokio::test]
async fn the_stream_stops_before_the_sites_lease_ends_not_after_a_fixed_time() {
    let mut fx = Fixture::new("hang", "ok").await;
    // The fixed timeout is far away; only the lease the site answered with can end the stream soon.
    fx.lease_timeout = Duration::from_secs(120);
    fx.stop_grace = Duration::from_millis(100);
    fx.request_timeout = Duration::from_millis(100);
    {
        let mut s = fx.state.lock().unwrap();
        s.lease_ahead = Some(Duration::from_millis(1500));
        s.heartbeats_unavailable_once = Some(fx.dir.path().join("runs"));
    }
    fx.queue("YOUTUBE", LABEL);
    let logs = fx.logs.clone();
    let began = std::time::Instant::now();
    fx.run_until(move |_| logs.text().contains("before the lease ran out"))
        .await;
    assert!(
        began.elapsed() < Duration::from_secs(15),
        "{:?}",
        began.elapsed()
    );
    assert!(fx.finishes().is_empty());
}

#[tokio::test]
async fn ffmpeg_that_stops_making_progress_is_stopped_and_resumed() {
    let mut fx = Fixture::new("stalled", "ok").await;
    fx.stall_timeout = Duration::from_millis(300);
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;

    assert_eq!(fx.ffmpeg_runs(), 3, "the first run and two resumes");
    assert_eq!(fx.file("interrupted").lines().count(), 3);
    assert_eq!(fx.finishes()[0]["failure_code"], "connection_lost");
}

#[tokio::test]
async fn a_run_that_ends_well_before_the_end_of_the_file_is_not_done() {
    let mut fx = Fixture::new("early_end", "ok").await;
    fx.queue_with("YOUTUBE", LABEL, 60_000, MEDIA_URL);
    fx.run_until(|s| !s.finishes.is_empty()).await;

    assert_eq!(fx.ffmpeg_runs(), 3, "resumed from where it ended");
    let finish = &fx.finishes()[0];
    assert_eq!(finish["failure_code"], "connection_lost");
    assert_eq!(finish["streamed_ms"], 3000, "the measured position");
    let argv = fx.file("argv.log");
    assert!(argv.lines().nth(1).unwrap().contains(" -ss 1.000 -i "));
}

#[tokio::test]
async fn output_that_is_not_utf8_never_blocks_ffmpeg() {
    let mut fx = Fixture::new("noisy", "ok").await;
    fx.queue("YOUTUBE", LABEL);
    fx.run_until(|s| !s.finishes.is_empty()).await;
    assert!(fx.finishes()[0].get("failure_code").is_none());
    assert_eq!(fx.ffmpeg_runs(), 1);
}

#[tokio::test]
async fn a_file_address_off_the_allowed_hosts_is_never_opened() {
    for url in [
        "https://evil.example/files/lesson.mp4?X-Amz-Signature=abcdefgh",
        "http://store.example/files/lesson.mp4",
        "https://store.example@evil.example/lesson.mp4",
    ] {
        let mut fx = Fixture::new("ok", "ok").await;
        fx.queue_with("YOUTUBE", LABEL, 3000, url);
        fx.run_until(|s| !s.finishes.is_empty()).await;
        assert_eq!(fx.finishes()[0]["failure_code"], "stream_failed", "{url}");
        assert_eq!(fx.ffmpeg_runs(), 0);
        assert!(fx.file("probe-argv.log").is_empty(), "{url}");
    }
}

/// Starts the real binary against the fake site with `extra` in its environment, waits for the
/// stream to finish and returns the address ffmpeg was told to send to.
async fn output_address_of_the_binary(extra: &[(&str, &str)]) -> String {
    let fx = Fixture::new("ok", "ok").await;
    let token_file = fx.dir.path().join("runner-token");
    let key_file = fx.dir.path().join("runner-key.pem");
    fs::write(&token_file, TOKEN).unwrap();
    fs::write(&key_file, &fx.private_pem).unwrap();
    fx.queue("YOUTUBE", LABEL);
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_stream-runner"));
    command
        .env_clear()
        .env("STREAM_API_BASE_URL", format!("http://{}/api/v4", fx.addr))
        .env("STREAM_RUNNER_TOKEN_FILE", &token_file)
        .env("STREAM_RUNNER_KEY_FILE", &key_file)
        .env("STREAM_MEDIA_HOST", "store.example")
        .env("STREAM_FFMPEG", fx.dir.path().join("ffmpeg"))
        .env("STREAM_FFPROBE", fx.dir.path().join("ffprobe"))
        .env("STREAM_POLL_INTERVAL_SECS", "1")
        .envs(extra.iter().copied())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    for _ in 0..1000 {
        if !fx.state.lock().unwrap().finishes.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    child.kill().await.unwrap();
    assert!(
        !fx.state.lock().unwrap().finishes.is_empty(),
        "the binary did not stream"
    );
    let argv = fx.file("argv.log");
    argv.split_whitespace().last().unwrap().to_owned()
}

#[tokio::test]
async fn the_destination_is_the_platforms_constant() {
    let address = output_address_of_the_binary(&[]).await;
    assert_eq!(address, OUTPUT_URL);
}

/// Without the feature the override variable is not read at all.
#[cfg(not(feature = "test-ingest"))]
#[tokio::test]
async fn the_default_build_ignores_the_test_ingest_variable() {
    let address =
        output_address_of_the_binary(&[("STREAM_TEST_INGEST_URL", "rtmp://127.0.0.1:19350/live")])
            .await;
    assert_eq!(address, OUTPUT_URL);
}

#[cfg(feature = "test-ingest")]
#[tokio::test]
async fn the_test_ingest_feature_sends_to_a_local_listener() {
    let address =
        output_address_of_the_binary(&[("STREAM_TEST_INGEST_URL", "rtmp://127.0.0.1:19350/live")])
            .await;
    assert_eq!(address, format!("rtmp://127.0.0.1:19350/live/{KEY}"));
}
