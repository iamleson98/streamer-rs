//! Where things sit in the rendered frame: a strip under the stage always carries the brand at its
//! left; when the teacher's camera is on at any point the strip is taller and holds the camera
//! tile, centred. Nothing is ever drawn over the stage. Port of `export/layout.ts`.

use crate::clock::{HEIGHT, WIDTH};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Boxf {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Height of the strip under the stage: with the camera tile, and without it.
pub const STRIP_H: f64 = 96.0;
pub const SLIM_STRIP_H: f64 = 56.0;
/// The camera tile (16:9) inside the strip.
pub const TILE_W: f64 = 144.0;
pub const TILE_H: f64 = 80.0;
pub const BRAND_X: f64 = 24.0;
pub const BRAND_W: f64 = 300.0;
const MARK_H: f64 = 36.0;
const SLIM_MARK_H: f64 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageLayout {
    pub stage: Boxf,
    pub strip: Boxf,
    pub brand: Boxf,
    pub tile: Option<Boxf>,
}

pub fn stage_layout(has_camera: bool) -> StageLayout {
    let (w, h) = (f64::from(WIDTH), f64::from(HEIGHT));
    let strip_h = if has_camera { STRIP_H } else { SLIM_STRIP_H };
    let mark_h = if has_camera { MARK_H } else { SLIM_MARK_H };
    let stage_h = h - strip_h;
    StageLayout {
        stage: Boxf {
            x: 0.0,
            y: 0.0,
            w,
            h: stage_h,
        },
        strip: Boxf {
            x: 0.0,
            y: stage_h,
            w,
            h: strip_h,
        },
        brand: Boxf {
            x: BRAND_X,
            y: stage_h + (strip_h - mark_h) / 2.0,
            w: BRAND_W,
            h: mark_h,
        },
        tile: has_camera.then(|| Boxf {
            x: (w - TILE_W) / 2.0,
            y: stage_h + (STRIP_H - TILE_H) / 2.0,
            w: TILE_W,
            h: TILE_H,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_and_strip_fill_the_frame() {
        let slim = stage_layout(false);
        assert_eq!((slim.stage.h, slim.strip.h), (664.0, 56.0));
        assert!(slim.tile.is_none());
        let full = stage_layout(true);
        assert_eq!((full.stage.h, full.strip.y), (624.0, 624.0));
        let tile = full.tile.unwrap();
        assert_eq!(
            (tile.x, tile.y, tile.w, tile.h),
            (568.0, 632.0, 144.0, 80.0)
        );
        assert_eq!(full.brand.y, 624.0 + (96.0 - 36.0) / 2.0);
    }
}
