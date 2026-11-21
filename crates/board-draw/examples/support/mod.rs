//! Shared by the dev tools: reading `--name value` options and the pinned fonts. Each tool uses
//! some of it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::rc::Rc;

use board_draw::{BOARD_FONT_FILES, FontSet};

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
