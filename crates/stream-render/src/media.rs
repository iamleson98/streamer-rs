//! After the page is done: the raw fragmented MP4 becomes the file that is uploaded (video copied,
//! audio to AAC, the index at the front so a player can start at once), and ffprobe checks it is what
//! the stream runner will accept.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use stream_common::probe::ProbeOutput;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// ffprobe's output is read up to this size; a real answer is a few kilobytes.
const MAX_PROBE_OUTPUT: u64 = 1 << 20;
/// The lines of ffmpeg's stderr that are kept for the log.
const MAX_LOG_LINES: usize = 20;

/// The arguments of the remux, input and output last.
pub fn remux_args(raw: &Path, out: &Path) -> Vec<String> {
    let mut args = ["-nostdin", "-v", "error", "-y", "-i"]
        .map(String::from)
        .to_vec();
    args.push(raw.display().to_string());
    args.extend(
        [
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-movflags",
            "+faststart",
        ]
        .map(String::from),
    );
    args.push(out.display().to_string());
    args
}

/// Why the file could not be made or is not fit to upload. No message carries ffmpeg's text.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MediaError {
    #[error("ffmpeg could not remux the output")]
    Remux,
    #[error("ffmpeg took too long")]
    Timeout,
    #[error("the finished file is not what the render makes")]
    NotRenderOutput,
}

/// Runs ffmpeg. Its error lines (not its output) go to the log, redacted by the logging writer.
pub async fn remux(
    ffmpeg: &Path,
    raw: &Path,
    out: &Path,
    limit: Duration,
) -> Result<(), MediaError> {
    let mut child = Command::new(ffmpeg)
        .args(remux_args(raw, out))
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            tracing::warn!(kind = ?error.kind(), "ffmpeg did not start");
            MediaError::Remux
        })?;
    let stderr = child.stderr.take().ok_or(MediaError::Remux)?;
    let run = async {
        let mut lines = BufReader::new(stderr).lines();
        let mut kept = 0;
        while let Ok(Some(line)) = lines.next_line().await {
            if kept < MAX_LOG_LINES {
                tracing::warn!(line, "ffmpeg");
                kept += 1;
            }
        }
        child.wait().await
    };
    match tokio::time::timeout(limit, run).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(_) => Err(MediaError::Remux),
        Err(_) => Err(MediaError::Timeout),
    }
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

    #[test]
    fn the_remux_command_is_the_documented_one() {
        assert_eq!(
            remux_args(Path::new("/w/raw.mp4"), Path::new("/w/final.mp4")).join(" "),
            "-nostdin -v error -y -i /w/raw.mp4 -c:v copy -c:a aac -b:a 128k -movflags +faststart /w/final.mp4"
        );
    }

    #[tokio::test]
    async fn a_remux_that_succeeds_and_one_that_fails() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw.mp4");
        std::fs::write(&raw, "bytes").unwrap();
        let out = dir.path().join("final.mp4");
        // Arguments: -nostdin -v error -y -i RAW ... OUT, so RAW is the sixth.
        let copy = script(
            dir.path(),
            "copy",
            "for a; do last=$a; done; cp \"$6\" \"$last\"",
        );
        remux(&copy, &raw, &out, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"bytes");

        let fail = script(dir.path(), "fail", "echo 'Invalid data' >&2; exit 1");
        assert_eq!(
            remux(&fail, &raw, &out, Duration::from_secs(5)).await,
            Err(MediaError::Remux)
        );
        assert_eq!(
            remux(
                &dir.path().join("missing"),
                &raw,
                &out,
                Duration::from_secs(5)
            )
            .await,
            Err(MediaError::Remux)
        );
        let slow = script(dir.path(), "slow", "sleep 30");
        assert_eq!(
            remux(&slow, &raw, &out, Duration::from_millis(200)).await,
            Err(MediaError::Timeout)
        );
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
