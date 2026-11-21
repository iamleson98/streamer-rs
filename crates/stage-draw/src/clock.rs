//! The export's clock. A render is not played: output frame `i` is the picture at
//! `t = i * 1000 / 30` ms of the recording, however long it takes to make. Port of
//! `export/frame-clock.ts`.

pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
pub const FPS: u32 = 30;
pub const SAMPLE_RATE: u32 = 48_000;
/// A keyframe every 2 s.
pub const KEYFRAME_EVERY: u32 = 2 * FPS;

/// Where output frame `i` falls on the recording's timeline (ms); negative on the card.
pub fn frame_time_ms(lesson_frame: i64) -> f64 {
    lesson_frame as f64 * 1000.0 / f64::from(FPS)
}

/// How many frames a recording of `duration_ms` makes.
pub fn frame_count(duration_ms: f64) -> u64 {
    (duration_ms * f64::from(FPS) / 1000.0).floor().max(0.0) as u64
}

/// How many card frames come before the lesson's first one.
pub fn preroll_frames(preroll_ms: u64) -> u64 {
    (preroll_ms as f64 * f64::from(FPS) / 1000.0).round() as u64
}

/// How many frames the whole video makes: the card, then the lesson.
pub fn total_frames(duration_ms: f64, preroll_ms: u64) -> u64 {
    preroll_frames(preroll_ms) + frame_count(duration_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_counts_whole_frames() {
        assert_eq!(frame_count(1000.0), 30);
        assert_eq!(frame_count(1033.0), 30);
        assert_eq!(frame_count(1034.0), 31);
        assert_eq!(preroll_frames(60_000), 1800);
        assert_eq!(total_frames(2000.0, 60_000), 1860);
        assert_eq!(frame_time_ms(-30), -1000.0);
        assert_eq!(KEYFRAME_EVERY, 60);
    }
}
