//! `RecordingReader` against a fake site and storage: the token goes on the first hop only, the
//! 302 to storage is followed, a cut download resumes with the newest token, and every cap holds.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::StreamExt;
use serde_json::json;
use stream_common::key::Secret;
use stream_render::native::reader::RecordingReader;
use tokio::net::TcpListener;
use tokio::sync::watch;

const CLASS: &str = "cccccccccccccccccccccccccc";
const DECK: &str = "dddddddddddddddddddddddddd";
const FILE: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

#[derive(Default)]
struct Site {
    /// (path, authorization, range) of every request.
    seen: Vec<(String, Option<String>, Option<String>)>,
    manifest: serde_json::Value,
    /// How storage answers a file: `Normal`, a body cut after 10 bytes on the first plain read,
    /// the whole file for any range, or more bytes than the file has.
    mode: Mode,
    cut_done: bool,
    token_after_cut: Option<watch::Sender<Option<Secret>>>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Mode {
    #[default]
    Normal,
    CutOnce,
    IgnoreRange,
    TooLong,
    Loop,
}

type State_ = Arc<Mutex<Site>>;

fn note(site: &State_, path: &str, headers: &HeaderMap) {
    let get = |name| headers.get(name).map(|v| v.to_str().unwrap().to_owned());
    site.lock().unwrap().seen.push((
        path.to_owned(),
        get(header::AUTHORIZATION),
        get(header::RANGE),
    ));
}

async fn manifest(State(site): State<State_>, headers: HeaderMap) -> Response {
    note(&site, "manifest", &headers);
    let body = site.lock().unwrap().manifest.to_string();
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

async fn file(State(site): State<State_>, Path(n): Path<u32>, headers: HeaderMap) -> Response {
    note(&site, &format!("files/{n}"), &headers);
    let location = if site.lock().unwrap().mode == Mode::Loop {
        "/api/v4/lms/recordings/rec1/files/1".to_owned()
    } else {
        format!("/storage/{n}?sig=SIGNATURE")
    };
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

async fn storage(State(site): State<State_>, Path(n): Path<u32>, headers: HeaderMap) -> Response {
    note(&site, &format!("storage/{n}"), &headers);
    let (mode, cut_done) = {
        let s = site.lock().unwrap();
        (s.mode, s.cut_done)
    };
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.strip_suffix('-'))
        .and_then(|v| v.parse::<usize>().ok());
    if mode == Mode::CutOnce && !cut_done {
        let mut s = site.lock().unwrap();
        s.cut_done = true;
        if let Some(tx) = &s.token_after_cut {
            tx.send(Some(Secret::new("fresh-token"))).unwrap();
        }
        let first = bytes::Bytes::from_static(&FILE[..10]);
        // A pause between the pieces, so the client has the first one before the cut.
        let stream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(first),
            Err(std::io::Error::other("cut")),
        ])
        .then(|piece| async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            piece
        });
        return Response::builder()
            .header(header::CONTENT_LENGTH, FILE.len())
            .body(Body::from_stream(stream))
            .unwrap();
    }
    if mode == Mode::TooLong {
        let mut long = FILE.to_vec();
        long.extend_from_slice(b"EXTRA");
        return long.into_response();
    }
    match range {
        Some(start) if mode != Mode::IgnoreRange => (
            StatusCode::PARTIAL_CONTENT,
            [(
                header::CONTENT_RANGE,
                format!("bytes {start}-{}/{}", FILE.len() - 1, FILE.len()),
            )],
            FILE[start..].to_vec(),
        )
            .into_response(),
        _ => FILE.to_vec().into_response(),
    }
}

async fn board(State(site): State<State_>, headers: HeaderMap) -> Response {
    note(&site, "board", &headers);
    b"BOARDLOG".to_vec().into_response()
}

async fn deck_page(
    State(site): State<State_>,
    Path((class, deck, n)): Path<(String, String, u32)>,
    headers: HeaderMap,
) -> Response {
    note(&site, &format!("deck/{class}/{deck}/{n}"), &headers);
    b"PICTURE".to_vec().into_response()
}

struct Fixture {
    site: State_,
    base: String,
    reader: RecordingReader,
    tokens: watch::Sender<Option<Secret>>,
}

async fn fixture(manifest: serde_json::Value, mode: Mode) -> Fixture {
    let site: State_ = Arc::new(Mutex::new(Site {
        manifest,
        mode,
        ..Default::default()
    }));
    let app = Router::new()
        .route("/api/v4/lms/recordings/rec1/manifest", get(manifest_))
        .route("/api/v4/lms/recordings/rec1/files/{n}", get(file))
        .route("/storage/{n}", get(storage))
        .route("/board.gz", get(board))
        .route(
            "/api/v4/lms/classes/{class}/decks/{deck}/pages/{n}",
            get(deck_page),
        )
        .with_state(site.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (tokens, rx) = watch::channel(None);
    site.lock().unwrap().token_after_cut = Some(tokens.clone());
    let reader = RecordingReader::new(
        &format!("{base}/api/v4"),
        "rec1",
        Secret::new("first-token"),
        rx,
        3,
        Duration::from_millis(5),
    )
    .unwrap();
    Fixture {
        site,
        base,
        reader,
        tokens,
    }
}

use manifest as manifest_;

fn plain_manifest(base: &str) -> serde_json::Value {
    json!({"duration_ms": 1000, "teacher_id": "t", "class_id": CLASS,
           "board_url": format!("{base}/board.gz"), "files": []})
}

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[tokio::test]
async fn the_manifest_is_read_with_the_token_and_the_board_from_storage_without_it() {
    let f = fixture(json!({}), Mode::Normal).await;
    f.site.lock().unwrap().manifest = plain_manifest(&f.base);
    let manifest = f.reader.manifest().await.unwrap();
    assert_eq!(manifest["duration_ms"], 1000);
    assert_eq!(f.reader.board_log(None).await.unwrap(), b"BOARDLOG");
    let seen = f.site.lock().unwrap().seen.clone();
    assert_eq!(seen[0].1.as_deref(), Some("Bearer first-token"));
    let board = seen.iter().find(|s| s.0 == "board").unwrap();
    assert_eq!(board.1, None, "storage must not see the token");
}

#[tokio::test]
async fn a_board_address_with_another_scheme_is_dropped_and_the_file_is_read() {
    let f = fixture(json!({}), Mode::Normal).await;
    f.site.lock().unwrap().manifest = json!({"board_url": "ftp://x/y", "files": []});
    f.reader.manifest().await.unwrap();
    assert_eq!(f.reader.board_log(Some(4)).await.unwrap(), FILE);
    assert!(f.reader.board_log(None).await.is_err());
}

#[tokio::test]
async fn a_file_follows_the_redirect_without_the_token() {
    let f = fixture(json!({"files": []}), Mode::Normal).await;
    let d = dir();
    let dest = d.path().join("1.webm");
    f.reader
        .download_file(1, FILE.len() as u64, &dest)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), FILE);
    let seen = f.site.lock().unwrap().seen.clone();
    assert_eq!(seen[0].1.as_deref(), Some("Bearer first-token"));
    assert_eq!(seen[1].0, "storage/1");
    assert_eq!(seen[1].1, None);
    assert!(!d.path().join("1.part").exists());
}

#[tokio::test]
async fn a_cut_download_resumes_by_range_with_the_newest_token() {
    let f = fixture(json!({"files": []}), Mode::CutOnce).await;
    let d = dir();
    let dest = d.path().join("2.webm");
    f.reader
        .download_file(2, FILE.len() as u64, &dest)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), FILE);
    let seen = f.site.lock().unwrap().seen.clone();
    let resumed = seen.iter().rfind(|s| s.0 == "files/2").unwrap();
    assert_eq!(resumed.1.as_deref(), Some("Bearer fresh-token"));
    assert_eq!(resumed.2.as_deref(), Some("bytes=10-"));
    drop(f.tokens);
}

#[tokio::test]
async fn an_answer_that_ignored_the_range_starts_the_file_over() {
    let f = fixture(json!({"files": []}), Mode::CutOnce).await;
    f.site.lock().unwrap().mode = Mode::CutOnce;
    // After the cut, storage ignores ranges.
    let d = dir();
    let dest = d.path().join("3.webm");
    let site = f.site.clone();
    let task = f.reader.download_file(3, FILE.len() as u64, &dest);
    // Flip the mode once the cut has happened.
    let flip = async {
        loop {
            if site.lock().unwrap().cut_done {
                site.lock().unwrap().mode = Mode::IgnoreRange;
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    let (result, ()) = tokio::join!(task, flip);
    result.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), FILE);
}

#[tokio::test]
async fn more_or_fewer_bytes_than_the_manifest_say_are_refused() {
    let f = fixture(json!({"files": []}), Mode::TooLong).await;
    let d = dir();
    let dest = d.path().join("4.webm");
    assert!(
        f.reader
            .download_file(4, FILE.len() as u64, &dest)
            .await
            .is_err()
    );
    assert!(!dest.exists());
    let f = fixture(json!({"files": []}), Mode::Normal).await;
    assert!(
        f.reader
            .download_file(5, FILE.len() as u64 + 1, &d.path().join("5.webm"))
            .await
            .is_err()
    );
    assert!(
        f.reader
            .download_file(6, 0, &d.path().join("6.webm"))
            .await
            .is_err()
    );
    assert!(
        f.reader
            .download_file(7, (1 << 30) + 1, &d.path().join("7.webm"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn more_than_three_redirects_are_refused() {
    let f = fixture(json!({"files": []}), Mode::Loop).await;
    let d = dir();
    assert!(
        f.reader
            .download_file(1, 10, &d.path().join("8.webm"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_deck_page_is_read_only_for_the_class_the_manifest_named() {
    let f = fixture(json!({}), Mode::Normal).await;
    assert!(
        f.reader.deck_page(DECK, 1).await.is_err(),
        "no manifest yet"
    );
    f.site.lock().unwrap().manifest = plain_manifest(&f.base);
    f.reader.manifest().await.unwrap();
    assert_eq!(f.reader.deck_page(DECK, 1).await.unwrap(), b"PICTURE");
    assert!(f.reader.deck_page("../x", 1).await.is_err());
}

#[tokio::test]
async fn an_oversized_or_listing_too_many_files_manifest_is_refused() {
    let many: Vec<_> = (0..20_001).map(|i| json!({"index": i})).collect();
    let f = fixture(json!({"files": many}), Mode::Normal).await;
    assert!(f.reader.manifest().await.is_err());
    let f = fixture(json!({"pad": "x".repeat(9 << 20)}), Mode::Normal).await;
    assert!(f.reader.manifest().await.is_err());
}

#[tokio::test]
async fn an_unreachable_site_is_an_error_that_names_no_address() {
    let (_tx, rx) = watch::channel(None);
    let reader = RecordingReader::new(
        "http://127.0.0.1:9/api/v4",
        "rec1",
        Secret::new("tok"),
        rx,
        2,
        Duration::from_millis(1),
    )
    .unwrap();
    let error = reader.manifest().await.unwrap_err().to_string();
    assert!(
        !error.contains("127.0.0.1") && !error.contains("tok"),
        "{error}"
    );
}
