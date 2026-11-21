//! Reads one recording for the native engine: the same reads, addresses and redirect rules as the
//! render page's proxy (`recording.rs`), with the newest render token the heartbeat has brought, and
//! every media file downloaded into the render's directory, so ffmpeg only ever gets a local path
//! and the token never appears on a command line or in a log.
//!
//! The errors carry a fixed reason and never the address: a `reqwest` error would print the
//! presigned storage address, signature included.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::{Method, StatusCode};
use reqwest::Url;
use stream_common::key::Secret;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::watch;

use crate::recording::{Fetch, MAX_MANIFEST_BYTES, Scope, deck_page_address, fetch, read_capped};
use stream_common::api::is_site_id;

/// The most a single media file may be (the manifest's size is checked against it too).
pub const MAX_FILE_BYTES: u64 = 1 << 30;
/// The most media files a manifest may list.
pub const MAX_FILES: usize = 20_000;
/// The most a gzipped board log may be.
pub const MAX_BOARD_BYTES: usize = 64 << 20;
/// The most a deck page picture may be.
pub const MAX_PICTURE_BYTES: usize = 20 << 20;

/// Why a read failed: always the recording being unreadable, with a reason that names no address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("recording unreadable: {0}")]
pub struct ReadError(pub &'static str);

/// Reads of one recording.
#[derive(Debug)]
pub struct RecordingReader {
    http: reqwest::Client,
    scope: Scope,
    first: Secret,
    tokens: watch::Receiver<Option<Secret>>,
    attempts: usize,
    retry_delay: Duration,
    facts: Mutex<Facts>,
}

#[derive(Debug, Default)]
struct Facts {
    board: Option<Url>,
    class_id: Option<String>,
}

impl RecordingReader {
    pub fn new(
        upstream: &str,
        recording_id: &str,
        first: Secret,
        tokens: watch::Receiver<Option<Secret>>,
        attempts: usize,
        retry_delay: Duration,
    ) -> Result<Self, ReadError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| ReadError("cannot build the client"))?;
        Ok(Self {
            http,
            scope: Scope::new(upstream, recording_id),
            first,
            tokens,
            attempts: attempts.max(1),
            retry_delay,
            facts: Mutex::default(),
        })
    }

    fn facts(&self) -> std::sync::MutexGuard<'_, Facts> {
        self.facts.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The newest token: the heartbeat's latest, else the claim's.
    fn token(&self) -> String {
        self.tokens
            .borrow()
            .as_ref()
            .unwrap_or(&self.first)
            .expose()
            .to_owned()
    }

    fn headers(token: &str, range: Option<&str>) -> Result<HeaderMap, ReadError> {
        let mut headers = HeaderMap::new();
        let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ReadError("the token is not a header"))?;
        bearer.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, bearer);
        if let Some(range) = range {
            headers.insert(
                header::RANGE,
                HeaderValue::from_str(range).map_err(|_| ReadError("bad range"))?,
            );
        }
        Ok(headers)
    }

    /// One answer for `url`, retried on a cut connection, a 5xx or (once, and only when the token
    /// has changed since) a 401 or 403. The answer is a success (200 or 206) or an error.
    async fn get(
        &self,
        url: &str,
        how: Fetch,
        range: Option<&str>,
    ) -> Result<reqwest::Response, ReadError> {
        let mut last_token = String::new();
        let mut denied = false;
        for attempt in 0..self.attempts {
            if attempt > 0 {
                tokio::time::sleep(self.retry_delay).await;
            }
            let token = self.token();
            let headers = Self::headers(&token, range)?;
            let Some(answer) = fetch(&self.http, &Method::GET, url, &headers, how).await else {
                continue;
            };
            let status = answer.status();
            if status == StatusCode::OK || status == StatusCode::PARTIAL_CONTENT {
                return Ok(answer);
            }
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                if denied && token == last_token {
                    return Err(ReadError("the site refused the token"));
                }
                denied = true;
                last_token = token;
                continue;
            }
            if status.is_server_error() {
                continue;
            }
            return Err(ReadError("the site refused a read"));
        }
        Err(ReadError("the site could not be reached"))
    }

    /// The manifest as the site sent it. What it says of the class and the board is kept for the
    /// reads that follow.
    pub async fn manifest(&self) -> Result<serde_json::Value, ReadError> {
        let answer = self
            .get(&self.scope.manifest_url(), Fetch::Site, None)
            .await?;
        let body = read_capped(answer, MAX_MANIFEST_BYTES)
            .await
            .ok_or(ReadError("the manifest is unreadable or too large"))?;
        let manifest: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| ReadError("the manifest is not JSON"))?;
        let object = manifest
            .as_object()
            .ok_or(ReadError("the manifest is not an object"))?;
        if object
            .get("files")
            .and_then(|f| f.as_array())
            .is_some_and(|f| f.len() > MAX_FILES)
        {
            return Err(ReadError("the manifest lists too many files"));
        }
        let mut facts = self.facts();
        facts.class_id = object
            .get("class_id")
            .and_then(|v| v.as_str())
            .filter(|id| is_site_id(id))
            .map(str::to_owned);
        facts.board = object
            .get("board_url")
            .and_then(|a| crate::recording::storage_address(&a.clone(), &self.scope.upstream));
        Ok(manifest)
    }

    /// The gzipped board log: from the storage address the manifest gave, else as the recording's
    /// file `file_index`.
    pub async fn board_log(&self, file_index: Option<u32>) -> Result<Vec<u8>, ReadError> {
        let board = self.facts().board.clone();
        let answer = match (board, file_index) {
            (Some(address), _) => self.get(address.as_str(), Fetch::Storage, None).await?,
            (None, Some(index)) => {
                self.get(&self.scope.file_url(index), Fetch::Site, None)
                    .await?
            }
            (None, None) => return Err(ReadError("the recording has no board log")),
        };
        read_capped(answer, MAX_BOARD_BYTES)
            .await
            .ok_or(ReadError("the board log is unreadable or too large"))
    }

    /// A deck page picture of the class the manifest named.
    pub async fn deck_page(&self, deck: &str, n: u32) -> Result<Vec<u8>, ReadError> {
        let class = self
            .facts()
            .class_id
            .clone()
            .ok_or(ReadError("no class to read a deck page of"))?;
        let url = deck_page_address(&self.scope.upstream, &class, deck, &n.to_string())
            .ok_or(ReadError("not a deck page"))?;
        let answer = self.get(&url, Fetch::Site, None).await?;
        read_capped(answer, MAX_PICTURE_BYTES)
            .await
            .ok_or(ReadError("the picture is unreadable or too large"))
    }

    /// Downloads file `index`, which the manifest says is `size` bytes, to `dest`, resuming after a
    /// cut and refusing more or fewer bytes than the manifest said. The file is written under a
    /// temporary name (mode 0600) and renamed when it is whole.
    pub async fn download_file(&self, index: u32, size: u64, dest: &Path) -> Result<(), ReadError> {
        if size == 0 || size > MAX_FILE_BYTES {
            return Err(ReadError("a file's size is not one to read"));
        }
        let url = self.scope.file_url(index);
        let part = dest.with_extension("part");
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options
            .open(&part)
            .await
            .map_err(|_| ReadError("cannot create a file"))?;
        let mut have: u64 = 0;
        for attempt in 0..self.attempts {
            if attempt > 0 {
                tokio::time::sleep(self.retry_delay).await;
            }
            let range = (have > 0).then(|| format!("bytes={have}-"));
            let Ok(mut answer) = self.get(&url, Fetch::Site, range.as_deref()).await else {
                continue;
            };
            if answer.status() == StatusCode::PARTIAL_CONTENT {
                let starts = answer
                    .headers()
                    .get(header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("bytes "))
                    .and_then(|v| v.split(['-', '/']).next())
                    .and_then(|v| v.parse::<u64>().ok());
                if starts != Some(have) {
                    return Err(ReadError("a ranged read answered another range"));
                }
            } else if have > 0 {
                // The range was ignored: the whole file starts again.
                file.set_len(0)
                    .await
                    .map_err(|_| ReadError("cannot write a file"))?;
                file.rewind()
                    .await
                    .map_err(|_| ReadError("cannot write a file"))?;
                have = 0;
            }
            loop {
                match answer.chunk().await {
                    Ok(Some(chunk)) => {
                        have += chunk.len() as u64;
                        if have > size {
                            return Err(ReadError("a file is larger than its manifest says"));
                        }
                        file.write_all(&chunk)
                            .await
                            .map_err(|_| ReadError("cannot write a file"))?;
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            if have == size {
                file.flush()
                    .await
                    .map_err(|_| ReadError("cannot write a file"))?;
                drop(file);
                return tokio::fs::rename(&part, dest)
                    .await
                    .map_err(|_| ReadError("cannot write a file"));
            }
        }
        let _ = tokio::fs::remove_file(&part).await;
        Err(ReadError("a file could not be read whole"))
    }
}
