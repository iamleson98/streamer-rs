//! What is measured of a video with ffmpeg and ffprobe: the per-frame SSIM and VMAF of one video
//! against another, loudness, keyframes, the probe the stream runner does, and the raw pictures and
//! sound the timeline and the A/V checks read. Every measure runs an external process and parses
//! its output; the parsers are pure and tested.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const FPS: usize = 30;

/// The tools: the ffmpeg that renders and probes, and the one that has libvmaf.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub vmaf_ffmpeg: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct MetricError(pub String);

fn fail(what: impl Into<String>) -> MetricError {
    MetricError(what.into())
}

fn run(
    tool: &Path,
    cwd: Option<&Path>,
    args: &[String],
) -> Result<std::process::Output, MetricError> {
    let mut c = Command::new(tool);
    c.args(args).stdin(Stdio::null());
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    let out = c
        .output()
        .map_err(|e| fail(format!("cannot run {}: {e}", tool.display())))?;
    if out.status.success() {
        Ok(out)
    } else {
        let text = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<&str> = text.lines().collect();
        Err(fail(lines[lines.len().saturating_sub(4)..].join(" | ")))
    }
}

fn s(v: impl AsRef<std::ffi::OsStr>) -> String {
    v.as_ref().to_string_lossy().into_owned()
}

/// Whether `ffmpeg` has the libvmaf filter.
pub fn has_vmaf(ffmpeg: &Path) -> bool {
    run(ffmpeg, None, &["-hide_banner".into(), "-filters".into()])
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("libvmaf"))
        .unwrap_or(false)
}

/// Both videos on the same 30 fps grid at 1280x720 in 4:2:0, so frame `n` meets frame `n`.
fn aligned(label_a: &str, label_b: &str, then: &str) -> String {
    let grid = |i: usize, label: &str| {
        format!("[{i}:v]scale=1280:720:flags=bicubic,format=yuv420p,setpts=N/{FPS}/TB[{label}]")
    };
    format!(
        "{};{};[{label_a}][{label_b}]{then}",
        grid(0, label_a),
        grid(1, label_b)
    )
}

/// The SSIM of each frame of `dist` against `reference`.
pub fn frame_ssim(
    t: &Tools,
    dist: &Path,
    reference: &Path,
    work: &Path,
) -> Result<Vec<f64>, MetricError> {
    let stats = work.join("ssim.log");
    let graph = aligned("a", "b", "ssim=stats_file=ssim.log");
    run(
        &t.ffmpeg,
        Some(work),
        &[
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-i".into(),
            s(dist),
            "-i".into(),
            s(reference),
            "-filter_complex".into(),
            graph,
            "-f".into(),
            "null".into(),
            "-".into(),
        ],
    )?;
    let text = std::fs::read_to_string(&stats).map_err(|e| fail(e.to_string()))?;
    Ok(parse_ssim(&text))
}

/// `n:1 Y:0.99 U:0.99 V:0.99 All:0.993 (21.5)` lines to the `All` values.
pub fn parse_ssim(text: &str) -> Vec<f64> {
    text.lines()
        .filter_map(|l| {
            l.split(" All:")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

/// The VMAF of each frame of `dist` against `reference` (model `vmaf_v0.6.1`).
pub fn frame_vmaf(
    t: &Tools,
    dist: &Path,
    reference: &Path,
    work: &Path,
) -> Result<Vec<f64>, MetricError> {
    let graph = aligned(
        "a",
        "b",
        "libvmaf=log_fmt=json:log_path=vmaf.json:n_threads=8:model=version=vmaf_v0.6.1",
    );
    run(
        &t.vmaf_ffmpeg,
        Some(work),
        &[
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-i".into(),
            s(dist),
            "-i".into(),
            s(reference),
            "-filter_complex".into(),
            graph,
            "-f".into(),
            "null".into(),
            "-".into(),
        ],
    )?;
    let json = std::fs::read(work.join("vmaf.json")).map_err(|e| fail(e.to_string()))?;
    parse_vmaf(&json)
}

pub fn parse_vmaf(json: &[u8]) -> Result<Vec<f64>, MetricError> {
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|_| fail("the VMAF log is not JSON"))?;
    let frames = v["frames"]
        .as_array()
        .ok_or_else(|| fail("the VMAF log has no frames"))?;
    Ok(frames
        .iter()
        .filter_map(|f| f["metrics"]["vmaf"].as_f64())
        .collect())
}

/// Mean of each second's frames (the last second may be shorter).
pub fn per_second(values: &[f64]) -> Vec<f64> {
    values
        .chunks(FPS)
        .map(|c| c.iter().sum::<f64>() / c.len() as f64)
        .collect()
}

/// A series' summary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Series {
    pub per_second: Vec<f64>,
    pub mean: f64,
    pub min: f64,
}

impl Series {
    pub fn of(frames: &[f64]) -> Self {
        let per_second = per_second(frames);
        let mean = if frames.is_empty() {
            0.0
        } else {
            frames.iter().sum::<f64>() / frames.len() as f64
        };
        let min = per_second.iter().copied().fold(f64::INFINITY, f64::min);
        Self {
            per_second,
            mean,
            min: if min.is_finite() { min } else { 0.0 },
        }
    }

    /// The `n` seconds with the lowest value, worst first.
    pub fn worst(&self, n: usize) -> Vec<(usize, f64)> {
        let mut v: Vec<(usize, f64)> = self.per_second.iter().copied().enumerate().collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
        v.truncate(n);
        v
    }
}

/// Loudness of the audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    pub integrated_lufs: f64,
    pub true_peak_dbtp: f64,
}

pub fn loudness(t: &Tools, file: &Path) -> Result<Loudness, MetricError> {
    let out = run(
        &t.ffmpeg,
        None,
        &[
            "-nostdin".into(),
            "-hide_banner".into(),
            "-nostats".into(),
            "-i".into(),
            s(file),
            "-map".into(),
            "0:a:0".into(),
            "-af".into(),
            "ebur128=peak=true".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ],
    )?;
    parse_ebur128(&String::from_utf8_lossy(&out.stderr)).ok_or_else(|| fail("no loudness summary"))
}

/// The summary at the end of `ebur128`'s output.
pub fn parse_ebur128(text: &str) -> Option<Loudness> {
    let summary = &text[text.rfind("Summary:")?..];
    let number = |key: &str, unit: &str| -> Option<f64> {
        let rest = &summary[summary.find(key)? + key.len()..];
        rest.split(unit).next()?.trim().parse().ok()
    };
    Some(Loudness {
        integrated_lufs: number("I:", "LUFS")?,
        true_peak_dbtp: number("Peak:", "dBFS")?,
    })
}

/// ffprobe's JSON for the runner's own check.
pub fn probe_json(t: &Tools, file: &Path) -> Result<Vec<u8>, MetricError> {
    Ok(run(
        &t.ffprobe,
        None,
        &[
            "-v".into(),
            "error".into(),
            "-print_format".into(),
            "json".into(),
            "-show_streams".into(),
            "-show_format".into(),
            s(file),
        ],
    )?
    .stdout)
}

/// The times (s) of the video's keyframes.
pub fn keyframes(t: &Tools, file: &Path) -> Result<Vec<f64>, MetricError> {
    let out = run(
        &t.ffprobe,
        None,
        &[
            "-v".into(),
            "error".into(),
            "-select_streams".into(),
            "v:0".into(),
            "-show_entries".into(),
            "packet=pts_time,flags".into(),
            "-of".into(),
            "csv=p=0".into(),
            s(file),
        ],
    )?;
    Ok(parse_keyframes(&String::from_utf8_lossy(&out.stdout)))
}

pub fn parse_keyframes(csv: &str) -> Vec<f64> {
    let mut times: Vec<f64> = csv
        .lines()
        .filter(|l| l.split(',').nth(1).is_some_and(|f| f.contains('K')))
        .filter_map(|l| l.split(',').next()?.parse().ok())
        .collect();
    times.sort_by(f64::total_cmp);
    times
}

/// The largest gap between keyframes, and from the last to the end.
pub fn max_keyframe_gap(keys: &[f64], duration_s: f64) -> f64 {
    let mut edges = keys.to_vec();
    edges.push(duration_s);
    edges.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max)
}

/// Raw frames of `file` through `filter`, `bytes` per frame.
fn raw_frames(
    t: &Tools,
    file: &Path,
    pre: &[&str],
    filter: &str,
    pix_fmt: &str,
    bytes: usize,
) -> Result<Vec<Vec<u8>>, MetricError> {
    let mut args: Vec<String> = vec!["-nostdin".into(), "-v".into(), "error".into()];
    args.extend(pre.iter().map(|a| (*a).to_owned()));
    args.extend([
        "-i".into(),
        s(file),
        "-an".into(),
        "-vf".into(),
        filter.into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        pix_fmt.into(),
        "pipe:1".into(),
    ]);
    let out = run(&t.ffmpeg, None, &args)?;
    Ok(out.stdout.chunks_exact(bytes).map(<[u8]>::to_vec).collect())
}

/// One small RGB picture per second, taken half a second in: 320 x 180.
pub fn second_pictures(t: &Tools, file: &Path) -> Result<Vec<Vec<u8>>, MetricError> {
    raw_frames(
        t,
        file,
        &["-ss", "0.5"],
        "fps=1,scale=320:180:flags=area",
        "rgb24",
        320 * 180 * 3,
    )
}

/// The mean brightness (0-255) of the box at (x, y) of w x h in every frame.
pub fn box_brightness(
    t: &Tools,
    file: &Path,
    (x, y, w, h): (u32, u32, u32, u32),
) -> Result<Vec<f64>, MetricError> {
    let filter = format!("crop={w}:{h}:{x}:{y},scale=1:1:flags=area");
    Ok(raw_frames(t, file, &[], &filter, "gray", 1)?
        .into_iter()
        .map(|f| f64::from(f[0]))
        .collect())
}

/// The audio's envelope at 1 kHz: the largest absolute sample of every millisecond.
pub fn audio_envelope(t: &Tools, file: &Path) -> Result<Vec<f32>, MetricError> {
    let out = run(
        &t.ffmpeg,
        None,
        &[
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-i".into(),
            s(file),
            "-vn".into(),
            "-ac".into(),
            "1".into(),
            "-ar".into(),
            "8000".into(),
            "-f".into(),
            "f32le".into(),
            "pipe:1".into(),
        ],
    )?;
    let samples: Vec<f32> = out
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    Ok(samples
        .chunks(8)
        .map(|c| c.iter().fold(0.0f32, |m, v| m.max(v.abs())))
        .collect())
}

/// The lag (ms) at which `heard` best matches `expected`, searched within `max_lag` either way:
/// positive when what was heard is later than expected.
pub fn best_lag(heard: &[f32], expected: &[f32], max_lag: i64) -> i64 {
    let (mut best, mut best_score) = (0, f64::MIN);
    for lag in -max_lag..=max_lag {
        let score: f64 = expected
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                heard
                    .get(usize::try_from(i as i64 + lag).ok()?)
                    .map(|h| f64::from(*h) * f64::from(*e))
            })
            .sum();
        if score > best_score {
            (best, best_score) = (lag, score);
        }
    }
    best
}

/// The times (ms) at which `levels` (one value per frame at 30 fps) rises through `threshold`.
pub fn onsets_ms(levels: &[f64], threshold: f64) -> Vec<f64> {
    (0..levels.len())
        .filter(|&i| levels[i] >= threshold && (i == 0 || levels[i - 1] < threshold))
        .map(|i| i as f64 * 1000.0 / FPS as f64)
        .collect()
}

/// The median of what `observed` is off from the nearest `expected` time within `window_ms`.
pub fn median_offset(observed: &[f64], expected: &[f64], window_ms: f64) -> Option<f64> {
    let mut offsets: Vec<f64> = expected
        .iter()
        .filter_map(|e| {
            observed
                .iter()
                .map(|o| o - e)
                .filter(|d| d.abs() <= window_ms)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
        })
        .collect();
    offsets.sort_by(f64::total_cmp);
    (!offsets.is_empty()).then(|| offsets[offsets.len() / 2])
}

/// A single frame of `file` at `t_s` as a JPEG `width` pixels wide.
pub fn frame_jpeg(t: &Tools, file: &Path, t_s: f64, width: u32) -> Result<Vec<u8>, MetricError> {
    Ok(run(
        &t.ffmpeg,
        None,
        &[
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-ss".into(),
            format!("{t_s:.3}"),
            "-i".into(),
            s(file),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            format!("scale={width}:-1"),
            "-q:v".into(),
            "5".into(),
            "-f".into(),
            "mjpeg".into(),
            "pipe:1".into(),
        ],
    )?
    .stdout)
}

/// The difference of two videos' frames at `t_s`, brightened, as a JPEG.
pub fn diff_jpeg(
    t: &Tools,
    a: &Path,
    b: &Path,
    t_s: f64,
    width: u32,
) -> Result<Vec<u8>, MetricError> {
    let graph = format!(
        "[0:v]scale={width}:-1,format=gbrp[a];[1:v]scale={width}:-1,format=gbrp[b];[a][b]blend=all_mode=difference,eq=contrast=5:brightness=0.02"
    );
    Ok(run(
        &t.ffmpeg,
        None,
        &[
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-ss".into(),
            format!("{t_s:.3}"),
            "-i".into(),
            s(a),
            "-ss".into(),
            format!("{t_s:.3}"),
            "-i".into(),
            s(b),
            "-filter_complex".into(),
            graph,
            "-frames:v".into(),
            "1".into(),
            "-q:v".into(),
            "5".into(),
            "-f".into(),
            "mjpeg".into(),
            "pipe:1".into(),
        ],
    )?
    .stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssim_lines_give_their_all_value() {
        let log =
            "n:1 Y:0.99 U:0.98 V:0.97 All:0.985 (18.2)\nn:2 Y:1 U:1 V:1 All:1.000000 (inf)\nnoise";
        assert_eq!(parse_ssim(log), [0.985, 1.0]);
    }

    #[test]
    fn vmaf_frames_are_read_and_a_log_without_frames_is_refused() {
        let json = br#"{"frames":[{"metrics":{"vmaf":97.5}},{"metrics":{"vmaf":88.0}}]}"#;
        assert_eq!(parse_vmaf(json).unwrap(), [97.5, 88.0]);
        assert!(parse_vmaf(b"{}").is_err());
    }

    #[test]
    fn seconds_are_the_mean_of_thirty_frames_and_the_worst_are_listed_first() {
        let mut frames = vec![100.0; 60];
        frames.extend(vec![50.0; 30]);
        let series = Series::of(&frames);
        assert_eq!(series.per_second, [100.0, 100.0, 50.0]);
        assert_eq!(series.worst(1), [(2, 50.0)]);
        assert!((series.mean - 83.333).abs() < 0.01);
        assert_eq!(series.min, 50.0);
    }

    #[test]
    fn the_ebur128_summary_is_parsed() {
        let text = "noise\n[Parsed_ebur128_0 @ 0x1] Summary:\n\n  Integrated loudness:\n    I:         -23.4 LUFS\n    Threshold: -33.5 LUFS\n\n  True peak:\n    Peak:       -3.1 dBFS\n";
        assert_eq!(
            parse_ebur128(text),
            Some(Loudness {
                integrated_lufs: -23.4,
                true_peak_dbtp: -3.1
            })
        );
        assert_eq!(parse_ebur128("nothing"), None);
    }

    #[test]
    fn keyframes_are_the_packets_flagged_k_and_the_gap_includes_the_tail() {
        let csv = "0.000000,K__\n0.033000,___\n2.000000,K__\n4.000000,K_\n";
        let keys = parse_keyframes(csv);
        assert_eq!(keys, [0.0, 2.0, 4.0]);
        assert_eq!(max_keyframe_gap(&keys, 5.0), 2.0);
        assert_eq!(max_keyframe_gap(&keys, 7.0), 3.0);
    }

    #[test]
    fn the_best_lag_finds_a_shifted_copy_and_the_median_offset_ignores_a_stray_onset() {
        let mut expected = vec![0.0f32; 200];
        expected[50] = 1.0;
        expected[120] = 1.0;
        let mut heard = vec![0.0f32; 200];
        heard[57] = 1.0;
        heard[127] = 1.0;
        assert_eq!(best_lag(&heard, &expected, 30), 7);
        let levels: Vec<f64> = (0..70)
            .map(|i| if i % 30 < 3 { 235.0 } else { 100.0 })
            .collect();
        let onsets = onsets_ms(&levels, 180.0);
        assert_eq!(onsets.len(), 3);
        assert_eq!(
            median_offset(&[10.0, 1010.0, 5000.0], &[0.0, 1000.0, 2000.0], 300.0),
            Some(10.0)
        );
        assert_eq!(median_offset(&[], &[0.0], 300.0), None);
    }
}
