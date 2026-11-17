//! The gate before a stream: ffprobe reads the rendered file through its presigned address and the
//! runner streams it only when it is what the render makes (H.264 video, AAC audio, at most 1280x720,
//! not longer than the request said plus a few seconds).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use stream_common::codes::Failure;
use stream_common::probe::ProbeOutput as Parsed;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::ffmpeg;

/// How much longer than the request the file may be, in seconds.
const DURATION_SLACK_SECS: f64 = 5.0;
/// ffprobe's output is read up to this size; a real answer is a few kilobytes.
const MAX_OUTPUT_BYTES: u64 = 1 << 20;

#[derive(Debug)]
pub struct ProbeOutput(Parsed);

impl ProbeOutput {
    pub fn parse(json: &[u8]) -> Result<Self, Failure> {
        Parsed::parse(json).map(Self).ok_or(Failure::InputError)
    }

    /// Checks the first video and the first audio stream (the ones ffmpeg is told to map) and the
    /// length against the request's `duration_ms`.
    pub fn check(&self, duration_ms: i64) -> Result<(), Failure> {
        let seconds = self.0.render_duration_secs().ok_or(Failure::InputError)?;
        if seconds > duration_ms as f64 / 1000.0 + DURATION_SLACK_SECS {
            return Err(Failure::InputError);
        }
        Ok(())
    }
}

/// Runs ffprobe on the presigned address. A failure is classified from ffprobe's messages (a missing
/// file is told from an unreadable one); the messages themselves are dropped.
pub async fn probe(ffprobe: &Path, url: &str, timeout: Duration) -> Result<ProbeOutput, Failure> {
    let mut child = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            stream_common::media::INPUT_PROTOCOLS,
            "-rw_timeout",
            ffmpeg::IO_TIMEOUT_MICROS,
        ])
        .args(ffmpeg::INPUT_FORMAT)
        .args([
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            url,
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Failure::StreamFailed)?;
    let (mut stdout, mut stderr) = (
        child.stdout.take().ok_or(Failure::StreamFailed)?,
        child.stderr.take().ok_or(Failure::StreamFailed)?,
    );

    let run = async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (mut limited_out, mut limited_err) = (
            (&mut stdout).take(MAX_OUTPUT_BYTES),
            (&mut stderr).take(MAX_OUTPUT_BYTES),
        );
        let (_, _) = tokio::join!(
            limited_out.read_to_end(&mut out),
            limited_err.read_to_end(&mut err)
        );
        let status = child.wait().await;
        (status, out, err)
    };
    let Ok((status, out, err)) = tokio::time::timeout(timeout, run).await else {
        return Err(Failure::InputError);
    };
    match status {
        Ok(s) if s.success() => ProbeOutput::parse(&out),
        _ => {
            let lines: Vec<String> = String::from_utf8_lossy(&err)
                .lines()
                .map(str::to_owned)
                .collect();
            Err(match Failure::classify(&lines) {
                Failure::FileMissing => Failure::FileMissing,
                _ => Failure::InputError,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(video: &str, audio: &str, w: u32, h: u32, duration: &str) -> ProbeOutput {
        let json = format!(
            r#"{{"streams":[{{"codec_type":"video","codec_name":"{video}","width":{w},"height":{h}}},
                {{"codec_type":"audio","codec_name":"{audio}"}}],"format":{{"duration":"{duration}"}}}}"#
        );
        ProbeOutput::parse(json.as_bytes()).unwrap()
    }

    #[test]
    fn accepts_what_the_render_makes() {
        assert_eq!(
            out("h264", "aac", 1280, 720, "60.000").check(60_000),
            Ok(())
        );
        assert_eq!(
            out("h264", "aac", 1280, 720, "64.900").check(60_000),
            Ok(())
        );
        assert_eq!(out("h264", "aac", 640, 360, "10").check(60_000), Ok(()));
    }

    #[test]
    fn refuses_everything_else() {
        let bad = [
            out("hevc", "aac", 1280, 720, "60"),
            out("vp9", "aac", 1280, 720, "60"),
            out("h264", "opus", 1280, 720, "60"),
            out("h264", "aac", 1920, 1080, "60"),
            out("h264", "aac", 1281, 720, "60"),
            out("h264", "aac", 1280, 721, "60"),
            out("h264", "aac", 0, 0, "60"),
            out("h264", "aac", 1280, 720, "65.1"),
            out("h264", "aac", 1280, 720, "N/A"),
            out("h264", "aac", 1280, 720, "0"),
        ];
        for (i, o) in bad.iter().enumerate() {
            assert_eq!(o.check(60_000), Err(Failure::InputError), "case {i}");
        }
    }

    #[test]
    fn needs_a_video_and_an_audio_stream_and_a_length() {
        let no_audio = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1280,"height":720}],"format":{"duration":"5"}}"#;
        assert_eq!(
            ProbeOutput::parse(no_audio.as_bytes())
                .unwrap()
                .check(60_000),
            Err(Failure::InputError)
        );
        let no_format = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1,"height":1},{"codec_type":"audio","codec_name":"aac"}]}"#;
        assert_eq!(
            ProbeOutput::parse(no_format.as_bytes())
                .unwrap()
                .check(60_000),
            Err(Failure::InputError)
        );
        assert_eq!(
            ProbeOutput::parse(b"{}").unwrap().check(60_000),
            Err(Failure::InputError)
        );
        assert!(ProbeOutput::parse(b"not json").is_err());
    }
}
