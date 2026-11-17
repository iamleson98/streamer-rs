//! Why a stream failed. Inside the runner a failure is one of five causes; to the site it is one of
//! the codes its `finish` route accepts. Tool output (ffmpeg's messages) is read here to tell the
//! causes apart and is never sent anywhere.

use std::fmt;

/// A cause of failure, named by what the runner could tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The platform refused the key, or the key could not be used at all.
    AuthRejected,
    /// The platform's ingest could not be reached or the connection kept dropping.
    IngestUnreachable,
    /// The rendered file is not one the runner may stream (wrong codecs, size or length).
    InputError,
    /// The rendered file is gone or its address no longer works.
    FileMissing,
    /// Anything else that ended the stream.
    StreamFailed,
}

impl Failure {
    /// The name used in the runner's own logs.
    pub fn name(self) -> &'static str {
        match self {
            Self::AuthRejected => "auth_rejected",
            Self::IngestUnreachable => "ingest_unreachable",
            Self::InputError => "input_error",
            Self::FileMissing => "file_missing",
            Self::StreamFailed => "stream_failed",
        }
    }

    /// The `failure_code` the site's `finish` route accepts for this cause. A file that is not fit to
    /// stream is the render's fault, so it is reported as `render_failed`.
    pub fn wire_code(self) -> &'static str {
        match self {
            Self::AuthRejected => "key_rejected",
            Self::IngestUnreachable => "connection_lost",
            Self::InputError => "render_failed",
            Self::FileMissing => "file_missing",
            Self::StreamFailed => "stream_failed",
        }
    }

    /// Whether starting again can help: a refused key, a bad or missing file stay so.
    pub fn is_resumable(self) -> bool {
        matches!(self, Self::IngestUnreachable | Self::StreamFailed)
    }

    /// Reads ffmpeg's or ffprobe's error lines. Only words are looked for; the lines are not kept.
    pub fn classify<S: AsRef<str>>(lines: &[S]) -> Self {
        let text = lines
            .iter()
            .map(|l| l.as_ref().to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("\n");
        let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
        // A broken file is checked first: "moov atom not found" holds the words of a missing one.
        if has(&[
            "invalid data found",
            "moov atom not found",
            "could not find codec",
            "no such stream",
            "matches no streams",
        ]) {
            Self::InputError
        // Status numbers are looked for with their words: ffmpeg's pointers (`0x55d4040a`) hold digits too.
        } else if has(&[
            "http error 404",
            "http error 403",
            "http error 410",
            "returned 404",
            "returned 403",
            "returned 410",
            "not found",
            "no such file",
            "forbidden",
        ]) {
            Self::FileMissing
        } else if has(&[
            "server error",
            "badname",
            "unauthorized",
            "not authorized",
            "authentication",
            "publish.failed",
        ]) {
            Self::AuthRejected
        } else if has(&[
            "connection refused",
            "connection reset",
            "connection timed out",
            "timed out",
            "name or service not known",
            "failed to resolve",
            "network is unreachable",
            "cannot open connection",
            "handshake failed",
            "broken pipe",
            "tls",
            "i/o error",
            "input/output error",
        ]) {
            Self::IngestUnreachable
        } else {
            Self::StreamFailed
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_codes_are_the_ones_the_site_accepts() {
        let accepted = [
            "recording_unreadable",
            "render_failed",
            "file_missing",
            "key_rejected",
            "connection_lost",
            "stream_failed",
        ];
        for f in [
            Failure::AuthRejected,
            Failure::IngestUnreachable,
            Failure::InputError,
            Failure::FileMissing,
            Failure::StreamFailed,
        ] {
            assert!(accepted.contains(&f.wire_code()), "{f}");
        }
        assert_eq!(Failure::AuthRejected.wire_code(), "key_rejected");
        assert_eq!(Failure::IngestUnreachable.wire_code(), "connection_lost");
    }

    #[test]
    fn only_transient_failures_resume() {
        assert!(Failure::IngestUnreachable.is_resumable());
        assert!(Failure::StreamFailed.is_resumable());
        assert!(!Failure::AuthRejected.is_resumable());
        assert!(!Failure::InputError.is_resumable());
        assert!(!Failure::FileMissing.is_resumable());
    }

    #[test]
    fn error_lines_map_to_causes() {
        let cases: [(&[&str], Failure); 9] = [
            (
                &["[rtmp @ 0x1] Server error: Stream key is invalid"],
                Failure::AuthRejected,
            ),
            (
                &["[rtmp @ 0x1] Unable to publish: NetStream.Publish.BadName"],
                Failure::AuthRejected,
            ),
            (
                &["[tcp @ 0x1] Connection to tcp://a:443 failed: Connection refused"],
                Failure::IngestUnreachable,
            ),
            (
                &["[tls @ 0x1] Error in the pull function."],
                Failure::IngestUnreachable,
            ),
            (
                &[
                    "Error writing trailer: Broken pipe",
                    "Error closing file: Broken pipe",
                ],
                Failure::IngestUnreachable,
            ),
            (
                &["[https @ 0x1] HTTP error 404 Not Found"],
                Failure::FileMissing,
            ),
            (
                &["[https @ 0x1] Server returned 403 Forbidden (access denied)"],
                Failure::FileMissing,
            ),
            (
                &["Invalid data found when processing input"],
                Failure::InputError,
            ),
            (&["something nobody has seen"], Failure::StreamFailed),
        ];
        for (lines, want) in cases {
            assert_eq!(Failure::classify(lines), want, "{lines:?}");
        }
        assert_eq!(Failure::classify::<&str>(&[]), Failure::StreamFailed);
    }
}
