//! Which addresses the file to stream may come from. The site hands the runner a presigned address;
//! the runner opens it only when it is HTTPS on a host the operator named, so a wrong or hostile
//! answer cannot make it fetch from anywhere else.

use reqwest::Url;

/// The protocols ffmpeg and ffprobe may open for the file.
#[cfg(not(feature = "test-ingest"))]
pub const INPUT_PROTOCOLS: &str = "https,tls,tcp";
/// The protocols ffmpeg and ffprobe may open for the file (the test build adds plain `http`).
#[cfg(feature = "test-ingest")]
pub const INPUT_PROTOCOLS: &str = "https,tls,tcp,http";

/// Whether the scheme (and, for the test build's plain `http`, the host) may carry the file.
fn scheme_allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        #[cfg(feature = "test-ingest")]
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1")),
        _ => false,
    }
}

/// Whether `address` is an `https://` address without credentials whose host is one of `hosts`
/// (lowercase names; a port is allowed, the name decides). The `test-ingest` build also takes
/// `http://` for `localhost` and `127.0.0.1`.
pub fn is_allowed_media_url(address: &str, hosts: &[String]) -> bool {
    let Ok(url) = Url::parse(address) else {
        return false;
    };
    scheme_allowed(&url)
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .host_str()
            .is_some_and(|host| hosts.iter().any(|allowed| allowed == host))
}

/// Reads a comma-separated list of host names (what the operator sets): lowercase, no empty items.
/// `None` when there is no usable name or one is not a plain DNS name.
pub fn parse_hosts(list: &str) -> Option<Vec<String>> {
    let hosts: Vec<String> = list
        .split(',')
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    let plain = |h: &String| {
        h.len() <= 253
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
            && !h.starts_with(['.', '-'])
    };
    (!hosts.is_empty() && hosts.iter().all(plain)).then_some(hosts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> Vec<String> {
        parse_hosts("Store.example, other.example").unwrap()
    }

    #[test]
    fn only_https_on_a_named_host() {
        let ok = [
            "https://store.example/f.mp4?sig=1",
            "https://OTHER.example:8443/f.mp4",
        ];
        for url in ok {
            assert!(is_allowed_media_url(url, &hosts()), "{url}");
        }
        let refused = [
            "http://store.example/f.mp4",
            "https://evil.example/f.mp4",
            "https://store.example.evil.example/f.mp4",
            "https://evil.example/store.example",
            "https://store.example@evil.example/f.mp4",
            "https://user:pw@store.example/f.mp4",
            "https://store.example:pw@evil.example/",
            "file:///etc/passwd",
            "ftp://store.example/f.mp4",
            "https:///f.mp4",
            "not a url",
            "",
        ];
        for url in refused {
            assert!(!is_allowed_media_url(url, &hosts()), "{url}");
        }
        assert!(!is_allowed_media_url("https://store.example/", &[]));
    }

    /// Plain http is never taken in the default build, not even from this machine.
    #[cfg(not(feature = "test-ingest"))]
    #[test]
    fn the_default_build_refuses_http_even_on_this_machine() {
        let local = parse_hosts("localhost,127.0.0.1").unwrap();
        for url in ["http://localhost:9000/f.mp4", "http://127.0.0.1:9000/f.mp4"] {
            assert!(!is_allowed_media_url(url, &local), "{url}");
        }
        assert_eq!(INPUT_PROTOCOLS, "https,tls,tcp");
    }

    /// The test build takes http only from this machine, and the host must still be named.
    #[cfg(feature = "test-ingest")]
    #[test]
    fn the_test_build_takes_http_only_from_this_machine() {
        let local = parse_hosts("localhost,127.0.0.1,store.example").unwrap();
        for url in [
            "http://localhost:9000/f.mp4?sig=1",
            "http://127.0.0.1:9000/f.mp4",
        ] {
            assert!(is_allowed_media_url(url, &local), "{url}");
        }
        for url in [
            "http://store.example/f.mp4",
            "http://evil.example/f.mp4",
            "http://localhost.evil.example/f.mp4",
        ] {
            assert!(!is_allowed_media_url(url, &local), "{url}");
        }
        assert!(!is_allowed_media_url("http://localhost:9000/f.mp4", &[]));
    }

    #[test]
    fn the_host_list() {
        assert_eq!(
            parse_hosts(" a.example ,B.example,, "),
            Some(vec!["a.example".to_owned(), "b.example".to_owned()])
        );
        for bad in [
            "",
            " , ",
            "a b",
            "a.example/path",
            "https://a.example",
            "-a.example",
            "a_b.example",
        ] {
            assert_eq!(parse_hosts(bad), None, "{bad:?}");
        }
    }
}
