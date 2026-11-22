//! The probe with a REAL ffmpeg and ffprobe. The file is made the way the native engine's output is
//! shaped: H.264 (Main) and AAC in an MP4 with the index first, and an Opus file for what it must
//! refuse. Ignored by default: it needs the tools, by absolute path (the probe runs with an empty
//! environment): `RENDER_TEST_FFMPEG` and `RENDER_TEST_FFPROBE`, built with libx264, libopus and an
//! AAC encoder.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use stream_render::media;

fn tool(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set")))
}

fn make(out: &Path, audio: &str) {
    let status = Command::new(tool("RENDER_TEST_FFMPEG"))
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=size=1280x720:rate=30")
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-t", "3", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .args(["-profile:v", "main", "-g", "60", "-c:a", audio])
        .args(["-movflags", "+faststart"])
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
#[ignore = "needs ffmpeg and ffprobe (RENDER_TEST_FFMPEG, RENDER_TEST_FFPROBE)"]
async fn the_probe_reads_the_length_of_the_uploadable_file_and_refuses_opus() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.mp4");
    make(&good, "aac");
    let ms = media::probe(&tool("RENDER_TEST_FFPROBE"), &good, Duration::from_secs(60))
        .await
        .unwrap();
    assert!((2_900..=3_200).contains(&ms), "{ms} ms");
    assert!(media::length_within_tolerance(ms, 3_000));

    // Opus in an MP4 is not what the stream runner accepts: the probe says so.
    let opus = dir.path().join("opus.mp4");
    make(&opus, "libopus");
    assert!(
        media::probe(&tool("RENDER_TEST_FFPROBE"), &opus, Duration::from_secs(60))
            .await
            .is_err()
    );
}
