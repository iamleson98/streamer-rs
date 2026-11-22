//! The fake site's HTTP side: the reads of one recording and the render lease's heartbeat.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use tokio::net::TcpListener;

use crate::recording::{CLASS_ID, Recording};

/// A request the site saw.
#[derive(Debug, Clone)]
pub struct Request {
    pub path: String,
    pub authorization: Option<String>,
    pub range: Option<String>,
}

#[derive(Debug)]
struct State_ {
    recording: Recording,
    seen: Mutex<Vec<Request>>,
    heartbeats: Mutex<usize>,
}

type Shared = State<Arc<State_>>;

/// A running fake site.
#[derive(Debug)]
pub struct Server {
    /// The API root (`http://127.0.0.1:port/api/v4`).
    pub api: String,
    state: Arc<State_>,
}

impl Server {
    /// Every request so far.
    pub fn requests(&self) -> Vec<Request> {
        self.state
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The tokens the heartbeat handed out are `fresh-token-N`; the one a claim starts with is
    /// whatever the caller chose.
    pub fn heartbeats(&self) -> usize {
        *self
            .state
            .heartbeats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

fn note(state: &State_, path: &str, headers: &HeaderMap) {
    let get = |name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    state
        .seen
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Request {
            path: path.to_owned(),
            authorization: get(header::AUTHORIZATION),
            range: get(header::RANGE),
        });
}

async fn manifest(State(s): Shared, headers: HeaderMap) -> Response {
    note(&s, "manifest", &headers);
    axum::Json(s.recording.manifest()).into_response()
}

async fn file(
    State(s): Shared,
    Path((_, n)): Path<(String, usize)>,
    headers: HeaderMap,
) -> Response {
    note(&s, &format!("files/{n}"), &headers);
    if n >= s.recording.files.len() {
        return StatusCode::NOT_FOUND.into_response();
    }
    (
        StatusCode::FOUND,
        [(header::LOCATION, format!("/storage/{n}?sig=fake-signature"))],
    )
        .into_response()
}

async fn storage(State(s): Shared, Path(n): Path<usize>, headers: HeaderMap) -> Response {
    note(&s, &format!("storage/{n}"), &headers);
    let Some(file) = s.recording.files.get(n) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(bytes) = std::fs::read(&file.path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let total = bytes.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split_once('-'))
        .and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok())));
    let accept = [(header::ACCEPT_RANGES, "bytes".to_owned())];
    match range {
        Some((start, end)) if start < total => {
            let end = end.map_or(total - 1, |e| e.min(total - 1));
            (
                StatusCode::PARTIAL_CONTENT,
                accept,
                [(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total}"),
                )],
                bytes[start..=end].to_vec(),
            )
                .into_response()
        }
        _ => (accept, bytes).into_response(),
    }
}

async fn deck_page(
    State(s): Shared,
    Path((_, deck, n)): Path<(String, String, u32)>,
    headers: HeaderMap,
) -> Response {
    note(&s, &format!("deck/{deck}/{n}"), &headers);
    match s.recording.pictures.get(&(deck, n)) {
        Some(png) => ([(header::CONTENT_TYPE, "image/png")], png.to_vec()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn heartbeat(State(s): Shared, headers: HeaderMap) -> Response {
    note(&s, "heartbeat", &headers);
    let n = {
        let mut count = s.heartbeats.lock().unwrap_or_else(|e| e.into_inner());
        *count += 1;
        *count
    };
    axum::Json(json!({
        "lease_until": 1, "stop": false,
        "render_token": format!("fresh-token-{n}"), "render_token_expires_at": 2,
    }))
    .into_response()
}

/// Starts the site for `recording` on a free local port.
pub async fn serve(recording: Recording) -> std::io::Result<Server> {
    let state = Arc::new(State_ {
        recording,
        seen: Mutex::default(),
        heartbeats: Mutex::default(),
    });
    let app = Router::new()
        .route("/api/v4/lms/recordings/{id}/manifest", get(manifest))
        .route("/api/v4/lms/recordings/{id}/files/{n}", get(file))
        .route("/storage/{n}", get(storage))
        .route(
            &format!("/api/v4/lms/classes/{CLASS_ID}/decks/{{deck}}/pages/{{n}}"),
            get(
                |s: Shared, Path((deck, n)): Path<(String, u32)>, h: HeaderMap| {
                    deck_page(s, Path((CLASS_ID.to_owned(), deck, n)), h)
                },
            ),
        )
        .route(
            "/api/v4/lms/stream_worker/render/{id}/heartbeat",
            post(heartbeat),
        )
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(Server {
        api: format!("http://{addr}/api/v4"),
        state,
    })
}
