//! What a recording shows at a moment, and which presentation is on the stage. Port of
//! `player/timeline.ts` and `export/stage.ts` in `lesson-replay`.

use crate::manifest::{FileKind, RecordingFile};

fn covers(f: &RecordingFile, t: f64) -> bool {
    f.start_ms <= t && t < f.end_ms
}

/// The teacher's own streams: a screen, its sound, a camera, a microphone.
pub fn teacher_files(files: &[RecordingFile], teacher_id: &str) -> Vec<RecordingFile> {
    if teacher_id.is_empty() {
        return Vec::new();
    }
    files
        .iter()
        .filter(|f| {
            f.user_id.as_deref() == Some(teacher_id)
                && matches!(
                    f.kind,
                    FileKind::Screen | FileKind::ScreenAudio | FileKind::Camera | FileKind::Voice
                )
        })
        .cloned()
        .collect()
}

/// The screens playing at `t` (or starting within `lead` ms), oldest first.
pub fn screen_layers_at(files: &[RecordingFile], t: f64, lead: f64) -> Vec<RecordingFile> {
    let mut layers: Vec<RecordingFile> = files
        .iter()
        .filter(|f| f.kind == FileKind::Screen && f.start_ms - lead <= t && t < f.end_ms)
        .cloned()
        .collect();
    layers.sort_by(|a, b| {
        a.start_ms
            .total_cmp(&b.start_ms)
            .then(a.index.cmp(&b.index))
    });
    layers
}

/// Which of the loaded screens to show at `t`: the latest started one that covers `t` and has a
/// picture, else `held` (the one shown last) if it has one, else none.
pub fn shown_screen(
    layers: &[RecordingFile],
    t: f64,
    ready: impl Fn(u32) -> bool,
    held: Option<u32>,
) -> Option<u32> {
    let mut shown = None;
    for f in layers {
        if covers(f, t) && ready(f.index) {
            shown = Some(f.index);
        }
    }
    shown.or_else(|| held.filter(|h| layers.iter().any(|f| f.index == *h) && ready(*h)))
}

/// Whether the teacher's camera is on at any point of the recording.
pub fn has_camera(teacher: &[RecordingFile]) -> bool {
    teacher.iter().any(|f| f.kind == FileKind::Camera)
}

/// The teacher's camera file while the camera is on at `t`: the latest to have begun, as
/// `participantsAt` leaves it.
pub fn camera_at(teacher: &[RecordingFile], t: f64) -> Option<&RecordingFile> {
    // `participantsAt` keeps, per session, the last camera file in manifest order.
    let mut cameras: Vec<&RecordingFile> = Vec::new();
    for f in teacher {
        if f.kind != FileKind::Camera
            || !covers(f, t)
            || f.session_id.as_deref().is_none_or(str::is_empty)
        {
            continue;
        }
        cameras.retain(|c| c.session_id != f.session_id);
        cameras.push(f);
    }
    // Teachers rank first, then earlier appearance; every file here is the teacher's.
    cameras
        .into_iter()
        .min_by(|a, b| a.start_ms.total_cmp(&b.start_ms))
}

/// The teacher's sound files (microphone, screen sound) that overlap [from_ms, to_ms).
pub fn mics_between(teacher: &[RecordingFile], from_ms: f64, to_ms: f64) -> Vec<&RecordingFile> {
    teacher
        .iter()
        .filter(|f| {
            matches!(f.kind, FileKind::Voice | FileKind::ScreenAudio)
                && f.start_ms < to_ms
                && f.end_ms > from_ms
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    Screen,
    Quiz,
    Board,
}

/// Which presentation is on the stage: the latest one that began takes it (a shared screen, a quiz
/// game), and the board is what is left.
pub fn choose_stage(screen_start: Option<f64>, quiz_on: bool, quiz_since: f64) -> StageKind {
    if let Some(start) = screen_start
        && (!quiz_on || start >= quiz_since)
    {
        return StageKind::Screen;
    }
    if quiz_on {
        StageKind::Quiz
    } else {
        StageKind::Board
    }
}

/// Where a `width` x `height` picture sits inside a box, whole and centred.
pub fn fit_rect(width: f64, height: f64, box_w: f64, box_h: f64) -> (f64, f64, f64, f64) {
    if width <= 0.0 || height <= 0.0 {
        return (0.0, 0.0, box_w, box_h);
    }
    let scale = (box_w / width).min(box_h / height);
    let (w, h) = (width * scale, height * scale);
    ((box_w - w) / 2.0, (box_h - h) / 2.0, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ManifestFile;

    fn file(
        index: u32,
        kind: FileKind,
        user: &str,
        session: &str,
        a: f64,
        b: f64,
    ) -> RecordingFile {
        RecordingFile {
            index,
            file: ManifestFile {
                kind,
                session_id: Some(session.into()),
                user_id: Some(user.into()),
                start_ms: a,
                end_ms: b,
                width: None,
                height: None,
                size: 1,
            },
        }
    }

    #[test]
    fn only_the_teachers_streams_count() {
        let files = vec![
            file(0, FileKind::Camera, "t", "s1", 0.0, 9.0),
            file(1, FileKind::Camera, "u", "s2", 0.0, 9.0),
            file(2, FileKind::Board, "t", "s1", 0.0, 9.0),
            file(3, FileKind::ScreenAudio, "t", "s1", 0.0, 9.0),
        ];
        let t = teacher_files(&files, "t");
        assert_eq!(t.iter().map(|f| f.index).collect::<Vec<_>>(), [0, 3]);
        assert!(teacher_files(&files, "").is_empty());
    }

    #[test]
    fn screens_are_layered_oldest_first_and_the_latest_ready_one_is_shown() {
        let files = vec![
            file(0, FileKind::Screen, "t", "s", 0.0, 100.0),
            file(1, FileKind::Screen, "t", "s", 50.0, 100.0),
        ];
        let layers = screen_layers_at(&files, 60.0, 0.0);
        assert_eq!(layers.len(), 2);
        assert_eq!(shown_screen(&layers, 60.0, |_| true, None), Some(1));
        assert_eq!(shown_screen(&layers, 60.0, |i| i == 0, None), Some(0));
        // A screen still ahead is loaded early but not shown.
        assert_eq!(screen_layers_at(&files, 40.0, 20.0).len(), 2);
        assert_eq!(
            shown_screen(&screen_layers_at(&files, 40.0, 20.0), 40.0, |_| true, None),
            Some(0)
        );
        // The held one stays while nothing covering has a picture.
        assert_eq!(shown_screen(&layers, 60.0, |i| i == 0, Some(0)), Some(0));
        assert_eq!(shown_screen(&layers, 60.0, |_| false, Some(0)), None);
    }

    #[test]
    fn the_latest_presentation_takes_the_stage() {
        assert_eq!(choose_stage(None, false, 0.0), StageKind::Board);
        assert_eq!(choose_stage(None, true, 0.0), StageKind::Quiz);
        assert_eq!(choose_stage(Some(5.0), false, 0.0), StageKind::Screen);
        assert_eq!(choose_stage(Some(5.0), true, 9.0), StageKind::Quiz);
        assert_eq!(choose_stage(Some(9.0), true, 9.0), StageKind::Screen);
    }

    #[test]
    fn the_camera_is_on_while_a_file_covers_the_time() {
        let t = vec![file(0, FileKind::Camera, "t", "s", 10.0, 20.0)];
        assert!(has_camera(&t));
        assert!(camera_at(&t, 5.0).is_none());
        assert_eq!(camera_at(&t, 10.0).map(|f| f.index), Some(0));
        assert!(camera_at(&t, 20.0).is_none());
    }

    #[test]
    fn sound_files_overlapping_a_span_are_listed() {
        let t = vec![
            file(0, FileKind::Voice, "t", "s", 0.0, 1000.0),
            file(1, FileKind::Voice, "t", "s", 1000.0, 2000.0),
            file(2, FileKind::Camera, "t", "s", 0.0, 2000.0),
        ];
        let got: Vec<u32> = mics_between(&t, 500.0, 1500.0)
            .iter()
            .map(|f| f.index)
            .collect();
        assert_eq!(got, [0, 1]);
        assert_eq!(mics_between(&t, 1000.0, 1000.0).len(), 0);
    }

    #[test]
    fn a_picture_fits_whole_and_centred() {
        let (x, y, w, h) = fit_rect(1920.0, 1080.0, 1280.0, 624.0);
        assert!((w - 1109.333).abs() < 0.001 && (h - 624.0).abs() < 1e-6);
        assert!((x - (1280.0 - w) / 2.0).abs() < 1e-9 && y.abs() < 1e-6);
        assert_eq!(fit_rect(0.0, 0.0, 10.0, 5.0), (0.0, 0.0, 10.0, 5.0));
    }
}
