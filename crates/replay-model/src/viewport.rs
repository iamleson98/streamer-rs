//! The camera over the board: `x, y` is the board point at the canvas's top-left corner, `zoom`
//! the screen pixels per board unit. Port of `board/viewport.ts`.

use crate::element::Rect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

impl Viewport {
    /// The camera at the origin, at scale 1.
    pub const IDENTITY: Viewport = Viewport {
        x: 0.0,
        y: 0.0,
        zoom: 1.0,
    };
}

const MIN_ZOOM: f64 = 0.2;

/// The board area visible in a canvas of this size.
pub fn visible_rect(v: Viewport, width: f64, height: f64) -> Rect {
    Rect {
        min_x: v.x,
        min_y: v.y,
        max_x: v.x + width / v.zoom,
        max_y: v.y + height / v.zoom,
    }
}

/// A viewport showing `content` centred in the canvas (zoom capped at 1), with `margin` pixels to
/// spare around it. (The player's insets, for a toolbar floating over the board, never apply to a
/// recording.)
pub fn fit_to(content: Rect, width: f64, height: f64, margin: f64) -> Viewport {
    // `(w || 1)`: an empty or undefined extent counts as one unit.
    let nonzero = |v: f64| if v == 0.0 || v.is_nan() { 1.0 } else { v };
    let w = content.max_x - content.min_x;
    let h = content.max_y - content.min_y;
    let zoom = ((width - margin * 2.0) / nonzero(w))
        .min((height - margin * 2.0) / nonzero(h))
        .clamp(MIN_ZOOM, 1.0);
    Viewport {
        zoom,
        x: (content.min_x + content.max_x) / 2.0 - width / 2.0 / zoom,
        y: (content.min_y + content.max_y) / 2.0 - height / 2.0 / zoom,
    }
}

/// The margin the player keeps around what it frames.
pub const DEFAULT_MARGIN: f64 = 48.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Rect {
        Rect {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    #[test]
    fn small_content_is_centred_at_zoom_one() {
        let v = fit_to(rect(0.0, 0.0, 100.0, 100.0), 1000.0, 600.0, DEFAULT_MARGIN);
        assert_eq!(v.zoom, 1.0);
        assert_eq!((v.x, v.y), (50.0 - 500.0, 50.0 - 300.0));
    }

    #[test]
    fn large_content_zooms_out_to_fit_inside_the_margin() {
        let v = fit_to(rect(0.0, 0.0, 2000.0, 400.0), 1000.0, 600.0, DEFAULT_MARGIN);
        assert!((v.zoom - 904.0 / 2000.0).abs() < 1e-12);
        let seen = visible_rect(v, 1000.0, 600.0);
        assert!((seen.min_x + seen.max_x) / 2.0 - 1000.0 < 1e-9);
    }

    #[test]
    fn zoom_never_goes_below_the_minimum_and_an_empty_box_counts_as_one_unit() {
        assert_eq!(
            fit_to(rect(0.0, 0.0, 1e6, 1e6), 1000.0, 600.0, 48.0).zoom,
            0.2
        );
        let dot = fit_to(rect(5.0, 5.0, 5.0, 5.0), 1000.0, 600.0, 48.0);
        assert_eq!(dot.zoom, 1.0);
    }
}
