//! Draws a recorded lesson's whiteboard. The board code is a port of the web player's
//! (`lesson-replay`, `board/render.ts`, `board/sheet.ts`, `export/painters.ts`) written against a
//! small [`Canvas`] trait that mirrors the Canvas 2D calls it makes; [`skia::SkiaCanvas`] is the
//! one backend. The scene, replay clock and camera come from `replay-model`.
//!
//! Parity with the web player is checked by `parity/` (see this crate's README).

pub mod canvas;
pub mod layout;
pub mod painter;
pub mod pictures;
pub mod render;
pub mod skia;

pub use canvas::{Canvas, LineCap, LineJoin, Path, Rgba};
pub use layout::{layout_dump, layout_dump_at};
pub use painter::BoardPainter;
pub use pictures::{PictureState, Pictures, StaticPictures};
pub use render::{BoardRenderer, BoardTheme, DrawOptions};
pub use skia::{BOARD_FONT_FILES, FontSet, SkiaCanvas, decode_image, decode_rgba, encode_png};
