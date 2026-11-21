//! Shared by the integration tests: the pinned board fonts from the lesson-replay submodule.
#![allow(dead_code)] // each test file uses some of it

use std::rc::Rc;

use board_draw::{BOARD_FONT_FILES, FALLBACK_FONT_FILES, FallbackFace, FontSet, SkiaCanvas};

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

/// The board's fonts and the fallback fonts behind them, from `BOARD_FALLBACK_FONTS` or, fetched
/// and checked by `fallback-fonts.sh`, `target/fallback-fonts`.
pub fn fonts_with_fallbacks() -> Rc<FontSet> {
    let dir = std::env::var("BOARD_FALLBACK_FONTS").unwrap_or_else(|_| {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/fallback-fonts");
        let status =
            std::process::Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fallback-fonts.sh"))
                .arg(dir)
                .stdout(std::process::Stdio::null())
                .status()
                .expect("running fallback-fonts.sh");
        assert!(status.success(), "fallback-fonts.sh failed");
        dir.to_owned()
    });
    let board: Vec<Vec<u8>> = BOARD_FONT_FILES
        .iter()
        .map(|name| std::fs::read(format!("{FONTS}/{name}")).expect("a board font"))
        .collect();
    let board: Vec<&[u8]> = board.iter().map(Vec::as_slice).collect();
    let files: Vec<Vec<u8>> = FALLBACK_FONT_FILES
        .iter()
        .map(|f| {
            std::fs::read(format!("{dir}/{}", f.name)).unwrap_or_else(|e| panic!("{}: {e}", f.name))
        })
        .collect();
    let fallbacks: Vec<FallbackFace> = FALLBACK_FONT_FILES
        .iter()
        .zip(&files)
        .map(|(f, bytes)| FallbackFace {
            bytes,
            index: f.index,
            emoji: f.emoji,
            han: f.han,
        })
        .collect();
    FontSet::with_fallbacks(&board, &fallbacks).expect("the fonts parse")
}

pub fn canvas_with_fallbacks(w: u32, h: u32) -> SkiaCanvas {
    SkiaCanvas::new(w, h, fonts_with_fallbacks()).expect("a canvas")
}

pub fn canvas(w: u32, h: u32) -> SkiaCanvas {
    SkiaCanvas::new(w, h, fonts()).expect("a canvas")
}

/// The pixel at (x, y) as RGBA.
pub fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}
