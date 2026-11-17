//! What ffprobe says about a rendered file, and the one question both workloads ask of it: is it
//! the kind of file the render makes (H.264 video, AAC audio, at most 1280x720, a real length)?

use serde::Deserialize;

const MAX_WIDTH: u32 = 1280;
const MAX_HEIGHT: u32 = 720;

/// ffprobe's `-print_format json -show_streams -show_format` output, the parts that are read.
#[derive(Debug, Deserialize)]
pub struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

impl ProbeOutput {
    /// `None` when `json` is not ffprobe's output.
    pub fn parse(json: &[u8]) -> Option<Self> {
        serde_json::from_slice(json).ok()
    }

    /// The file's length in seconds when its first video and first audio streams (the ones that are
    /// used) are H.264 and AAC and the picture is at most 1280x720; `None` otherwise, or when the
    /// length is missing, not a number or not positive.
    pub fn render_duration_secs(&self) -> Option<f64> {
        let first = |kind: &str| {
            self.streams
                .iter()
                .find(|s| s.codec_type.as_deref() == Some(kind))
        };
        let video = first("video")?;
        let audio = first("audio")?;
        if video.codec_name.as_deref() != Some("h264") || audio.codec_name.as_deref() != Some("aac")
        {
            return None;
        }
        match (video.width, video.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 && w <= MAX_WIDTH && h <= MAX_HEIGHT => {}
            _ => return None,
        }
        self.format
            .as_ref()?
            .duration
            .as_deref()?
            .parse()
            .ok()
            .filter(|d: &f64| d.is_finite() && *d > 0.0)
    }
}
