//! The timeline of a video, read from its pictures: for each second, whether the stage shows the
//! countdown card, the board or a screen, and whether the camera tile is lit. The same reading is
//! applied to every video, so the engines are judged by what they drew, not by what they say they
//! did. It is meant for the synthetic recordings: a white board, a colourful `testsrc2` screen, a
//! dark slate card, a camera tile that differs from the strip's colour.

use replay_model::manifest::RecordingFile;
use replay_model::timeline::{camera_at, screen_layers_at};

const W: usize = 320;
const H: usize = 180;
/// Rows of the 180-row picture that are always stage (the stage is 156 or 166 of them).
const STAGE_ROWS: usize = 150;
/// Inside the camera tile in the 320 x 180 picture, and inside the strip of either layout (the
/// slim strip starts at row 166, the tall one at 156).
const TILE: (usize, usize, usize, usize) = (146, 168, 28, 8);
/// The strip's colour (#0f172a) as a brightness.
const STRIP_LUMA: f64 = 22.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Card,
    Board,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Second {
    pub stage: Stage,
    pub camera: bool,
}

fn luma(p: &[u8]) -> f64 {
    0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
}

/// Reads one 320 x 180 RGB picture.
pub fn read_second(rgb: &[u8]) -> Second {
    let stage: Vec<f64> = (0..STAGE_ROWS * W)
        .map(|i| luma(&rgb[i * 3..i * 3 + 3]))
        .collect();
    let mean = stage.iter().sum::<f64>() / stage.len() as f64;
    let kind = if mean > 190.0 {
        Stage::Board
    } else if mean < 70.0 {
        Stage::Card
    } else {
        Stage::Screen
    };
    let (x0, y0, w, h) = TILE;
    let mut sum = 0.0;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            sum += luma(&rgb[(y * W + x) * 3..(y * W + x) * 3 + 3]);
        }
    }
    let _ = H;
    let camera = (sum / (w * h) as f64 - STRIP_LUMA).abs() > 10.0;
    Second {
        stage: kind,
        camera,
    }
}

/// What the manifest says each second of the video should show: the card for the preroll, then
/// whichever of a screen and the board is on the stage at the middle of the second.
pub fn expected(teacher: &[RecordingFile], preroll_ms: u64, seconds: usize) -> Vec<Second> {
    (0..seconds)
        .map(|s| {
            let t = s as f64 * 1000.0 + 500.0 - preroll_ms as f64;
            if t < 0.0 {
                return Second {
                    stage: Stage::Card,
                    camera: false,
                };
            }
            let screen = !screen_layers_at(teacher, t, 0.0).is_empty();
            Second {
                stage: if screen { Stage::Screen } else { Stage::Board },
                camera: camera_at(teacher, t).is_some(),
            }
        })
        .collect()
}

/// How many seconds differ, in stage or in camera.
pub fn mismatches(observed: &[Second], expected: &[Second]) -> Vec<usize> {
    (0..observed.len().min(expected.len()))
        .filter(|&i| observed[i] != expected[i])
        .collect()
}

/// Runs of the same stage: (first second, last second, stage).
pub fn stage_runs(seconds: &[Second]) -> Vec<(usize, usize, Stage)> {
    let mut runs: Vec<(usize, usize, Stage)> = Vec::new();
    for (i, s) in seconds.iter().enumerate() {
        match runs.last_mut() {
            Some(last) if last.2 == s.stage => last.1 = i,
            _ => runs.push((i, i, s.stage)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(stage: u8, tile: u8) -> Vec<u8> {
        let mut rgb = vec![22u8; W * H * 3];
        for px in rgb[..STAGE_ROWS * W * 3].iter_mut() {
            *px = stage;
        }
        let (x0, y0, w, h) = TILE;
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                rgb[(y * W + x) * 3..(y * W + x) * 3 + 3].fill(tile);
            }
        }
        rgb
    }

    #[test]
    fn a_white_stage_is_the_board_a_dark_one_the_card_and_a_lit_tile_is_the_camera() {
        assert_eq!(
            read_second(&picture(255, 22)),
            Second {
                stage: Stage::Board,
                camera: false
            }
        );
        assert_eq!(
            read_second(&picture(30, 22)),
            Second {
                stage: Stage::Card,
                camera: false
            }
        );
        assert_eq!(
            read_second(&picture(120, 150)),
            Second {
                stage: Stage::Screen,
                camera: true
            }
        );
    }

    #[test]
    fn the_runs_group_equal_stages() {
        let s = |stage| Second {
            stage,
            camera: false,
        };
        let runs = stage_runs(&[
            s(Stage::Card),
            s(Stage::Card),
            s(Stage::Board),
            s(Stage::Screen),
        ]);
        assert_eq!(
            runs,
            [
                (0, 1, Stage::Card),
                (2, 2, Stage::Board),
                (3, 3, Stage::Screen)
            ]
        );
    }

    #[test]
    fn mismatches_name_the_seconds_that_differ() {
        let a = Second {
            stage: Stage::Board,
            camera: false,
        };
        let b = Second {
            stage: Stage::Screen,
            camera: false,
        };
        assert_eq!(mismatches(&[a, b, a], &[a, a, a]), [1]);
    }
}
