//! One ffmpeg child per video file: it decodes the file, puts each picture on the lesson's 30 fps
//! grid, scales it into its box and pads it to the box's size on black, and writes raw BGRA to a
//! pipe. The compositor reads one frame per output frame, so the pipe's own back-pressure bounds
//! the memory: nothing is buffered here beyond the one frame in hand.
//!
//! The grid reproduces the web player's `frameAt`: the picture shown at lesson time `t` is the
//! latest one at or before `t` plus half a millisecond. `setpts` moves every picture half a
//! millisecond earlier and `fps` with `round=up` puts a picture on the first slot at or after its
//! time, so slot `k` (lesson frame `j0 + k`) holds the latest picture at or before it.

use std::io::Read;
use std::path::Path;
use std::process::{ChildStdout, Command};

use super::ffmpeg::{Proc, Registry, ffmpeg};

/// A picture is shown while its time is at or before the frame's time plus this (seconds).
pub const TOLERANCE_S: f64 = 0.0005;

/// The first lesson frame at or after `start_ms`, and how far into the file that frame's time is
/// (seconds, never negative): the origin of the decoder's grid.
pub fn grid_origin(start_ms: f64, fps: u32) -> (i64, f64) {
    let j0 = (start_ms * f64::from(fps) / 1000.0).ceil().max(0.0) as i64;
    let t0 = j0 as f64 * 1000.0 / f64::from(fps);
    (j0, ((t0 - start_ms) / 1000.0).max(0.0))
}

/// The filter graph of a decoder for a `w` x `h` box whose grid starts `origin_s` into the file.
pub fn filter(w: u32, h: u32, origin_s: f64, fps: u32) -> String {
    format!(
        "setpts=PTS-{TOLERANCE_S}/TB,fps={fps}:start_time={origin_s:.6}:round=up,\
         scale={w}:{h}:force_original_aspect_ratio=decrease:flags=bicubic,\
         pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,format=bgra"
    )
}

/// The decoder's command line.
pub fn command(ffmpeg_bin: &Path, file: &Path, w: u32, h: u32, origin_s: f64, fps: u32) -> Command {
    let mut c = ffmpeg(ffmpeg_bin);
    c.args(["-threads", "2", "-i"])
        .arg(file)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-fps_mode",
            "passthrough",
            "-vf",
        ])
        .arg(filter(w, h, origin_s, fps))
        .args(["-f", "rawvideo", "pipe:1"]);
    c
}

/// A running decoder and the frame last read from it.
#[derive(Debug)]
pub struct Decoder {
    _proc: Proc,
    out: ChildStdout,
    frame: Vec<u8>,
    has_frame: bool,
    ended: bool,
    /// How many frames have been read.
    pub read: u64,
}

impl Decoder {
    pub fn spawn(
        registry: &Registry,
        ffmpeg_bin: &Path,
        file: &Path,
        (w, h): (u32, u32),
        origin_s: f64,
        fps: u32,
        log: &Path,
    ) -> std::io::Result<Self> {
        let mut proc = registry.spawn(&mut command(ffmpeg_bin, file, w, h, origin_s, fps), log)?;
        let out = proc
            .child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("no stdout"))?;
        drop(proc.child.stdin.take());
        Ok(Self {
            _proc: proc,
            out,
            frame: vec![0; (w * h * 4) as usize],
            has_frame: false,
            ended: false,
            read: 0,
        })
    }

    /// Reads the next frame of the grid. At the end of the stream the last frame stays (a picture
    /// is held until its file ends). Returns whether a new frame was read.
    pub fn advance(&mut self) -> bool {
        if self.ended {
            return false;
        }
        match self.out.read_exact(&mut self.frame) {
            Ok(()) => {
                self.has_frame = true;
                self.read += 1;
                true
            }
            Err(_) => {
                self.ended = true;
                false
            }
        }
    }

    /// The picture on the grid slot last read, once there is one.
    pub fn frame(&self) -> Option<&[u8]> {
        self.has_frame.then_some(self.frame.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_starts_on_the_first_lesson_frame_at_or_after_the_file() {
        assert_eq!(grid_origin(0.0, 30), (0, 0.0));
        let (j0, origin) = grid_origin(1000.0, 30);
        assert_eq!((j0, origin), (30, 0.0));
        let (j0, origin) = grid_origin(1010.0, 30);
        assert_eq!(j0, 31);
        // Frame 31 is at 1033.33 ms: 23.33 ms into a file that began at 1010 ms.
        assert!((origin - 0.023_333).abs() < 1e-5);
        assert_eq!(grid_origin(-50.0, 30).0, 0);
    }

    #[test]
    fn the_filter_shifts_rounds_up_and_pads_on_black() {
        let f = filter(1280, 624, 0.0233333, 30);
        assert_eq!(
            f,
            "setpts=PTS-0.0005/TB,fps=30:start_time=0.023333:round=up,\
             scale=1280:624:force_original_aspect_ratio=decrease:flags=bicubic,\
             pad=1280:624:(ow-iw)/2:(oh-ih)/2:color=black,format=bgra"
        );
    }
}
