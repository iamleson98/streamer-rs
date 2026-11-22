//! What every read of a recording through the site's API has in common, shared by the render page's
//! reverse proxy (`proxy.rs`) and the native engine's `RecordingReader` (`native/reader.rs`): the
//! addresses the worker may read, the redirect rules and the size caps. The render token goes on
//! the first hop to the site and nowhere else.

use reqwest::Method;
use reqwest::Url;
use reqwest::header::{self, HeaderMap, HeaderName};
use stream_common::api::is_site_id;

/// Redirects followed on the way to the bytes (the API answers one, to storage).
pub const MAX_HOPS: usize = 3;

/// The manifest is a small JSON document; a larger answer is not one.
pub const MAX_MANIFEST_BYTES: usize = 8 << 20;

/// The request headers passed on: the render token, the byte range and what the caller accepts.
const FORWARD_REQUEST: [HeaderName; 3] = [header::AUTHORIZATION, header::RANGE, header::ACCEPT];

/// How a fetch treats the token and redirects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    /// The site: the token goes on the first hop, redirects (to storage) are followed without it.
    Site,
    /// Storage, at one address: no token, and a redirect is an error.
    Storage,
}

/// Whether `text` is 1 to `max` ASCII digits.
pub fn digits(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_digit())
}

/// The whole body, or `None` when it is longer than `max` bytes or cannot be read.
pub async fn read_capped(mut upstream: reqwest::Response, max: usize) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = upstream.chunk().await.ok()? {
        if body.len() + chunk.len() > max {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

/// Fetches `first`. For [`Fetch::Site`] the first hop carries the token and a redirect (to a
/// presigned address) carries only the range; for [`Fetch::Storage`] there is no token and no
/// redirect. `http` must not follow redirects itself.
pub async fn fetch(
    http: &reqwest::Client,
    method: &Method,
    first: &str,
    incoming: &HeaderMap,
    how: Fetch,
) -> Option<reqwest::Response> {
    let mut url = Url::parse(first).ok()?;
    for hop in 0..=MAX_HOPS {
        let mut request = http.request(method.clone(), url.clone());
        for name in &FORWARD_REQUEST {
            if *name == header::AUTHORIZATION && (how == Fetch::Storage || hop > 0) {
                continue;
            }
            if let Some(value) = incoming.get(name) {
                request = request.header(name, value);
            }
        }
        let upstream = request.send().await.ok()?;
        if upstream.status().is_redirection() {
            if how == Fetch::Storage {
                return None;
            }
            let location = upstream.headers().get(header::LOCATION)?.to_str().ok()?;
            url = url.join(location).ok()?;
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            continue;
        }
        return Some(upstream);
    }
    None
}

/// The storage address the manifest gave for the board log: one that is not a plain http(s)
/// address of the API's scheme, or that carries credentials, is dropped, and the board is read as
/// the recording's own file instead.
pub fn storage_address(address: &serde_json::Value, upstream: &str) -> Option<Url> {
    let upstream_scheme = Url::parse(upstream).ok()?.scheme().to_owned();
    address
        .as_str()
        .and_then(|a| Url::parse(a).ok())
        .filter(|u| {
            u.scheme() == upstream_scheme
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
        })
}

/// `{class}/decks/{deck}/pages/{n}` of a class, as the site's address, or `None` when a part is
/// not what the site's ids and page numbers look like.
pub fn deck_page_address(upstream: &str, class: &str, deck: &str, n: &str) -> Option<String> {
    (is_site_id(class) && is_site_id(deck) && digits(n, 3))
        .then(|| format!("{upstream}/lms/classes/{class}/decks/{deck}/pages/{n}"))
}

/// The addresses of one recording's reads.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The API root, such as `https://host/api/v4`.
    pub upstream: String,
    pub recording_id: String,
}

impl Scope {
    pub fn new(upstream: &str, recording_id: &str) -> Self {
        Self {
            upstream: upstream.trim_end_matches('/').to_owned(),
            recording_id: recording_id.to_owned(),
        }
    }

    pub fn manifest_url(&self) -> String {
        format!(
            "{}/lms/recordings/{}/manifest",
            self.upstream, self.recording_id
        )
    }

    pub fn file_url(&self, index: u32) -> String {
        format!(
            "{}/lms/recordings/{}/files/{index}",
            self.upstream, self.recording_id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_one_to_max_ascii_digits() {
        assert!(digits("0", 6) && digits("123456", 6));
        for bad in ["", "1234567", "12a", "-1", "１２"] {
            assert!(!digits(bad, 6), "{bad:?}");
        }
    }

    #[test]
    fn a_storage_address_keeps_the_api_scheme_and_no_credentials() {
        let up = "https://site.example/api/v4";
        let ok = serde_json::json!("https://store.example/b?sig=1");
        assert!(storage_address(&ok, up).is_some());
        for bad in [
            serde_json::json!("http://store.example/b"),
            serde_json::json!("https://u:p@store.example/b"),
            serde_json::json!("file:///etc/passwd"),
            serde_json::json!(7),
        ] {
            assert!(storage_address(&bad, up).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_deck_page_needs_site_ids_and_a_short_page_number() {
        let (class, deck) = ("a".repeat(26), "b".repeat(26));
        let ok = deck_page_address("https://s/api/v4", &class, &deck, "12").unwrap();
        assert_eq!(
            ok,
            format!("https://s/api/v4/lms/classes/{class}/decks/{deck}/pages/12")
        );
        assert!(deck_page_address("u", &class, &deck, "1234").is_none());
        assert!(deck_page_address("u", "../x", &deck, "1").is_none());
        assert!(deck_page_address("u", &class, "short", "1").is_none());
    }

    #[test]
    fn a_scope_builds_the_recordings_addresses() {
        let scope = Scope::new("https://s/api/v4/", "rec1");
        assert_eq!(
            scope.manifest_url(),
            "https://s/api/v4/lms/recordings/rec1/manifest"
        );
        assert_eq!(
            scope.file_url(7),
            "https://s/api/v4/lms/recordings/rec1/files/7"
        );
    }
}
