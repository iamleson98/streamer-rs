//! After the engine is done: ffprobe checks the file is what the stream runner will accept, and its
//! length is the recording's.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use stream_common::probe::ProbeOutput;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// ffprobe's output is read up to this size; a real answer is a few kilobytes.
const MAX_PROBE_OUTPUT: u64 = 1 << 20;

/// Why the file is not fit to upload. No message carries ffprobe's text.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MediaError {
    #[error("ffprobe took too long")]
    Timeout,
    #[error("the finished file is not what the render makes")]
    NotRenderOutput,
}

/// The length in milliseconds of the finished file, after checking its codecs and size.
pub async fn probe(ffprobe: &Path, file: &Path, limit: Duration) -> Result<i64, MediaError> {
    let mut child = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(file)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            tracing::warn!(kind = ?error.kind(), "ffprobe did not start");
            MediaError::NotRenderOutput
        })?;
    let mut stdout = child.stdout.take().ok_or(MediaError::NotRenderOutput)?;
    let run = async {
        let mut out = Vec::new();
        let _ = (&mut stdout)
            .take(MAX_PROBE_OUTPUT)
            .read_to_end(&mut out)
            .await;
        (child.wait().await, out)
    };
    let Ok((status, out)) = tokio::time::timeout(limit, run).await else {
        return Err(MediaError::Timeout);
    };
    if !status.is_ok_and(|s| s.success()) {
        return Err(MediaError::NotRenderOutput);
    }
    let seconds = ProbeOutput::parse(&out)
        .and_then(|p| p.render_duration_secs())
        .ok_or(MediaError::NotRenderOutput)?;
    Ok((seconds * 1000.0).round() as i64)
}

/// The server accepts a rendered length within 5 seconds of the recording's, or 5 percent of it when
/// that is more (the same rule as the site's, so a file it would refuse is not uploaded).
pub fn length_within_tolerance(rendered_ms: i64, recording_ms: i64) -> bool {
    let tolerance = 5_000.max(recording_ms / 20);
    rendered_ms >= recording_ms - tolerance && rendered_ms <= recording_ms + tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const PROBE_OK: &str = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"duration":"12.3456"}}"#;
    const PROBE_VP9: &str = r#"{"streams":[{"codec_type":"video","codec_name":"vp9","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"duration":"12"}}"#;

    #[tokio::test]
    async fn the_probe_reads_the_length_and_refuses_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("final.mp4");
        let ok = script(dir.path(), "probe-ok", &format!("echo '{PROBE_OK}'"));
        assert_eq!(probe(&ok, &file, Duration::from_secs(5)).await, Ok(12_346));
        let vp9 = script(dir.path(), "probe-vp9", &format!("echo '{PROBE_VP9}'"));
        assert_eq!(
            probe(&vp9, &file, Duration::from_secs(5)).await,
            Err(MediaError::NotRenderOutput)
        );
        let bad = script(dir.path(), "probe-bad", "exit 1");
        assert_eq!(
            probe(&bad, &file, Duration::from_secs(5)).await,
            Err(MediaError::NotRenderOutput)
        );
        let junk = script(dir.path(), "probe-junk", "echo nonsense");
        assert_eq!(
            probe(&junk, &file, Duration::from_secs(5)).await,
            Err(MediaError::NotRenderOutput)
        );
    }

    #[test]
    fn the_tolerance_is_the_sites_rule() {
        // Up to 100 s: 5 s either way; beyond: 5 percent.
        assert!(length_within_tolerance(55_000, 60_000));
        assert!(length_within_tolerance(65_000, 60_000));
        assert!(!length_within_tolerance(54_999, 60_000));
        assert!(!length_within_tolerance(65_001, 60_000));
        assert!(length_within_tolerance(3_600_000 + 180_000, 3_600_000));
        assert!(!length_within_tolerance(3_600_000 + 180_001, 3_600_000));
    }
}
