//! The soundtrack: the teacher's microphones and screen sound, each placed at its offset with
//! `adelay` (the countdown card's silence is just the preroll added to every offset), summed with
//! `amix` and held below full scale by the web exporter's soft limiter, as one AAC file. Port of
//! `export/audio-mix.ts`.
//!
//! It is its own ffmpeg step, so a lesson with hundreds of sound files does not share a process
//! with the video pipe.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::ffmpeg::ffmpeg;

pub const SAMPLE_RATE: u32 = 48_000;

/// A sound file and where it starts on the lesson's timeline (ms).
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub path: PathBuf,
    pub start_ms: f64,
}

/// `limit` of `audio-mix.ts`: unchanged under the knee, rounded off towards 1 above it.
const LIMITER: &str = "aeval='if(lte(abs(val(0)),0.9),val(0),sgn(val(0))*(0.9+0.1*tanh((abs(val(0))-0.9)/0.1)))':c=same";

/// The filter graph for `sounds`; the output pad is `[out]`.
pub fn graph(sounds: &[Sound], preroll_ms: u64, file_ms: i64) -> String {
    let mut parts = Vec::new();
    for (k, sound) in sounds.iter().enumerate() {
        let delay = ((preroll_ms as f64 + sound.start_ms) * f64::from(SAMPLE_RATE) / 1000.0)
            .round()
            .max(0.0) as u64;
        parts.push(format!(
            "[{k}:a]aresample={SAMPLE_RATE}:async=1:first_pts=0,\
             aformat=sample_fmts=fltp:channel_layouts=mono,adelay=delays={delay}S:all=1[a{k}]"
        ));
    }
    let inputs: String = (0..sounds.len()).map(|k| format!("[a{k}]")).collect();
    let seconds = file_ms as f64 / 1000.0;
    parts.push(format!(
        "{inputs}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0,{LIMITER},\
         apad=whole_dur={file_ms}ms,atrim=end={seconds:.3}[out]",
        sounds.len()
    ));
    parts.join(";")
}

/// The command that writes `out` (AAC, mono, 48 kHz, `file_ms` long). With no sound at all the
/// soundtrack is silence.
pub fn command(
    ffmpeg_bin: &Path,
    sounds: &[Sound],
    preroll_ms: u64,
    file_ms: i64,
    out: &Path,
) -> Command {
    let mut c = ffmpeg(ffmpeg_bin);
    if sounds.is_empty() {
        c.args(["-f", "lavfi", "-t"])
            .arg(format!("{:.3}", file_ms as f64 / 1000.0))
            .arg("-i")
            .arg(format!("anullsrc=r={SAMPLE_RATE}:cl=mono"));
    } else {
        for sound in sounds {
            c.arg("-i").arg(&sound.path);
        }
        c.arg("-filter_complex")
            .arg(graph(sounds, preroll_ms, file_ms))
            .args(["-map", "[out]"]);
    }
    c.args(["-c:a", "aac", "-b:a", "128k", "-ac", "1", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .arg(out);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(name: &str, start_ms: f64) -> Sound {
        Sound {
            path: PathBuf::from(name),
            start_ms,
        }
    }

    #[test]
    fn each_sound_is_delayed_by_the_card_and_its_offset_in_samples() {
        let g = graph(
            &[sound("a.webm", 0.0), sound("b.webm", 2500.0)],
            60_000,
            70_000,
        );
        assert!(g.contains("[0:a]aresample=48000:async=1:first_pts=0,aformat=sample_fmts=fltp:channel_layouts=mono,adelay=delays=2880000S:all=1[a0]"));
        // 62.5 s at 48 kHz.
        assert!(g.contains("adelay=delays=3000000S:all=1[a1]"));
        assert!(g.contains(
            "[a0][a1]amix=inputs=2:duration=longest:dropout_transition=0:normalize=0,aeval="
        ));
        assert!(g.ends_with("apad=whole_dur=70000ms,atrim=end=70.000[out]"));
    }

    #[test]
    fn the_command_has_one_input_per_sound_and_no_secret_only_paths() {
        let c = command(
            Path::new("ffmpeg"),
            &[sound("/w/1.webm", 0.0)],
            0,
            5000,
            Path::new("/w/audio.m4a"),
        );
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 1);
        assert!(args.contains(&"/w/1.webm".to_owned()));
        assert_eq!(args.last().unwrap(), "/w/audio.m4a");
        assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
    }

    #[test]
    fn no_sound_is_silence_of_the_whole_length() {
        let c = command(Path::new("ffmpeg"), &[], 0, 12_500, Path::new("a.m4a"));
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"anullsrc=r=48000:cl=mono".to_owned()));
        assert!(args.contains(&"12.500".to_owned()));
        assert!(!args.contains(&"-filter_complex".to_owned()));
    }
}
