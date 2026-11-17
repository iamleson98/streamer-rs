//! The remux and the probe with a REAL ffmpeg and ffprobe. The input is made the way the render
//! page's output is shaped: H.264 and Opus in a fragmented MP4. Ignored by default: it needs the
//! tools (`ffmpeg` and `ffprobe` on `PATH`, built with libx264 and libopus).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use stream_render::media;

#[tokio::test]
#[ignore = "needs ffmpeg and ffprobe with libx264 and libopus on PATH"]
async fn the_real_tools_turn_a_fragmented_mp4_with_opus_into_the_uploadable_file() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=1280x720:rate=30",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args([
            "-t",
            "3",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-profile:v",
            "main",
            "-g",
            "60",
        ])
        .args([
            "-c:a",
            "libopus",
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof",
        ])
        .arg(&raw)
        .status()
        .unwrap();
    assert!(status.success());

    let out = dir.path().join("final.mp4");
    media::remux(Path::new("ffmpeg"), &raw, &out, Duration::from_secs(60))
        .await
        .unwrap();
    let ms = media::probe(Path::new("ffprobe"), &out, Duration::from_secs(60))
        .await
        .unwrap();
    assert!((2_900..=3_200).contains(&ms), "{ms} ms");
    assert!(media::length_within_tolerance(ms, 3_000));
    assert!(
        std::fs::metadata(&out).unwrap().len() > 0,
        "the finished file exists"
    );

    // The raw file itself is not what the stream runner accepts (Opus): the probe says so.
    assert!(
        media::probe(Path::new("ffprobe"), &raw, Duration::from_secs(60))
            .await
            .is_err()
    );
}
