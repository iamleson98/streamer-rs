//! What the compositor needs to know of a downloaded video file: when its first picture is, on the
//! clock ffmpeg uses (the file's start shifted to zero).

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
#[error("cannot probe a file: {0}")]
pub struct ProbeError(pub &'static str);

#[derive(Deserialize)]
struct Packet {
    pts_time: Option<String>,
}

#[derive(Deserialize)]
struct Format {
    start_time: Option<String>,
}

#[derive(Deserialize)]
struct Output {
    #[serde(default)]
    packets: Vec<Packet>,
    format: Option<Format>,
}

/// When the first picture of the first video stream of `file` is, in seconds from the start of the
/// file as ffmpeg's command line sees it.
pub async fn first_picture(
    ffprobe: &Path,
    file: &Path,
    timeout: Duration,
) -> Result<f64, ProbeError> {
    let run = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-read_intervals",
            "%+#1",
            "-show_entries",
            "format=start_time:packet=pts_time",
            "-of",
            "json",
        ])
        .arg(file)
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| ProbeError("timed out"))?
        .map_err(|_| ProbeError("cannot run"))?;
    if !output.status.success() {
        return Err(ProbeError("refused"));
    }
    parse(&output.stdout)
}

fn parse(json: &[u8]) -> Result<f64, ProbeError> {
    let out: Output = serde_json::from_slice(json).map_err(|_| ProbeError("unreadable output"))?;
    let seconds = |s: &Option<String>| s.as_deref().and_then(|v| v.parse::<f64>().ok());
    let first = out
        .packets
        .first()
        .and_then(|p| seconds(&p.pts_time))
        .ok_or(ProbeError("no picture"))?;
    let start = out
        .format
        .and_then(|f| seconds(&f.start_time))
        .unwrap_or(0.0);
    Ok((first - start).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_picture_is_measured_from_the_files_start() {
        let json = br#"{"packets":[{"pts_time":"0.230000"}],"format":{"start_time":"0.030000"}}"#;
        assert!((parse(json).unwrap() - 0.2).abs() < 1e-9);
        assert_eq!(parse(br#"{"packets":[{"pts_time":"0.1"}]}"#).unwrap(), 0.1);
        assert!(parse(br#"{"packets":[],"format":{}}"#).is_err());
        assert!(parse(b"nope").is_err());
    }
}
