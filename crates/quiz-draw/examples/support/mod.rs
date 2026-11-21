//! Shared by the dev tools: reading `--name value` options and the pinned fonts. Each tool uses
//! some of it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::rc::Rc;

use board_draw::{BOARD_FONT_FILES, FALLBACK_FONT_FILES, FallbackFace, FontSet};

pub fn opt(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

pub fn required(name: &str) -> String {
    opt(name).unwrap_or_else(|| panic!("missing {name}"))
}

/// The board's fonts in `--fonts DIR` (default: the lesson-replay submodule's), in stack order.
pub fn fonts() -> Rc<FontSet> {
    let dir = PathBuf::from(opt("--fonts").unwrap_or_else(|| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../lesson-replay/static/fonts"
        )
        .to_owned()
    }));
    let files: Vec<Vec<u8>> = BOARD_FONT_FILES
        .iter()
        .map(|name| std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}")))
        .collect();
    let faces: Vec<&[u8]> = files.iter().map(Vec::as_slice).collect();
    FontSet::new(&faces).expect("the fonts parse")
}

/// The fallback fonts' directory: `--fallback-fonts DIR`, else `BOARD_FALLBACK_FONTS`, else
/// `target/fallback-fonts` of the workspace, fetched and checked by `fallback-fonts.sh`.
pub fn fallback_dir() -> PathBuf {
    if let Some(dir) =
        opt("--fallback-fonts").or_else(|| std::env::var("BOARD_FALLBACK_FONTS").ok())
    {
        return PathBuf::from(dir);
    }
    let dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/fallback-fonts"
    ));
    let status =
        std::process::Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../board-draw/fallback-fonts.sh"))
            .arg(&dir)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("running fallback-fonts.sh");
    assert!(status.success(), "fallback-fonts.sh failed");
    dir
}

/// [`fonts`] and the fallback fonts behind them.
pub fn fonts_with_fallbacks() -> Rc<FontSet> {
    let dir = PathBuf::from(opt("--fonts").unwrap_or_else(|| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../lesson-replay/static/fonts"
        )
        .to_owned()
    }));
    let board: Vec<Vec<u8>> = BOARD_FONT_FILES
        .iter()
        .map(|name| std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}")))
        .collect();
    let board: Vec<&[u8]> = board.iter().map(Vec::as_slice).collect();
    let fallback_dir = fallback_dir();
    let files: Vec<Vec<u8>> = FALLBACK_FONT_FILES
        .iter()
        .map(|f| {
            std::fs::read(fallback_dir.join(f.name)).unwrap_or_else(|e| panic!("{}: {e}", f.name))
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
