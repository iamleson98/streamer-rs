//! The native engine's fonts: the board's faces and the fallback faces (`board-draw`'s
//! `BOARD_FONT_FILES` and `FALLBACK_FONT_FILES`), read once at start from one directory. A missing
//! file is refused at start, not found out in the middle of a render.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use board_draw::{BOARD_FONT_FILES, FALLBACK_FONT_FILES, FallbackFace, FontSet};

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("the font {0} is missing or unreadable")]
    Missing(&'static str),
    #[error("the fonts do not parse")]
    Parse,
}

/// The font files' bytes; `Send`, unlike the [`FontSet`] built from them, which each render makes on
/// its own thread.
#[derive(Debug)]
pub struct FontFiles {
    board: Vec<Vec<u8>>,
    fallback: Vec<Vec<u8>>,
}

impl FontFiles {
    pub fn load(dir: &Path) -> Result<Arc<Self>, FontError> {
        let read = |name: &'static str| {
            std::fs::read(dir.join(name)).map_err(|_| FontError::Missing(name))
        };
        let files = Self {
            board: BOARD_FONT_FILES
                .iter()
                .map(|n| read(n))
                .collect::<Result<_, _>>()?,
            fallback: FALLBACK_FONT_FILES
                .iter()
                .map(|f| read(f.name))
                .collect::<Result<_, _>>()?,
        };
        files.build()?;
        Ok(Arc::new(files))
    }

    /// The font set, on the calling thread.
    pub fn build(&self) -> Result<Rc<FontSet>, FontError> {
        let board: Vec<&[u8]> = self.board.iter().map(Vec::as_slice).collect();
        let fallback: Vec<FallbackFace> = self
            .fallback
            .iter()
            .zip(FALLBACK_FONT_FILES.iter())
            .map(|(bytes, f)| FallbackFace {
                bytes,
                index: f.index,
                emoji: f.emoji,
                han: f.han,
            })
            .collect();
        FontSet::with_fallbacks(&board, &fallback).map_err(|_| FontError::Parse)
    }
}
