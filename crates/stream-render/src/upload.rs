//! Uploading the finished MP4: the site says where each part goes (presigned PUT URLs), the file is
//! read from disk part by part, straight into the request body, and the ETag each part's answer
//! carries is kept for the finish call. Memory stays at the size of a read buffer, not of a part.

use std::path::Path;
use std::time::Duration;

use stream_common::api::{ApiClient, ApiError, UploadInput, UploadPart, UploadedPart};
use stream_common::redact::{Registered, register_secret};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// The read buffer of a part's body.
const READ_BUFFER: usize = 256 << 10;
/// The longest ETag the site accepts.
const ETAG_MAX_LEN: usize = 128;

/// Why the upload failed. Whatever the cause, the render ends as `render_failed`; the messages carry
/// no address, signature or answer.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UploadError {
    #[error("the site would not say where to upload the file")]
    Plan,
    #[error("the upload plan does not fit the file")]
    BadPlan,
    #[error("the file could not be read")]
    File,
    #[error("storage did not take a part")]
    Storage,
}

/// What the retries wait and how long a part may take.
#[derive(Debug, Clone, Copy)]
pub struct UploadLimits {
    pub attempts: usize,
    pub retry_delay: Duration,
    pub part_timeout: Duration,
}

/// The file's parts as the plan cuts them: `(number, offset, length)`; `None` when the plan does not
/// cover the file exactly.
fn cut(size: u64, part_bytes: i64, parts: &[UploadPart]) -> Option<Vec<(u32, u64, u64)>> {
    let part_bytes = u64::try_from(part_bytes).ok().filter(|b| *b > 0)?;
    if parts.len() as u64 != size.div_ceil(part_bytes) {
        return None;
    }
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let offset = i as u64 * part_bytes;
            (part.number as usize == i + 1)
                .then(|| (part.number, offset, part_bytes.min(size - offset)))
        })
        .collect()
}

/// Uploads `file` and returns its size and the parts with their ETags, in order.
pub async fn upload(
    api: &ApiClient,
    http: &reqwest::Client,
    worker_id: &str,
    request_id: &str,
    file: &Path,
    limits: UploadLimits,
) -> Result<(i64, Vec<UploadedPart>), UploadError> {
    let size = tokio::fs::metadata(file)
        .await
        .map_err(|_| UploadError::File)?
        .len();
    let size_bytes = i64::try_from(size).map_err(|_| UploadError::File)?;
    let input = UploadInput {
        worker_id: worker_id.to_owned(),
        size_bytes,
    };
    let plan = {
        let mut tries = 0;
        loop {
            tries += 1;
            match api.render_uploads(request_id, &input).await {
                Ok(plan) => break plan,
                Err(ApiError::Unavailable) if tries < limits.attempts => {
                    tokio::time::sleep(limits.retry_delay).await;
                }
                Err(_) => return Err(UploadError::Plan),
            }
        }
    };
    let cuts = cut(size, plan.part_bytes, &plan.parts).ok_or(UploadError::BadPlan)?;
    // A presigned address is a secret until the part is in: nothing logged may hold it.
    let _guards: Vec<Registered> = plan
        .parts
        .iter()
        .map(|p| register_secret(p.url.expose()))
        .collect();

    let mut uploaded = Vec::with_capacity(cuts.len());
    for ((number, offset, length), part) in cuts.into_iter().zip(&plan.parts) {
        let etag = put_with_retries(http, part, file, offset, length, limits).await?;
        tracing::info!(part = number, of = plan.parts.len(), "part uploaded");
        uploaded.push(UploadedPart { number, etag });
    }
    Ok((size_bytes, uploaded))
}

async fn put_with_retries(
    http: &reqwest::Client,
    part: &UploadPart,
    file: &Path,
    offset: u64,
    length: u64,
    limits: UploadLimits,
) -> Result<String, UploadError> {
    let mut tries = 0;
    loop {
        tries += 1;
        match put_part(http, part, file, offset, length, limits.part_timeout).await {
            Ok(etag) => return Ok(etag),
            Err(error) if tries >= limits.attempts => return Err(error),
            Err(UploadError::File) => return Err(UploadError::File),
            Err(_) => tokio::time::sleep(limits.retry_delay).await,
        }
    }
}

async fn put_part(
    http: &reqwest::Client,
    part: &UploadPart,
    file: &Path,
    offset: u64,
    length: u64,
    timeout: Duration,
) -> Result<String, UploadError> {
    let mut source = tokio::fs::File::open(file)
        .await
        .map_err(|_| UploadError::File)?;
    source
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|_| UploadError::File)?;
    let body = reqwest::Body::wrap_stream(ReaderStream::with_capacity(
        source.take(length),
        READ_BUFFER,
    ));
    let response = http
        .put(part.url.expose())
        .header(reqwest::header::CONTENT_LENGTH, length)
        .timeout(timeout)
        .body(body)
        .send()
        .await
        .map_err(|_| UploadError::Storage)?;
    if !response.status().is_success() {
        return Err(UploadError::Storage);
    }
    response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|etag| !etag.is_empty() && etag.len() <= ETAG_MAX_LEN)
        .map(str::to_owned)
        .ok_or(UploadError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::extract::{Path as UrlPath, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{post, put};
    use futures_util::StreamExt;
    use std::sync::{Arc, Mutex};
    use stream_common::key::Secret;
    use tokio::net::TcpListener;

    /// A part as storage got it: number, content-length header, transfer-encoding header, body.
    type Received = (u32, Option<String>, Option<String>, Vec<u8>);

    #[derive(Default)]
    struct Storage {
        received: Vec<Received>,
        /// Part numbers that answer 503 once.
        flaky: Vec<u32>,
        /// Part numbers whose answer has no ETag.
        no_etag: Vec<u32>,
    }

    type Shared = Arc<Mutex<Storage>>;

    async fn put_part_route(
        State(storage): State<Shared>,
        UrlPath(n): UrlPath<u32>,
        headers: HeaderMap,
        body: Body,
    ) -> Response {
        let mut bytes = Vec::new();
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk.unwrap());
        }
        let mut s = storage.lock().unwrap();
        if let Some(i) = s.flaky.iter().position(|f| *f == n) {
            s.flaky.remove(i);
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let header = |name: &str| headers.get(name).map(|v| v.to_str().unwrap().to_owned());
        s.received.push((
            n,
            header("content-length"),
            header("transfer-encoding"),
            bytes,
        ));
        if s.no_etag.contains(&n) {
            return StatusCode::OK.into_response();
        }
        ([("etag", format!("\"etag-{n}\""))], StatusCode::OK).into_response()
    }

    /// The site's uploads route, answering `part_bytes` and one URL per part at the fake storage.
    async fn fake_site(
        storage_addr: std::net::SocketAddr,
        part_bytes: i64,
        parts: u32,
    ) -> ApiClient {
        let app = Router::new().route(
            "/api/v4/lms/stream_worker/render/{id}/uploads",
            post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
                assert_eq!(body["worker_id"], "w1");
                let parts: Vec<_> = (1..=parts)
                    .map(|n| serde_json::json!({"number": n, "url": format!("http://{storage_addr}/part/{n}?sig=SIGSECRET{n}")}))
                    .collect();
                axum::Json(serde_json::json!({"part_bytes": part_bytes, "parts": parts}))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        ApiClient::new(
            &format!("http://{addr}/api/v4"),
            Secret::new("worker-token"),
            Duration::from_secs(5),
        )
        .unwrap()
    }

    async fn fake_storage(storage: Shared) -> std::net::SocketAddr {
        let app = Router::new()
            .route("/part/{n}", put(put_part_route))
            .with_state(storage);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    fn limits() -> UploadLimits {
        UploadLimits {
            attempts: 3,
            retry_delay: Duration::from_millis(10),
            part_timeout: Duration::from_secs(5),
        }
    }

    fn file_of(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("final.mp4");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn the_file_goes_in_the_plans_parts_with_their_exact_sizes() {
        let storage = Shared::default();
        let addr = fake_storage(storage.clone()).await;
        let api = fake_site(addr, 4, 3).await;
        let (_dir, path) = file_of(b"0123456789");
        let (size, parts) = upload(&api, &http(), "w1", "r1", &path, limits())
            .await
            .unwrap();
        assert_eq!(size, 10);
        assert_eq!(
            parts,
            vec![
                UploadedPart {
                    number: 1,
                    etag: "\"etag-1\"".into()
                },
                UploadedPart {
                    number: 2,
                    etag: "\"etag-2\"".into()
                },
                UploadedPart {
                    number: 3,
                    etag: "\"etag-3\"".into()
                },
            ]
        );
        let received = &storage.lock().unwrap().received;
        let bodies: Vec<_> = received.iter().map(|r| r.3.clone()).collect();
        assert_eq!(
            bodies,
            vec![b"0123".to_vec(), b"4567".to_vec(), b"89".to_vec()]
        );
        for (_, length, encoding, body) in received {
            assert_eq!(length.as_deref(), Some(body.len().to_string().as_str()));
            assert!(
                encoding.is_none(),
                "a presigned PUT carries its length, not chunks"
            );
        }
    }

    #[tokio::test]
    async fn a_part_that_fails_once_is_tried_again_and_a_part_that_keeps_failing_ends_it() {
        let storage = Shared::default();
        storage.lock().unwrap().flaky = vec![2];
        let addr = fake_storage(storage.clone()).await;
        let api = fake_site(addr, 4, 3).await;
        let (_dir, path) = file_of(b"0123456789");
        let (_, parts) = upload(&api, &http(), "w1", "r1", &path, limits())
            .await
            .unwrap();
        assert_eq!(parts.len(), 3);

        let storage = Shared::default();
        storage.lock().unwrap().flaky = vec![2, 2, 2];
        let addr = fake_storage(storage.clone()).await;
        let api = fake_site(addr, 4, 3).await;
        assert_eq!(
            upload(&api, &http(), "w1", "r1", &path, limits()).await,
            Err(UploadError::Storage)
        );
        assert_eq!(
            storage.lock().unwrap().received.len(),
            1,
            "later parts are not sent"
        );
    }

    #[tokio::test]
    async fn an_answer_without_an_etag_is_a_failure() {
        let storage = Shared::default();
        storage.lock().unwrap().no_etag = vec![1];
        let addr = fake_storage(storage).await;
        let api = fake_site(addr, 20, 1).await;
        let (_dir, path) = file_of(b"0123456789");
        assert_eq!(
            upload(&api, &http(), "w1", "r1", &path, limits()).await,
            Err(UploadError::Storage)
        );
    }

    #[tokio::test]
    async fn a_plan_that_does_not_cover_the_file_is_refused_before_anything_is_sent() {
        let storage = Shared::default();
        let addr = fake_storage(storage.clone()).await;
        let (_dir, path) = file_of(b"0123456789");
        for (part_bytes, parts) in [(4, 2), (4, 4), (0, 1), (-4, 3)] {
            let api = fake_site(addr, part_bytes, parts).await;
            assert_eq!(
                upload(&api, &http(), "w1", "r1", &path, limits()).await,
                Err(UploadError::BadPlan),
                "{part_bytes} {parts}"
            );
        }
        assert!(storage.lock().unwrap().received.is_empty());
    }

    #[test]
    fn the_cut_numbers_parts_from_one_in_order() {
        let part = |number| UploadPart {
            number,
            url: Secret::new("https://s.example/p"),
        };
        assert_eq!(
            cut(10, 4, &[part(1), part(2), part(3)]),
            Some(vec![(1, 0, 4), (2, 4, 4), (3, 8, 2)])
        );
        assert_eq!(
            cut(8, 4, &[part(1), part(2)]),
            Some(vec![(1, 0, 4), (2, 4, 4)])
        );
        assert_eq!(cut(10, 4, &[part(1), part(3), part(2)]), None);
        assert_eq!(cut(10, 4, &[part(0), part(1), part(2)]), None);
    }

    #[tokio::test]
    async fn a_site_that_will_not_plan_ends_the_upload() {
        let app = Router::new().route(
            "/api/v4/lms/stream_worker/render/{id}/uploads",
            post(|| async { StatusCode::CONFLICT }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let api = ApiClient::new(
            &format!("http://{addr}/api/v4"),
            Secret::new("t"),
            Duration::from_secs(5),
        )
        .unwrap();
        let (_dir, path) = file_of(b"x");
        assert_eq!(
            upload(&api, &http(), "w1", "r1", &path, limits()).await,
            Err(UploadError::Plan)
        );
        assert_eq!(
            upload(
                &api,
                &http(),
                "w1",
                "r1",
                Path::new("/nonexistent/file"),
                limits()
            )
            .await,
            Err(UploadError::File)
        );
    }
}
