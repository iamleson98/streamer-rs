//! Where a platform's streams go. The platform is an enum the site sends; the address is a constant
//! here, so no address ever comes from a browser or from the API.

use crate::key::{Secret, StreamKey};

/// A platform a lesson can be streamed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    YouTube,
}

impl Platform {
    /// Reads the API's platform name; `None` for one this build does not know.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "YOUTUBE" => Some(Self::YouTube),
            _ => None,
        }
    }

    /// The ingest address without the stream key.
    pub fn ingest_base(self) -> &'static str {
        match self {
            Self::YouTube => "rtmps://a.rtmps.youtube.com/live2",
        }
    }

    /// The full output address: the ingest base and the key.
    pub fn output_url(self, key: &StreamKey) -> Secret {
        Secret::new(format!("{}/{}", self.ingest_base(), key.expose()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_destination_map() {
        assert_eq!(Platform::parse("YOUTUBE"), Some(Platform::YouTube));
        for unknown in ["", "youtube", "FACEBOOK", "rtmp://evil.example/live"] {
            assert_eq!(Platform::parse(unknown), None, "{unknown:?}");
        }
        assert_eq!(
            Platform::YouTube.ingest_base(),
            "rtmps://a.rtmps.youtube.com/live2"
        );
    }

    #[test]
    fn the_output_url_is_the_constant_and_the_key() {
        let key = StreamKey::new("abcd-efgh".into()).unwrap();
        assert_eq!(
            Platform::YouTube.output_url(&key).expose(),
            "rtmps://a.rtmps.youtube.com/live2/abcd-efgh"
        );
    }
}
