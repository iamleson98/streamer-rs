//! What the render page reads from the worker's local server besides the sink: the built web app
//! (the page and its assets) and the site's API, reached through a reverse proxy.
//!
//! The proxy exists so the browser only ever talks to `127.0.0.1`: the page's reads of the
//! recording (`manifest` and ranged `files/{n}`) carry the render token in an `Authorization`
//! header, the proxy adds the site's address and follows the API's redirect to storage itself, so
//! the answer the page gets has the file's bytes, not a cross-origin redirect. It forwards no
//! cookie, and it serves nothing but those two reads of the one recording being rendered.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use reqwest::Url;
use tokio_util::io::ReaderStream;

/// Redirects followed on the way to the bytes (the API answers one, to storage).
const MAX_HOPS: usize = 3;

/// The request headers passed on: the render token, the byte range and what the page accepts.
const FORWARD_REQUEST: [HeaderName; 3] = [header::AUTHORIZATION, header::RANGE, header::ACCEPT];

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

/// Reads of one recording through the site's API.
#[derive(Debug)]
pub struct ApiProxy {
    http: reqwest::Client,
    /// The API root, such as `https://host/api/v4`.
    upstream: String,
    recording_id: String,
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
        }))
    }

    /// `/api/v4/lms/recordings/{id}/manifest` and `/api/v4/lms/recordings/{id}/files/{n}` of this
    /// recording map to the site; every other path is `None`.
    fn upstream_url(&self, path: &str, query: Option<&str>) -> Option<String> {
        if query.is_some() {
            return None;
        }
        let rest = path.strip_prefix("/api/v4/lms/recordings/")?;
        let (id, tail) = rest.split_once('/')?;
        if id != self.recording_id {
            return None;
        }
        let allowed = tail == "manifest"
            || tail.strip_prefix("files/").is_some_and(|n| {
                !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit())
            });
        allowed.then(|| format!("{}/lms/recordings/{id}/{tail}", self.upstream))
    }

    pub async fn handle(State(proxy): State<Arc<Self>>, request: Request) -> Response {
        let method = request.method().clone();
        if method != Method::GET && method != Method::HEAD {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let Some(url) = proxy.upstream_url(request.uri().path(), request.uri().query()) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        match proxy.fetch(method, &url, request.headers()).await {
            Some(response) => response,
            None => StatusCode::BAD_GATEWAY.into_response(),
        }
    }

    /// The first hop carries the token; a redirect (to a presigned address) carries only the range.
    async fn fetch(&self, method: Method, first: &str, incoming: &HeaderMap) -> Option<Response> {
        let mut url = Url::parse(first).ok()?;
        for hop in 0..=MAX_HOPS {
            let mut request = self.http.request(method.clone(), url.clone());
            for name in &FORWARD_REQUEST {
                if hop > 0 && *name == header::AUTHORIZATION {
                    continue;
                }
                if let Some(value) = incoming.get(name) {
                    request = request.header(name, value);
                }
            }
            let upstream = request.send().await.ok()?;
            if upstream.status().is_redirection() {
                let location = upstream.headers().get(header::LOCATION)?.to_str().ok()?;
                url = url.join(location).ok()?;
                if !matches!(url.scheme(), "http" | "https") {
                    return None;
                }
                continue;
            }
            return Some(into_response(upstream, &method));
        }
        None
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
