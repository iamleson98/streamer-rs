//! The native engine against the fake site, with a real ffmpeg and the real fonts. Ignored by
//! default; run with the tools' paths:
//!
//! ```text
//! RENDER_TEST_FFMPEG=ffmpeg RENDER_TEST_FFPROBE=ffprobe RENDER_TEST_FONTS=target/render-fonts \
//!   cargo test -p stream-render --test native_media -- --ignored --nocapture
//! ```
//!
//! `RENDER_TEST_FONTS` holds the board's fonts and the fallback fonts together (see
//! `crates/stream-render/README.md`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fake_site::{Scenario, build, serve};
use stream_common::api::{ApiClient, RenderClaim};
use stream_common::key::Secret;
use stream_render::card::Card;
use stream_render::config::Settings;
use stream_render::engine::{RenderEngine, RenderJob};
use stream_render::heartbeat::{Heartbeat, HeartbeatTiming};
use stream_render::job::Ended;
use stream_render::native::NativeEngine;
use tokio_util::sync::CancellationToken;

fn tools() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let get = |n| std::env::var(n).ok().map(PathBuf::from);
    Some((
        get("RENDER_TEST_FFMPEG")?,
        get("RENDER_TEST_FFPROBE")?,
        get("RENDER_TEST_FONTS")?,
    ))
}

struct Rendered {
    dir: tempfile::TempDir,
    file: PathBuf,
    requests: Vec<fake_site::Request>,
}

async fn render(scenario: Scenario, secs: u64, preroll_ms: u64) -> Result<Rendered, Ended> {
    render_in(scenario, secs, preroll_ms, None).await
}

/// One render at a time: a debug build of the compositor, its encoders and the fake site on every
/// core starves the heartbeat of the render next to it into a lost lease.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `pieces`: how many pieces of the video to render at once (`None`: the engine's own count).
async fn render_in(
    scenario: Scenario,
    secs: u64,
    preroll_ms: u64,
    pieces: Option<usize>,
) -> Result<Rendered, Ended> {
    let _turn = ONE_AT_A_TIME.lock().await;
    let (ffmpeg, ffprobe, fonts) = tools().expect("the tools' paths");
    let dir = tempfile::tempdir().unwrap();
    let recording = build(scenario, &ffmpeg, &dir.path().join("recording"), secs).unwrap();
    let site = serve(recording).await.unwrap();
    let claim: RenderClaim = serde_json::from_value(serde_json::json!({
        "request_id": "req1", "recording_id": "recording0000000000001",
        "duration_ms": secs * 1000, "render_token": "first-token-SECRET",
        "token_expires_at": 1, "lease_until": 1, "locale": "vi",
        "preroll_ms": preroll_ms, "card_title": "Nhập môn đại số", "card_teacher": "Cô Lan",
    }))
    .unwrap();
    let card = Card::from_claim(&claim);
    let mut settings = Settings::new("test");
    settings.ffmpeg = ffmpeg;
    settings.ffprobe = ffprobe;
    settings.retry_delay = Duration::from_millis(10);
    let api = ApiClient::new(
        &site.api,
        Secret::new("worker-token"),
        Duration::from_secs(5),
    )
    .unwrap();
    let heartbeat = Heartbeat::start(
        api,
        "test".into(),
        "req1".into(),
        HeartbeatTiming {
            interval: Duration::from_millis(200),
            lease_timeout: Duration::from_secs(30),
        },
    );
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let shutdown = CancellationToken::new();
    let mut engine = NativeEngine::new(&fonts).expect("the fonts");
    if let Some(pieces) = pieces {
        engine = engine.with_chunks(pieces);
    }
    let file_ms = card.file_ms(claim.duration_ms);
    let result = engine
        .render(RenderJob {
            claim: &claim,
            card: &card,
            file_ms,
            dir: &work,
            settings: &settings,
            api_base_url: &site.api,
            heartbeat: &heartbeat,
            shutdown: &shutdown,
        })
        .await;
    heartbeat.finish().await;
    result.map(|file| Rendered {
        dir,
        file,
        requests: site.requests(),
    })
}

fn probe(file: &Path, args: &[&str]) -> String {
    let (_, ffprobe, _) = tools().unwrap();
    let out = Command::new(ffprobe)
        .args(["-v", "error"])
        .args(args)
        .arg(file)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// The top-level boxes of an MP4, in order.
fn atoms(file: &Path) -> Vec<String> {
    let bytes = std::fs::read(file).unwrap();
    let (mut at, mut names) = (0, Vec::new());
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        names.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        at += size.max(8);
    }
    names
}

fn check_file(r: &Rendered, expect_s: f64) {
    let info = probe(
        &r.file,
        &[
            "-show_entries",
            "stream=codec_type,codec_name,profile,width,height:format=duration",
            "-of",
            "compact",
        ],
    );
    assert!(
        info.contains("codec_name=h264")
            && info.contains("profile=Main")
            && info.contains("width=1280|height=720"),
        "{info}"
    );
    assert!(info.contains("codec_name=aac"), "{info}");
    let duration: f64 = info
        .lines()
        .find_map(|l| l.strip_prefix("format|duration="))
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (duration - expect_s).abs() < 0.3,
        "duration {duration} vs {expect_s}"
    );
    let names = atoms(&r.file);
    let (moov, mdat) = (
        names.iter().position(|n| n == "moov"),
        names.iter().position(|n| n == "mdat"),
    );
    assert!(moov < mdat, "moov must come first: {names:?}");
    // A keyframe at least every 2 seconds.
    let keys = probe(
        &r.file,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=p=0",
        ],
    );
    let times: Vec<f64> = keys
        .lines()
        .filter(|l| l.contains('K'))
        .map(|l| l.split(',').next().unwrap().parse().unwrap())
        .collect();
    assert!(times.windows(2).all(|w| w[1] - w[0] <= 2.001), "{times:?}");
    assert_eq!(times[0], 0.0);
}

fn assert_token_handling(r: &Rendered) {
    assert!(r.requests.iter().any(|q| q.path.starts_with("manifest")));
    for q in &r.requests {
        if q.path.starts_with("storage/") {
            assert_eq!(
                q.authorization, None,
                "storage must not see the token: {q:?}"
            );
        }
        if q.path.starts_with("files/") || q.path == "manifest" {
            assert!(
                q.authorization
                    .as_deref()
                    .is_some_and(|a| a.starts_with("Bearer ")),
                "{q:?}"
            );
        }
    }
    // Nothing the worker leaves in its directory carries the token either.
    for entry in std::fs::read_dir(r.dir.path().join("work"))
        .unwrap()
        .flatten()
    {
        let bytes = std::fs::read(entry.path()).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&bytes).contains("first-token-SECRET"));
    }
}

#[tokio::test]
#[ignore = "needs ffmpeg, ffprobe and the fonts (RENDER_TEST_*)"]
async fn a_board_lesson_with_a_card_renders_to_the_right_file() {
    let r = render(Scenario::BoardOnly, 6, 60_000).await.unwrap();
    check_file(&r, 66.0);
    assert_token_handling(&r);
}

#[tokio::test]
#[ignore = "needs ffmpeg, ffprobe and the fonts (RENDER_TEST_*)"]
async fn a_screen_share_a_camera_and_a_gap_render_and_leave_no_files_behind() {
    let r = render(Scenario::GapAndRotation, 10, 0).await.unwrap();
    check_file(&r, 10.0);
    assert_token_handling(&r);
    let left: Vec<String> = std::fs::read_dir(r.dir.path().join("work"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".webm") || n.ends_with(".part"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
#[ignore = "needs ffmpeg, ffprobe and the fonts (RENDER_TEST_*)"]
async fn decks_and_the_camera_render() {
    check_file(&render(Scenario::Decks, 8, 0).await.unwrap(), 8.0);
    check_file(&render(Scenario::Camera, 6, 0).await.unwrap(), 6.0);
}

#[tokio::test]
#[ignore = "needs ffmpeg, ffprobe and the fonts (RENDER_TEST_*)"]
async fn a_quiz_and_a_quest_render_instead_of_failing() {
    check_file(&render(Scenario::Quiz, 12, 0).await.unwrap(), 12.0);
    check_file(&render(Scenario::Quest, 12, 0).await.unwrap(), 12.0);
}

#[tokio::test]
#[ignore = "needs ffmpeg, ffprobe and the fonts (RENDER_TEST_*)"]
async fn a_video_rendered_in_pieces_has_every_frame_once_and_a_keyframe_every_two_seconds() {
    // A card of 60 s, a screen share and a camera, in 4 pieces: the joins fall inside the files.
    let r = render_in(Scenario::GapAndRotation, 30, 60_000, Some(4))
        .await
        .unwrap();
    check_file(&r, 90.0);
    let times = probe(
        &r.file,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
        ],
    );
    let mut pts: Vec<f64> = times.lines().map(|l| l.parse().unwrap()).collect();
    pts.sort_by(f64::total_cmp);
    assert_eq!(pts.len(), 90 * 30);
    for (i, t) in pts.iter().enumerate() {
        assert!(
            (t - i as f64 / 30.0).abs() < 0.001,
            "frame {i} is at {t} (a frame twice or missing)"
        );
    }
}
