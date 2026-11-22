//! What the render page reads from the worker's local server besides the sink: the built web app
//! (the page and its assets) and the site's API, reached through a reverse proxy.
//!
//! The proxy exists so the browser only ever talks to `127.0.0.1`: the page's reads of the
//! recording (`manifest` and ranged `files/{n}`) carry the render token in an `Authorization`
//! header, the proxy adds the site's address and follows the API's redirect to storage itself, so
//! the answer the page gets has the file's bytes, not a cross-origin redirect. It forwards no
//! cookie, and it serves nothing but these reads of the one recording being rendered:
//!
//! - `manifest` and `files/{n}` of the recording;
//! - the pictures of the imported deck pages on its board, `GET
//!   /api/v4/lms/classes/{class}/decks/{deck}/pages/{n}`, only for the class the manifest named;
//! - `/__board`: the board log read straight from storage. The manifest may carry a presigned
//!   storage address (`board_url`) that storage would not answer a cross-origin read of from
//!   `127.0.0.1`, so the proxy rewrites it to `/__board` in the manifest it passes on, remembers
//!   the address, and serves `/__board` by fetching exactly that address: no other, no token, no
//!   redirect followed.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use reqwest::Url;
use stream_common::api::is_site_id;

use crate::recording::{self, Fetch, MAX_MANIFEST_BYTES, digits, read_capped};
use tokio_util::io::ReaderStream;

/// Where the page is told to read the board log.
pub const BOARD_PATH: &str = "/__board";

/// The response headers passed back. Nothing else crosses: no cookie, no cache directive of the
/// site's, no address.
const FORWARD_RESPONSE: [HeaderName; 7] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_RANGE,
    header::ACCEPT_RANGES,
    header::ETAG,
    header::LAST_MODIFIED,
    header::CACHE_CONTROL,
];

/// What a request path maps to.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// The recording's manifest, which is rewritten on its way through.
    Manifest(String),
    /// A read the site answers (a file of the recording, a deck page picture).
    Site(String),
    /// The board log at the address the manifest gave.
    Board,
}

/// What the manifest said, kept for the reads it points to.
#[derive(Debug, Default)]
struct ManifestFacts {
    board: Option<Url>,
    class_id: Option<String>,
}

/// Reads of one recording through the site's API.
#[derive(Debug)]
pub struct ApiProxy {
    http: reqwest::Client,
    /// The API root, such as `https://host/api/v4`.
    upstream: String,
    recording_id: String,
    facts: Mutex<ManifestFacts>,
}

impl ApiProxy {
    pub fn new(upstream: &str, recording_id: &str) -> Result<Arc<Self>, reqwest::Error> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()?;
        Ok(Arc::new(Self {
            http,
            upstream: upstream.trim_end_matches('/').to_owned(),
            recording_id: recording_id.to_owned(),
            facts: Mutex::default(),
        }))
    }

    fn facts(&self) -> std::sync::MutexGuard<'_, ManifestFacts> {
        self.facts.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The read a path asks for, or `None` for every path the proxy does not serve.
    fn route(&self, path: &str, query: Option<&str>) -> Option<Route> {
        if query.is_some() {
            return None;
        }
        if path == BOARD_PATH {
            return Some(Route::Board);
        }
        if let Some(rest) = path.strip_prefix("/api/v4/lms/classes/") {
            return self.deck_page(rest).map(Route::Site);
        }
        let rest = path.strip_prefix("/api/v4/lms/recordings/")?;
        let (id, tail) = rest.split_once('/')?;
        if id != self.recording_id {
            return None;
        }
        let url = format!("{}/lms/recordings/{id}/{tail}", self.upstream);
        if tail == "manifest" {
            return Some(Route::Manifest(url));
        }
        let file = tail.strip_prefix("files/")?;
        digits(file, 6).then_some(Route::Site(url))
    }

    /// `{class}/decks/{deck}/pages/{n}` of the class the manifest named.
    fn deck_page(&self, rest: &str) -> Option<String> {
        let mut parts = rest.split('/');
        let (class, decks, deck, pages, n) = (
            parts.next()?,
            parts.next()?,
            parts.next()?,
            parts.next()?,
            parts.next()?,
        );
        let known = self.facts().class_id.as_deref() == Some(class);
        let ok = parts.next().is_none()
            && known
            && decks == "decks"
            && pages == "pages"
            && is_site_id(deck)
            && digits(n, 3);
        ok.then(|| {
            format!(
                "{}/lms/classes/{class}/decks/{deck}/pages/{n}",
                self.upstream
            )
        })
    }

    pub async fn handle(State(proxy): State<Arc<Self>>, request: Request) -> Response {
        let method = request.method().clone();
        if method != Method::GET && method != Method::HEAD {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let Some(route) = proxy.route(request.uri().path(), request.uri().query()) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let headers = request.headers();
        let answered = match route {
            Route::Manifest(url) => proxy.manifest(method, &url, headers).await,
            Route::Site(url) => proxy
                .fetch(&method, &url, headers, Fetch::Site)
                .await
                .map(|upstream| into_response(upstream, &method)),
            Route::Board => {
                let Some(url) = proxy.facts().board.clone() else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                proxy
                    .fetch(&method, url.as_str(), headers, Fetch::Storage)
                    .await
                    .map(|upstream| into_response(upstream, &method))
            }
        };
        answered.unwrap_or_else(|| StatusCode::BAD_GATEWAY.into_response())
    }

    /// Passes the manifest on with its storage address for the board log replaced by
    /// [`BOARD_PATH`], and remembers what it said. A refusal or an error of the site is passed on as
    /// it is.
    async fn manifest(&self, method: Method, url: &str, incoming: &HeaderMap) -> Option<Response> {
        let upstream = self.fetch(&method, url, incoming, Fetch::Site).await?;
        if method != Method::GET || !upstream.status().is_success() {
            return Some(into_response(upstream, &method));
        }
        let status = upstream.status();
        let body = read_capped(upstream, MAX_MANIFEST_BYTES).await?;
        let rewritten = self.rewrite_manifest(&body)?;
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(rewritten))
            .ok()
    }

    /// Reads `class_id` and `board_url` out of the manifest. The board address is kept and replaced
    /// by [`BOARD_PATH`]; one that is not a plain http(s) address of the API's scheme is dropped, so
    /// the page falls back to the recording's board file.
    fn rewrite_manifest(&self, body: &[u8]) -> Option<Vec<u8>> {
        let mut manifest: serde_json::Value = serde_json::from_slice(body).ok()?;
        let object = manifest.as_object_mut()?;
        let mut facts = self.facts();
        facts.class_id = object
            .get("class_id")
            .and_then(|v| v.as_str())
            .filter(|id| is_site_id(id))
            .map(str::to_owned);
        facts.board = None;
        if let Some(address) = object.remove("board_url") {
            let storage = recording::storage_address(&address, &self.upstream);
            if let Some(storage) = storage {
                facts.board = Some(storage);
                object.insert("board_url".to_owned(), BOARD_PATH.into());
            }
        }
        serde_json::to_vec(&manifest).ok()
    }

    async fn fetch(
        &self,
        method: &Method,
        first: &str,
        incoming: &HeaderMap,
        how: Fetch,
    ) -> Option<reqwest::Response> {
        recording::fetch(&self.http, method, first, incoming, how).await
    }
}

fn into_response(upstream: reqwest::Response, method: &Method) -> Response {
    let mut response = Response::builder().status(upstream.status());
    for name in &FORWARD_RESPONSE {
        if let Some(value) = upstream.headers().get(name) {
            response = response.header(name, value);
        }
    }
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from_stream(upstream.bytes_stream())
    };
    response
        .body(body)
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// The built web app: files under one directory, and `/render` as the client-only route it is.
#[derive(Debug)]
pub struct WebApp {
    dir: PathBuf,
}

impl WebApp {
    pub fn new(dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.to_owned(),
        })
    }

    /// The file a request path names: only plain, relative, decoded-free segments inside the
    /// directory. `/render` (the route has no file of its own) is the SvelteKit fallback page.
    fn resolve(&self, path: &str) -> Option<PathBuf> {
        if path == "/render" || path == "/render/" {
            return Some(self.dir.join("200.html"));
        }
        let relative = Path::new(path.strip_prefix('/')?);
        let mut out = self.dir.clone();
        for component in relative.components() {
            match component {
                Component::Normal(part) => {
                    let part = part.to_str()?;
                    if part.contains(['\\', '%', '\0']) {
                        return None;
                    }
                    out.push(part);
                }
                _ => return None,
            }
        }
        (out != self.dir).then_some(out)
    }

    pub async fn handle(State(app): State<Arc<Self>>, request: Request) -> Response {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let Some(path) = app.resolve(request.uri().path()) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(file) = tokio::fs::File::open(&path).await else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(meta) = file.metadata().await else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if !meta.is_file() {
            return StatusCode::NOT_FOUND.into_response();
        }
        let body = if request.method() == Method::HEAD {
            Body::empty()
        } else {
            Body::from_stream(ReaderStream::with_capacity(file, 64 << 10))
        };
        let mut response = Response::new(body);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(content_type(&path)),
        );
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
        response
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "webmanifest") => "application/json",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::extract::Path as UrlPath;
    use axum::routing::get;
    use tokio::net::TcpListener;

    async fn serve(app: Router) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    const FILE: &[u8] = b"0123456789abcdefghij";

    /// A fake site (the manifest, and a file that redirects to storage) and a fake storage that
    /// answers ranges and records what it was sent.
    async fn fake_site() -> (String, Arc<std::sync::Mutex<Vec<HeaderMap>>>) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<HeaderMap>::new()));
        let storage = {
            let seen = seen.clone();
            serve(Router::new().route(
                "/object",
                get(move |headers: HeaderMap| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().unwrap().push(headers.clone());
                        let range = headers
                            .get(header::RANGE)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|r| r.strip_prefix("bytes="))
                            .and_then(|r| r.split_once('-'))
                            .and_then(|(a, b)| {
                                Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                            });
                        match range {
                            Some((a, b)) => Response::builder()
                                .status(StatusCode::PARTIAL_CONTENT)
                                .header(
                                    header::CONTENT_RANGE,
                                    format!("bytes {a}-{b}/{}", FILE.len()),
                                )
                                .header(header::CONTENT_TYPE, "video/webm")
                                .header(header::SET_COOKIE, "storage=1")
                                .body(Body::from(FILE[a..=b].to_vec()))
                                .unwrap(),
                            None => Response::new(Body::from(FILE.to_vec())),
                        }
                    }
                }),
            ))
            .await
        };
        let site_seen = seen.clone();
        let site = serve(
            Router::new()
                .route(
                    "/api/v4/lms/recordings/{id}/manifest",
                    get(|headers: HeaderMap| async move {
                        if headers
                            .get(header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            == Some("Bearer tok")
                        {
                            Response::builder()
                                .header(header::CONTENT_TYPE, "application/json")
                                .header(header::SET_COOKIE, "session=1")
                                .body(Body::from(r#"{"files":[]}"#))
                                .unwrap()
                        } else {
                            StatusCode::UNAUTHORIZED.into_response()
                        }
                    }),
                )
                .route(
                    "/api/v4/lms/recordings/{id}/files/{n}",
                    get(
                        move |UrlPath((_, n)): UrlPath<(String, String)>, headers: HeaderMap| {
                            let site_seen = site_seen.clone();
                            async move {
                                site_seen.lock().unwrap().push(headers);
                                if n == "9" {
                                    return StatusCode::NOT_FOUND.into_response();
                                }
                                Response::builder()
                                    .status(StatusCode::FOUND)
                                    .header(
                                        header::LOCATION,
                                        format!("http://{storage}/object?sig=SECRET"),
                                    )
                                    .body(Body::empty())
                                    .unwrap()
                            }
                        },
                    ),
                ),
        )
        .await;
        (format!("http://{site}/api/v4"), seen)
    }

    async fn proxy_server(upstream: &str) -> String {
        let proxy = ApiProxy::new(upstream, "rec1").unwrap();
        let router = Router::new().fallback(ApiProxy::handle).with_state(proxy);
        format!("http://{}", serve(router).await)
    }

    #[tokio::test]
    async fn the_manifest_passes_with_the_token_and_without_cookies() {
        let (upstream, _) = fake_site().await;
        let base = proxy_server(&upstream).await;
        let client = reqwest::Client::new();
        let res = client
            .get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .header("Authorization", "Bearer tok")
            .header("Cookie", "mine=1")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert!(res.headers().get("set-cookie").is_none());
        assert_eq!(res.text().await.unwrap(), r#"{"files":[]}"#);
        let res = client
            .get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401, "the site's answer is passed on");
    }

    #[tokio::test]
    async fn a_file_read_follows_the_redirect_and_keeps_the_range() {
        let (upstream, seen) = fake_site().await;
        let base = proxy_server(&upstream).await;
        let res = reqwest::Client::new()
            .get(format!("{base}/api/v4/lms/recordings/rec1/files/0"))
            .header("Authorization", "Bearer tok")
            .header("Range", "bytes=5-9")
            .header("Cookie", "mine=1")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 206);
        assert_eq!(res.headers()["content-range"], "bytes 5-9/20");
        assert_eq!(res.headers()["content-type"], "video/webm");
        assert!(res.headers().get("set-cookie").is_none());
        assert_eq!(res.bytes().await.unwrap().as_ref(), b"56789");

        let seen = seen.lock().unwrap();
        let (site_hop, storage_hop) = (&seen[0], &seen[1]);
        assert_eq!(site_hop["authorization"], "Bearer tok");
        assert!(site_hop.get("cookie").is_none());
        assert_eq!(storage_hop["range"], "bytes=5-9");
        assert!(
            storage_hop.get("authorization").is_none(),
            "storage never sees the token"
        );
        assert!(storage_hop.get("cookie").is_none());
    }

    #[tokio::test]
    async fn a_missing_file_is_the_sites_404() {
        let (upstream, _) = fake_site().await;
        let base = proxy_server(&upstream).await;
        let res = reqwest::get(format!("{base}/api/v4/lms/recordings/rec1/files/9"))
            .await
            .unwrap();
        assert_eq!(res.status(), 404);
    }

    #[tokio::test]
    async fn every_other_path_and_method_is_refused() {
        let (upstream, seen) = fake_site().await;
        let base = proxy_server(&upstream).await;
        let client = reqwest::Client::new();
        for path in [
            "/api/v4/lms/recordings/other/manifest",
            "/api/v4/lms/recordings/rec1/media",
            "/api/v4/lms/recordings/rec1/files/x",
            "/api/v4/lms/recordings/rec1/files/",
            "/api/v4/lms/recordings/rec1/files/1/2",
            "/api/v4/lms/recordings/rec1/manifest?x=1",
            "/api/v4/lms/stream_worker/render/claim",
            "/api/v4/users/me",
            "/api/v4/lms/recordings/rec1/../rec2/manifest",
            "/api/v4/lms/recordings/",
            "/api/v4",
        ] {
            let res = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(res.status(), 404, "{path}");
        }
        for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
            let res = client
                .request(
                    method.clone(),
                    format!("{base}/api/v4/lms/recordings/rec1/manifest"),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 405, "{method}");
        }
        assert!(seen.lock().unwrap().is_empty(), "nothing reached the site");
    }

    #[tokio::test]
    async fn an_unreachable_site_is_a_bad_gateway() {
        let base = proxy_server("http://127.0.0.1:1/api/v4").await;
        let res = reqwest::get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .await
            .unwrap();
        assert_eq!(res.status(), 502);
    }

    const CLASS: &str = "classaaaaaaaaaaaaaaaaaaaaa";
    const DECK: &str = "deckaaaaaaaaaaaaaaaaaaaaaa";

    /// What a fake storage was asked for: (path with query, headers).
    type Asked = Arc<std::sync::Mutex<Vec<(String, HeaderMap)>>>;

    /// A fake site whose manifest names a class and a board log in a fake storage, with a deck page
    /// that redirects to the storage too. `board_url` is what the manifest says (`{storage}` is
    /// replaced by the storage's address).
    async fn board_site(board_url: Option<&'static str>) -> (String, String, Asked) {
        let asked = Asked::default();
        let storage = {
            let asked = asked.clone();
            serve(Router::new().fallback(move |request: Request| {
                let asked = asked.clone();
                async move {
                    let target = request
                        .uri()
                        .path_and_query()
                        .map(|p| p.to_string())
                        .unwrap_or_default();
                    asked
                        .lock()
                        .unwrap()
                        .push((target.clone(), request.headers().clone()));
                    if target.starts_with("/board") {
                        let range = request
                            .headers()
                            .get(header::RANGE)
                            .is_some_and(|r| r == "bytes=0-3");
                        if range {
                            return Response::builder()
                                .status(StatusCode::PARTIAL_CONTENT)
                                .header(header::CONTENT_RANGE, "bytes 0-3/20")
                                .body(Body::from(&FILE[..4]))
                                .unwrap();
                        }
                        Response::new(Body::from(FILE.to_vec()))
                    } else if target.starts_with("/bounce") {
                        Response::builder()
                            .status(StatusCode::FOUND)
                            .header(header::LOCATION, "/board?sig=OTHER")
                            .body(Body::empty())
                            .unwrap()
                    } else if target.starts_with("/picture") {
                        Response::builder()
                            .header(header::CONTENT_TYPE, "image/webp")
                            .body(Body::from("PIC"))
                            .unwrap()
                    } else {
                        StatusCode::NOT_FOUND.into_response()
                    }
                }
            }))
            .await
        };
        let manifest = board_url.map(|u| u.replace("{storage}", &storage.to_string()));
        let site = serve(
            Router::new()
                .route(
                    "/api/v4/lms/recordings/{id}/manifest",
                    get(move || {
                        let manifest = manifest.clone();
                        async move {
                            let mut body = serde_json::json!({
                                "files": [], "class_id": CLASS,
                            });
                            if let Some(url) = manifest {
                                body["board_url"] = url.into();
                            }
                            axum::Json(body)
                        }
                    }),
                )
                .route(
                    "/api/v4/lms/classes/{class}/decks/{deck}/pages/{n}",
                    get(move || async move {
                        Response::builder()
                            .status(StatusCode::FOUND)
                            .header(header::LOCATION, format!("http://{storage}/picture?sig=P"))
                            .body(Body::empty())
                            .unwrap()
                    }),
                ),
        )
        .await;
        (
            format!("http://{site}/api/v4"),
            format!("http://{storage}"),
            asked,
        )
    }

    #[tokio::test]
    async fn the_manifests_board_address_becomes_a_same_origin_path_served_from_that_address() {
        let (upstream, _, asked) = board_site(Some("http://{storage}/board?sig=SECRET")).await;
        let base = proxy_server(&upstream).await;
        let client = reqwest::Client::new();

        let manifest: serde_json::Value = client
            .get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .header("Authorization", "Bearer tok")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(manifest["board_url"], "/__board");
        assert_eq!(manifest["class_id"], CLASS);
        assert_eq!(manifest["files"], serde_json::json!([]));

        let whole = client
            .get(format!("{base}/__board"))
            .header("Authorization", "Bearer tok")
            .header("Cookie", "mine=1")
            .send()
            .await
            .unwrap();
        assert_eq!(whole.status(), 200);
        assert_eq!(whole.bytes().await.unwrap().as_ref(), FILE);
        let part = client
            .get(format!("{base}/__board"))
            .header("Range", "bytes=0-3")
            .send()
            .await
            .unwrap();
        assert_eq!(part.status(), 206);
        assert_eq!(part.bytes().await.unwrap().as_ref(), b"0123");

        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        for (target, headers) in asked.iter() {
            assert_eq!(
                target, "/board?sig=SECRET",
                "exactly the manifest's address"
            );
            assert!(
                headers.get("authorization").is_none(),
                "no token to storage"
            );
            assert!(headers.get("cookie").is_none());
        }
        assert_eq!(asked[1].1["range"], "bytes=0-3");
    }

    #[tokio::test]
    async fn the_board_path_is_closed_until_a_manifest_names_an_address_and_takes_no_other() {
        let (upstream, _, asked) = board_site(None).await;
        let base = proxy_server(&upstream).await;
        let client = reqwest::Client::new();
        let manifest: serde_json::Value = client
            .get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(manifest.get("board_url").is_none());
        assert_eq!(
            client
                .get(format!("{base}/__board"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        for path in ["/__board?url=http://127.0.0.1:1/", "/__board/x", "/__BOARD"] {
            assert_eq!(
                client
                    .get(format!("{base}{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404,
                "{path}"
            );
        }
        assert!(asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_board_address_that_redirects_or_is_not_the_apis_scheme_is_not_followed() {
        let (upstream, _, asked) = board_site(Some("http://{storage}/bounce?sig=S")).await;
        let base = proxy_server(&upstream).await;
        let client = reqwest::Client::new();
        client
            .get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .send()
            .await
            .unwrap();
        let res = client.get(format!("{base}/__board")).send().await.unwrap();
        assert_eq!(res.status(), 502);
        assert_eq!(
            asked.lock().unwrap().len(),
            1,
            "the redirect was not followed"
        );

        let (upstream, _, _) = board_site(Some("https://{storage}/board")).await;
        let base = proxy_server(&upstream).await;
        let manifest: serde_json::Value = client
            .get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(manifest.get("board_url").is_none(), "{manifest}");
        assert_eq!(
            client
                .get(format!("{base}/__board"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }

    #[tokio::test]
    async fn deck_page_pictures_are_read_for_the_manifests_class_only() {
        let (upstream, _, asked) = board_site(None).await;
        let base = proxy_server(&upstream).await;
        let client = reqwest::Client::new();
        let page = format!("/api/v4/lms/classes/{CLASS}/decks/{DECK}/pages/3");
        assert_eq!(
            client
                .get(format!("{base}{page}"))
                .send()
                .await
                .unwrap()
                .status(),
            404,
            "no class is known before the manifest"
        );
        client
            .get(format!("{base}/api/v4/lms/recordings/rec1/manifest"))
            .send()
            .await
            .unwrap();
        let res = client
            .get(format!("{base}{page}"))
            .header("Authorization", "Bearer tok")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "the redirect is followed");
        assert_eq!(res.headers()["content-type"], "image/webp");
        assert_eq!(res.bytes().await.unwrap().as_ref(), b"PIC");
        {
            let asked = asked.lock().unwrap();
            assert_eq!(asked.len(), 1);
            assert!(asked[0].1.get("authorization").is_none());
        }

        let other = "classbbbbbbbbbbbbbbbbbbbbb";
        for path in [
            format!("/api/v4/lms/classes/{other}/decks/{DECK}/pages/3"),
            format!("/api/v4/lms/classes/{CLASS}/decks/short/pages/3"),
            format!("/api/v4/lms/classes/{CLASS}/decks/{DECK}/pages/x"),
            format!("/api/v4/lms/classes/{CLASS}/decks/{DECK}/pages/1234"),
            format!("/api/v4/lms/classes/{CLASS}/decks/{DECK}/pages/3/more"),
            format!("/api/v4/lms/classes/{CLASS}/decks/{DECK}/pages/3?x=1"),
            format!("/api/v4/lms/classes/{CLASS}/decks/{DECK}"),
            format!("/api/v4/lms/classes/{CLASS}/members"),
            format!("/api/v4/lms/classes/{CLASS}"),
            format!("/api/v4/lms/classes/{CLASS}/decks/{DECK}/pages"),
        ] {
            let res = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(res.status(), 404, "{path}");
        }
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    fn web_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("200.html"), "<html>fallback</html>").unwrap();
        std::fs::create_dir_all(dir.path().join("_app/immutable")).unwrap();
        std::fs::write(dir.path().join("_app/immutable/app.js"), "export {}").unwrap();
        std::fs::write(dir.path().join("_app/immutable/x.wasm"), [0, 97, 115, 109]).unwrap();
        dir
    }

    async fn web_server(dir: &Path) -> String {
        let router = Router::new()
            .fallback(WebApp::handle)
            .with_state(WebApp::new(dir));
        format!("http://{}", serve(router).await)
    }

    #[tokio::test]
    async fn the_render_route_is_the_fallback_page_and_assets_have_their_types() {
        let dir = web_dir();
        let base = web_server(dir.path()).await;
        for path in ["/render", "/render/"] {
            let res = reqwest::get(format!("{base}{path}")).await.unwrap();
            assert_eq!(res.status(), 200, "{path}");
            assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
            assert!(res.headers().get("content-security-policy").is_none());
            assert_eq!(res.text().await.unwrap(), "<html>fallback</html>");
        }
        let js = reqwest::get(format!("{base}/_app/immutable/app.js"))
            .await
            .unwrap();
        assert_eq!(
            js.headers()["content-type"],
            "text/javascript; charset=utf-8"
        );
        let wasm = reqwest::get(format!("{base}/_app/immutable/x.wasm"))
            .await
            .unwrap();
        assert_eq!(wasm.headers()["content-type"], "application/wasm");
        assert_eq!(wasm.headers()["content-length"], "4");
    }

    #[tokio::test]
    async fn nothing_outside_the_directory_or_missing_is_served() {
        let dir = web_dir();
        let base = web_server(dir.path()).await;
        let client = reqwest::Client::new();
        for path in [
            "/",
            "/missing.js",
            "/_app",
            "/_app/immutable",
            "/%2e%2e/outside.txt",
            "/..%2foutside.txt",
            "/_app/%2e%2e/%2e%2e/outside.txt",
            "/courses",
        ] {
            let res = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(res.status(), 404, "{path}");
        }
        let app = WebApp::new(dir.path());
        assert!(app.resolve("/../outside.txt").is_none());
        assert!(app.resolve("//etc/passwd").is_none());
        assert!(app.resolve("/a\\b").is_none());
    }
}
