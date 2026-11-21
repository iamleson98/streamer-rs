//! Reads what is in a rendered video and compares videos: everything the report shows, as data.

use std::path::{Path, PathBuf};

use fake_site::Recording;
use replay_model::manifest::{FileKind, ManifestFile, RecordingFile};
use replay_model::timeline::teacher_files;

use crate::cost::Cost;
use crate::metrics::{self, Loudness, MetricError, Series, Tools};
use crate::timeline::{self, Second};

/// A video's own measures.
#[derive(Debug)]
pub struct Analysis {
    pub label: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub duration_s: f64,
    pub keyframes: Vec<f64>,
    pub max_keyframe_gap_s: f64,
    /// The stream runner's own check of the file, as its verdict.
    pub probe: Result<(), String>,
    pub loudness: Option<Loudness>,
    pub timeline: Vec<Second>,
    pub timeline_mismatches: Vec<usize>,
    pub av: Av,
    pub cost: Cost,
}

/// How far the sound and the picture are from where they should be, and from each other.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Av {
    /// Where the microphone is heard against where it was recorded (ms; positive = late).
    pub audio_ms: Option<f64>,
    /// Where the camera's flashes are seen against where they were recorded (ms).
    pub video_ms: Option<f64>,
}

impl Av {
    /// How much later the sound is than the picture (ms).
    pub fn skew_ms(&self) -> Option<f64> {
        Some(self.audio_ms? - self.video_ms?)
    }
}

/// One of the seconds a pair differs most in, as three pictures.
#[derive(Debug)]
pub struct Shot {
    pub second: usize,
    pub value: f64,
    pub a: Vec<u8>,
    pub b: Vec<u8>,
    pub diff: Vec<u8>,
}

/// Two videos against each other: `dist` measured against `reference`.
#[derive(Debug)]
pub struct Pair {
    pub title: String,
    pub dist_label: String,
    pub reference_label: String,
    pub vmaf: Series,
    pub ssim: Series,
    pub worst: Vec<Shot>,
}

fn teacher(recording: &Recording) -> Vec<RecordingFile> {
    let manifest = recording.manifest();
    let files: Vec<RecordingFile> = manifest["files"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, f)| {
            let kind = match f["kind"].as_str()? {
                "voice" => FileKind::Voice,
                "camera" => FileKind::Camera,
                "screen" => FileKind::Screen,
                "screen_audio" => FileKind::ScreenAudio,
                _ => FileKind::Board,
            };
            Some(RecordingFile {
                index: i as u32,
                file: ManifestFile {
                    kind,
                    session_id: f["session_id"].as_str().map(str::to_owned),
                    user_id: f["user_id"].as_str().map(str::to_owned),
                    start_ms: f["start_ms"].as_f64()?,
                    end_ms: f["end_ms"].as_f64()?,
                    width: None,
                    height: None,
                    size: f["size"].as_u64()?,
                },
            })
        })
        .collect();
    teacher_files(&files, fake_site::recording::TEACHER_ID)
}

/// The times (ms on the video's clock) at which the files of `kind` have a flash or beep: the first
/// instant of each second of the file.
fn marks(teacher: &[RecordingFile], kind: FileKind, preroll_ms: f64) -> Vec<f64> {
    teacher
        .iter()
        .filter(|f| f.kind == kind)
        .flat_map(|f| {
            let seconds = ((f.end_ms - f.start_ms) / 1000.0).floor() as u32;
            (0..seconds).map(move |k| preroll_ms + f.start_ms + f64::from(k) * 1000.0)
        })
        .collect()
}

fn av(
    t: &Tools,
    file: &Path,
    recording: &Recording,
    teacher: &[RecordingFile],
    preroll_ms: u64,
    has_camera: bool,
) -> Av {
    let preroll = preroll_ms as f64;
    // The sound: the microphones' own files placed where they belong, against what is heard.
    let audio_ms = (|| -> Result<Option<f64>, MetricError> {
        let heard = metrics::audio_envelope(t, file)?;
        let mut expected = vec![0.0f32; heard.len()];
        for (i, f) in recording.files.iter().enumerate() {
            if f.kind != "voice" || teacher.iter().all(|t| t.index != i as u32) {
                continue;
            }
            let at = (preroll + f.start_ms as f64) as usize;
            for (k, v) in metrics::audio_envelope(t, &f.path)?.into_iter().enumerate() {
                if let Some(slot) = expected.get_mut(at + k) {
                    *slot = v;
                }
            }
        }
        Ok(Some(metrics::best_lag(&heard, &expected, 300) as f64))
    })()
    .ok()
    .flatten();
    // The picture: the camera tile's flashes.
    let video_ms = has_camera
        .then(|| {
            let levels = metrics::box_brightness(t, file, (568 + 20, 632 + 20, 100, 40)).ok()?;
            let seen = metrics::onsets_ms(&levels, 180.0);
            metrics::median_offset(&seen, &marks(teacher, FileKind::Camera, preroll), 300.0)
        })
        .flatten();
    Av { audio_ms, video_ms }
}

/// Measures a rendered video against the recording it was made from.
pub fn analyze(
    t: &Tools,
    label: &str,
    file: &Path,
    recording: &Recording,
    preroll_ms: u64,
    cost: Cost,
) -> Result<Analysis, MetricError> {
    let teacher = teacher(recording);
    let has_camera = teacher.iter().any(|f| f.kind == FileKind::Camera);
    let json = metrics::probe_json(t, file)?;
    let duration_s = serde_json::from_slice::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| v["format"]["duration"].as_str()?.parse::<f64>().ok())
        .unwrap_or(0.0);
    let probe = stream_runner::probe::ProbeOutput::parse(&json)
        .and_then(|p| p.check(recording.duration_ms as i64, preroll_ms))
        .map_err(|e| format!("{e:?}"));
    let keyframes = metrics::keyframes(t, file)?;
    let seconds: Vec<Second> = metrics::second_pictures(t, file)?
        .iter()
        .map(|p| timeline::read_second(p))
        .collect();
    let expected = timeline::expected(&teacher, preroll_ms, seconds.len());
    Ok(Analysis {
        label: label.to_owned(),
        path: file.to_owned(),
        size_bytes: std::fs::metadata(file).map(|m| m.len()).unwrap_or(0),
        duration_s,
        max_keyframe_gap_s: metrics::max_keyframe_gap(&keyframes, duration_s),
        keyframes,
        probe,
        loudness: metrics::loudness(t, file).ok(),
        timeline_mismatches: timeline::mismatches(&seconds, &expected),
        timeline: seconds,
        av: av(t, file, recording, &teacher, preroll_ms, has_camera),
        cost,
    })
}

/// The expected timeline of a recording (what the manifest says).
pub fn expected_timeline(recording: &Recording, preroll_ms: u64, seconds: usize) -> Vec<Second> {
    timeline::expected(&teacher(recording), preroll_ms, seconds)
}

/// Measures `dist` against `reference` frame by frame and takes pictures of the worst `shots`
/// seconds.
pub fn compare(
    t: &Tools,
    title: &str,
    (dist_label, dist): (&str, &Path),
    (reference_label, reference): (&str, &Path),
    work: &Path,
    shots: usize,
) -> Result<Pair, MetricError> {
    std::fs::create_dir_all(work).map_err(|e| MetricError(e.to_string()))?;
    let vmaf = Series::of(&metrics::frame_vmaf(t, dist, reference, work)?);
    let ssim = Series::of(&metrics::frame_ssim(t, dist, reference, work)?);
    let mut worst = Vec::new();
    for (second, value) in vmaf.worst(shots) {
        let at = second as f64 + 0.5;
        worst.push(Shot {
            second,
            value,
            a: metrics::frame_jpeg(t, dist, at, 640)?,
            b: metrics::frame_jpeg(t, reference, at, 640)?,
            diff: metrics::diff_jpeg(t, dist, reference, at, 640)?,
        });
    }
    Ok(Pair {
        title: title.to_owned(),
        dist_label: dist_label.to_owned(),
        reference_label: reference_label.to_owned(),
        vmaf,
        ssim,
        worst,
    })
}
