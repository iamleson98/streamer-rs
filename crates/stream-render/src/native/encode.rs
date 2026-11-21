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
    /// The product's settings with x264 `superfast`: about twice as fast on a static board.
    Fast,
    /// A lossless copy of the composited frames (in 4:2:0), the reference `render-compare` measures
    /// both engines' pictures against. Never used for a render that is uploaded.
    Lossless,
}

/// The video-only encode: frames of `width` x `height` at `fps` on stdin, `out` an MP4 (or, named
/// `.ts`, an MPEG-TS starting at time 0: a piece of a video rendered in parts, which begins on a
/// keyframe and joins the others by [`VideoIn::Pieces`]). `threads` caps x264's own threads.
pub fn encode_command(
    ffmpeg_bin: &Path,
    profile: Profile,
    (width, height): (u32, u32),
    fps: u32,
    keyframe_every: u32,
    threads: Option<u32>,
    out: &Path,
) -> Command {
    let mut c = ffmpeg(ffmpeg_bin);
    if let Some(threads) = threads {
        c.args(["-threads", &threads.to_string()]);
    }
    c.args(["-f", "rawvideo", "-pix_fmt", "bgra", "-s"])
        .arg(format!("{width}x{height}"))
        .arg("-framerate")
        .arg(fps.to_string())
        .args(["-i", "pipe:0", "-vf"])
        .arg("scale=out_color_matrix=bt709:out_range=tv,format=yuv420p");
    match profile {
        Profile::Production | Profile::Fast => c.args([
            "-c:v",
            "libx264",
            "-preset",
            if profile == Profile::Fast {
                "superfast"
            } else {
                "veryfast"
            },
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
        ]);
    if out.extension().is_some_and(|e| e == "ts") {
        c.args(["-muxdelay", "0"]);
    }
    c.arg(out);
    c
}

/// The video a mux takes.
#[derive(Debug, Clone, Copy)]
pub enum VideoIn<'a> {
    /// An MP4 with its own timestamps.
    Mp4(&'a Path),
    /// A list for ffmpeg's concat demuxer of the MPEG-TS pieces of one video, in order.
    Pieces(&'a Path),
}

/// Joins the video and the soundtrack without re-encoding, the moov atom first.
pub fn mux_command(ffmpeg_bin: &Path, video: VideoIn<'_>, audio: &Path, out: &Path) -> Command {
    let mut c = ffmpeg(ffmpeg_bin);
    match video {
        VideoIn::Mp4(path) => c.arg("-i").arg(path),
        VideoIn::Pieces(list) => c.args(["-f", "concat", "-safe", "0", "-i"]).arg(list),
    };
    c.arg("-i")
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
            None,
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
            None,
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
    fn a_piece_names_its_threads() {
        let a = args(&encode_command(
            Path::new("ffmpeg"),
            Profile::Production,
            (1280, 720),
            30,
            60,
            Some(3),
            Path::new("v.h264"),
        ));
        assert!(
            a.join(" ")
                .starts_with("-nostdin -v error -y -threads 3 -f rawvideo")
        );
        assert_eq!(a.last().map(String::as_str), Some("v.h264"));
    }

    #[test]
    fn pieces_are_read_through_the_concat_demuxer() {
        let a = args(&mux_command(
            Path::new("ffmpeg"),
            VideoIn::Pieces(Path::new("list.txt")),
            Path::new("a.m4a"),
            Path::new("f.mp4"),
        ));
        assert!(
            a.join(" ")
                .contains("-f concat -safe 0 -i list.txt -i a.m4a")
        );
    }

    #[test]
    fn the_mux_copies_and_puts_the_moov_first() {
        let a = args(&mux_command(
            Path::new("ffmpeg"),
            VideoIn::Mp4(Path::new("v.mp4")),
            Path::new("a.m4a"),
            Path::new("f.mp4"),
        ));
        assert!(
            a.join(" ")
                .ends_with("-map 0:v:0 -map 1:a:0 -c copy -movflags +faststart f.mp4")
        );
    }
}
