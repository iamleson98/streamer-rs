//! Where a platform's streams go. The platform is an enum the site sends; the address is a constant
//! here (YouTube, Facebook) or checked against the platform's own hosts (TikTok, which shows a server
//! address with every live session), so no address ever comes from the API and none can send a key
//! to a host the platform does not own.

use crate::key::{KeyError, Secret, StreamKey};

/// A platform a lesson can be streamed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    YouTube,
    Facebook,
    TikTok,
}

/// The hosts TikTok's ingest servers are on: a host is accepted when it is one of these or a
/// subdomain of one.
pub const TIKTOK_HOSTS: [&str; 4] = [
    "tiktok.com",
    "tiktokcdn.com",
    "tiktokcdn-us.com",
    "tiktokv.com",
];

/// The longest TikTok key (the part after the server address).
const TIKTOK_KEY_MAX_LEN: usize = 100;
const TIKTOK_KEY_MIN_LEN: usize = 8;

/// What a secret opens into: the address ffmpeg sends to and the key in it (the part the process's
/// output is cleaned of, with the whole address).
#[derive(Debug, Clone)]
pub struct Ingest {
    pub output_url: Secret,
    pub key: Secret,
}

impl Platform {
    /// Reads the API's platform name; `None` for one this build does not know.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "YOUTUBE" => Some(Self::YouTube),
            "FACEBOOK" => Some(Self::Facebook),
            "TIKTOK" => Some(Self::TikTok),
            _ => None,
        }
    }

    /// The ingest address without the stream key, for a platform that has one address; `None` for
    /// TikTok, whose address comes with the secret.
    pub fn ingest_base(self) -> Option<&'static str> {
        match self {
            Self::YouTube => Some("rtmps://a.rtmps.youtube.com/live2"),
            // Facebook takes RTMPS only, on port 443.
            Self::Facebook => Some("rtmps://live-api-s.facebook.com:443/rtmp"),
            Self::TikTok => None,
        }
    }

    /// Turns what the browser sealed into the address to send to. For YouTube and Facebook the
    /// secret is the stream key; for TikTok it is the push address, server and key.
    pub fn ingest(self, secret: &Secret) -> Result<Ingest, KeyError> {
        match self.ingest_base() {
            Some(base) => {
                let key = StreamKey::new(secret.expose().to_owned())?;
                Ok(Ingest {
                    output_url: Secret::new(format!("{base}/{}", key.expose())),
                    key: Secret::new(key.expose()),
                })
            }
            None => tiktok_ingest(secret.expose()),
        }
    }
}

/// `rtmp(s)://host[:port][/segment...]/key`, on a TikTok host, the key being the last segment.
fn tiktok_ingest(address: &str) -> Result<Ingest, KeyError> {
    let rest = address
        .strip_prefix("rtmps://")
        .or_else(|| address.strip_prefix("rtmp://"))
        .ok_or(KeyError::Invalid)?;
    let (server, key) = rest.rsplit_once('/').ok_or(KeyError::Invalid)?;
    let (authority, path) = match server.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (server, None),
    };
    if !tiktok_authority_ok(authority)
        || !path.is_none_or(|p| p.split('/').all(tiktok_segment_ok))
        || !tiktok_key_ok(key)
    {
        return Err(KeyError::Invalid);
    }
    Ok(Ingest {
        output_url: Secret::new(address),
        key: Secret::new(key),
    })
}

/// A lower-case host on one of TikTok's domains, and a port when there is one.
fn tiktok_authority_ok(authority: &str) -> bool {
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let port_ok = port.is_none_or(|p| {
        !p.is_empty()
            && p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
    });
    let labels_ok = host.split('.').count() >= 2
        && host.split('.').all(|l| {
            !l.is_empty()
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
    port_ok
        && labels_ok
        && TIKTOK_HOSTS
            .iter()
            .any(|d| host == *d || host.strip_suffix(d).is_some_and(|h| h.ends_with('.')))
}

/// A path segment of the server address.
fn tiktok_segment_ok(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
}

/// TikTok's key may carry a query-like tail (`?expire=...&sign=...`), so `?`, `&`, `=` and `:` are
/// allowed here; nothing that could change the host or end the address is (`/`, `@`, `#`, `\`,
/// quotes, whitespace).
fn tiktok_key_ok(key: &str) -> bool {
    (TIKTOK_KEY_MIN_LEN..=TIKTOK_KEY_MAX_LEN).contains(&key.len())
        && key.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'.' | b'_' | b'~' | b':' | b'?' | b'&' | b'=' | b'%' | b'+' | b',' | b'-'
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(platform: Platform, secret: &str) -> Result<String, KeyError> {
        platform
            .ingest(&Secret::new(secret))
            .map(|i| i.output_url.expose().to_owned())
    }

    #[test]
    fn the_platform_names() {
        assert_eq!(Platform::parse("YOUTUBE"), Some(Platform::YouTube));
        assert_eq!(Platform::parse("FACEBOOK"), Some(Platform::Facebook));
        assert_eq!(Platform::parse("TIKTOK"), Some(Platform::TikTok));
        for unknown in [
            "",
            "youtube",
            "Facebook",
            "TWITCH",
            "rtmp://evil.example/live",
        ] {
            assert_eq!(Platform::parse(unknown), None, "{unknown:?}");
        }
    }

    #[test]
    fn youtube_and_facebook_send_the_key_to_their_constant_address() {
        assert_eq!(
            out(Platform::YouTube, "abcd-efgh").unwrap(),
            "rtmps://a.rtmps.youtube.com/live2/abcd-efgh"
        );
        assert_eq!(
            out(Platform::Facebook, "FB-123-0-AbCd").unwrap(),
            "rtmps://live-api-s.facebook.com:443/rtmp/FB-123-0-AbCd"
        );
        let ingest = Platform::Facebook
            .ingest(&Secret::new("FB-123-0-AbCd"))
            .unwrap();
        assert_eq!(ingest.key.expose(), "FB-123-0-AbCd");
    }

    #[test]
    fn a_key_of_youtube_or_facebook_cannot_change_the_address() {
        for platform in [Platform::YouTube, Platform::Facebook] {
            for bad in [
                "a/b",
                "a@b",
                "a:b",
                "a?b",
                "a#b",
                "a%2fb",
                "a b",
                "a\\b",
                "",
                "rtmp://evil.example/x",
            ] {
                assert_eq!(
                    out(platform, bad),
                    Err(KeyError::Invalid),
                    "{platform:?} {bad:?}"
                );
            }
        }
    }

    #[test]
    fn tiktok_sends_to_its_own_server_address() {
        for good in [
            "rtmp://push-rtmp-f5-sg01.tiktokcdn.com/game/stream-1234abcd",
            "rtmps://push.tiktok.com:443/live/abcdefgh",
            "rtmp://push.tiktokcdn-us.com:1935/a/b/abcdefgh",
            "rtmp://tiktok.com/live/abcdefgh",
            "rtmp://push.tiktokcdn.com/abcdefgh",
            "rtmp://push.tiktokcdn.com/game/stream-12345678?expire=170&sign=ab%2B-_.",
        ] {
            assert_eq!(out(Platform::TikTok, good).as_deref(), Ok(good), "{good}");
        }
        let ingest = Platform::TikTok
            .ingest(&Secret::new(
                "rtmp://push.tiktokcdn.com/game/stream-1234abcd",
            ))
            .unwrap();
        assert_eq!(ingest.key.expose(), "stream-1234abcd");
    }

    #[test]
    fn tiktok_never_sends_a_key_to_a_host_that_is_not_its_own() {
        for bad in [
            "rtmp://evil.example/game/abcdefgh",
            "rtmp://tiktokcdn.com.evil.example/game/abcdefgh",
            "rtmp://eviltiktok.com/game/abcdefgh",
            "rtmp://nottiktokcdn.com/game/abcdefgh",
            "rtmp://user@push.tiktokcdn.com/game/abcdefgh",
            "rtmp://push.tiktokcdn.com@evil.example/game/abcdefgh",
            "rtmp://push.tiktokcdn.com:80@evil.example/game/abcdefgh",
            "rtmp://PUSH.tiktokcdn.com/game/abcdefgh",
            "rtmp://push.tiktokcdn.com:0/game/abcdefgh",
            "rtmp://push.tiktokcdn.com:65536/game/abcdefgh",
            "rtmp://push.tiktokcdn.com:/game/abcdefgh",
            "rtmp://push.tiktokcdn.com\\.evil.example/game/abcdefgh",
            "rtmp://push.tiktokcdn.com#.evil.example/game/abcdefgh",
            "rtmp://push.tiktokcdn.com?x=1/game/abcdefgh",
            "http://push.tiktokcdn.com/game/abcdefgh",
            "https://push.tiktokcdn.com/game/abcdefgh",
            "tcp://push.tiktokcdn.com/game/abcdefgh",
            "push.tiktokcdn.com/game/abcdefgh",
            "rtmp://push.tiktokcdn.com",
            "rtmp://push.tiktokcdn.com/",
            "rtmp://push.tiktokcdn.com//abcdefgh",
            "rtmp://push.tiktokcdn.com/../abcdefgh",
            "rtmp://push.tiktokcdn.com/game/",
            "rtmp://push.tiktokcdn.com/game/short",
            "rtmp://push.tiktokcdn.com/game/abc def gh",
            "rtmp://localhost/game/abcdefgh",
            "rtmp://127.0.0.1/game/abcdefgh",
            "rtmp://com/game/abcdefgh",
            "",
        ] {
            assert_eq!(
                out(Platform::TikTok, bad),
                Err(KeyError::Invalid),
                "{bad:?}"
            );
        }
        let long = format!(
            "rtmp://push.tiktokcdn.com/game/{}",
            "k".repeat(TIKTOK_KEY_MAX_LEN + 1)
        );
        assert_eq!(out(Platform::TikTok, &long), Err(KeyError::Invalid));
    }
}
