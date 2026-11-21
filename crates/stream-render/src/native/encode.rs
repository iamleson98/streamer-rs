//! The encoder: raw BGRA frames on stdin, H.264 out. x264 veryfast, constant quality capped at
//! 2.5 Mbit/s, a keyframe exactly every 60 frames (the runner's 2-second rule) and no scene-cut
//! keyframes, Main profile, BT.709. The soundtrack is joined afterwards by [`mux`], which only
//! copies streams and puts the moov atom first.

use std::path::Path;
use std::process::Command;

use super::ffmpeg::ffmpeg;

/// What the encoder is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    /// The product's: x264 veryfast, constant quality capped at 2.5 Mbit/s.
    #[default]
    Production,
    /// A lossless copy of the composited frames (in 4:2:0), the reference `render-compare` measures
    /// both engines' pictures against. Never used for a render that is uploaded.
    Lossless,
}

/// The video-only encode: frames of `width` x `height` at `fps` on stdin, `out` an MP4.
pub fn encode_command(
    ffmpeg_bin: &Path,
    profile: Profile,
    (width, height): (u32, u32),
    fps: u32,
    keyframe_every: u32,
    out: &Path,
) -> Command {
    let mut c = ffmpeg(ffmpeg_bin);
    c.args(["-f", "rawvideo", "-pix_fmt", "bgra", "-s"])
        .arg(format!("{width}x{height}"))
        .arg("-framerate")
        .arg(fps.to_string())
        .args(["-i", "pipe:0", "-vf"])
        .arg("scale=out_color_matrix=bt709:out_range=tv,format=yuv420p");
    match profile {
        Profile::Production => c.args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-profile:v",
            "main",
            "-crf",
            "23",
            "-maxrate",
            "2500k",
            "-bufsize",
            "5000k",
        ]),
        Profile::Lossless => c.args(["-c:v", "libx264", "-preset", "ultrafast", "-qp", "0"]),
    };
    c.arg("-g")
        .arg(keyframe_every.to_string())
        .arg("-keyint_min")
        .arg(keyframe_every.to_string())
        .args([
            "-sc_threshold",
            "0",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-an",
        ])
        .arg(out);
    c
}

/// Joins the video and the soundtrack without re-encoding, the moov atom first.
pub fn mux_command(ffmpeg_bin: &Path, video: &Path, audio: &Path, out: &Path) -> Command {
    let mut c = ffmpeg(ffmpeg_bin);
    c.arg("-i")
        .arg(video)
        .arg("-i")
        .arg(audio)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c",
            "copy",
            "-movflags",
            "+faststart",
        ])
        .arg(out);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(c: &Command) -> Vec<String> {
        c.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_encode_is_x264_veryfast_capped_with_a_keyframe_every_60_frames() {
        let a = args(&encode_command(
            Path::new("ffmpeg"),
            Profile::Production,
            (1280, 720),
            30,
            60,
            Path::new("v.mp4"),
        ));
        let joined = a.join(" ");
        for expected in [
            "-f rawvideo -pix_fmt bgra -s 1280x720 -framerate 30 -i pipe:0",
            "-c:v libx264 -preset veryfast -profile:v main -crf 23 -maxrate 2500k -bufsize 5000k -g 60 -keyint_min 60 -sc_threshold 0",
            "-colorspace bt709 -color_primaries bt709 -color_trc bt709 -an v.mp4",
            "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
        ] {
            assert!(joined.contains(expected), "{expected}\n{joined}");
        }
    }

    #[test]
    fn the_reference_profile_is_lossless_and_keeps_the_keyframe_rule() {
        let a = args(&encode_command(
            Path::new("ffmpeg"),
            Profile::Lossless,
            (1280, 720),
            30,
            60,
            Path::new("r.mp4"),
        ));
        let joined = a.join(" ");
        assert!(
            joined.contains("-c:v libx264 -preset ultrafast -qp 0 -g 60 -keyint_min 60"),
            "{joined}"
        );
        assert!(!joined.contains("-crf"));
    }

    #[test]
    fn the_mux_copies_and_puts_the_moov_first() {
        let a = args(&mux_command(
            Path::new("ffmpeg"),
            Path::new("v.mp4"),
            Path::new("a.m4a"),
            Path::new("f.mp4"),
        ));
        assert!(
            a.join(" ")
                .ends_with("-map 0:v:0 -map 1:a:0 -c copy -movflags +faststart f.mp4")
        );
    }
}
