//! Shared by the integration tests: the pinned board fonts from the lesson-replay submodule.

use std::rc::Rc;

use board_draw::{BOARD_FONT_FILES, FontSet, SkiaCanvas};

const FONTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../lesson-replay/static/fonts"
);

pub fn fonts() -> Rc<FontSet> {
    let files: Vec<Vec<u8>> = BOARD_FONT_FILES
        .iter()
        .map(|name| {
            std::fs::read(format!("{FONTS}/{name}")).unwrap_or_else(|e| {
                panic!("{name}: {e} (run `git submodule update --init lesson-replay`)")
            })
        })
        .collect();
    let faces: Vec<&[u8]> = files.iter().map(Vec::as_slice).collect();
    FontSet::new(&faces).expect("the pinned fonts parse")
}

pub fn canvas(w: u32, h: u32) -> SkiaCanvas {
    SkiaCanvas::new(w, h, fonts()).expect("a canvas")
}

/// The pixel at (x, y) as RGBA.
pub fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}
