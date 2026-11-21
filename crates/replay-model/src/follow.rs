//! Where a recorded board is looked at: the whole drawing kept in frame as it grows or, on a board
//! with sheets, the sheet the teacher was on. Port of `player/board-viewport.ts`; the player's
//! board and the rendered video frame it the same way.

use crate::element::MeasureText;
use crate::pages::{sheets_of, viewed_sheet};
use crate::scene::BoardScene;
use crate::viewport::{DEFAULT_MARGIN, Viewport, fit_to};

pub fn board_viewport(
    scene: &BoardScene,
    measure: &dyn MeasureText,
    width: f64,
    height: f64,
) -> Viewport {
    if width == 0.0 || height == 0.0 {
        return Viewport::IDENTITY;
    }
    let sheets = sheets_of(&scene.ordered());
    let content = match viewed_sheet(&sheets, scene.view()) {
        Some(sheet) => Some(sheet.page.rect()),
        None => scene.content_bounds(measure),
    };
    content.map_or(Viewport::IDENTITY, |c| {
        fit_to(c, width, height, DEFAULT_MARGIN)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::tests::{env, line};
    use serde_json::json;

    const M: fn(&str, f64) -> f64 = |_, _| 0.0;

    #[test]
    fn an_empty_board_and_an_empty_canvas_show_the_origin() {
        let s = BoardScene::new();
        assert_eq!(board_viewport(&s, &M, 1280.0, 624.0), Viewport::IDENTITY);
        let mut s = BoardScene::new();
        s.merge([env("a", 1, 1, false, line(1.0, 10.0))]);
        assert_eq!(board_viewport(&s, &M, 0.0, 624.0), Viewport::IDENTITY);
    }

    #[test]
    fn without_sheets_the_ink_is_kept_in_frame() {
        let mut s = BoardScene::new();
        s.merge([env("a", 1, 1, false, line(1.0, 3000.0))]);
        let v = board_viewport(&s, &M, 1280.0, 624.0);
        assert!(v.zoom < 1.0 && v.zoom > 0.2);
    }

    #[test]
    fn with_sheets_the_sheet_the_view_names_fills_the_frame() {
        let page = |x: f64| json!({"type":"page","color":"ink","size":2,"z":0,"x":x,"y":0,"w":1280,"h":720});
        let mut s = BoardScene::new();
        s.merge([
            env("s1", 1, 1, false, page(0.0)),
            env("s2", 1, 1, false, page(1600.0)),
            env(
                "view",
                1,
                1,
                false,
                json!({"type":"view","color":"ink","size":1,"z":0,"page":"s2"}),
            ),
            env("ink", 1, 1, false, line(1.0, 5000.0)),
        ]);
        let v = board_viewport(&s, &M, 1280.0, 720.0);
        // The second sheet (x 1600..2880) is centred: the frame's centre is the sheet's centre.
        assert!((v.x + 1280.0 / 2.0 / v.zoom - 2240.0).abs() < 1e-9);
        assert!((v.zoom - (720.0 - 96.0) / 720.0).abs() < 1e-12);
    }
}
