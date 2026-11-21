//! Draws what surrounds a recorded lesson's stage in the video, a port of `lesson-replay`'s
//! `export/` (layout, brand strip, camera tile, screen fit, countdown card), in both languages.
//! The board is `board-draw`'s; the quiz and Quest screens plug in through [`quiz::QuizStageDraw`].

pub mod card;
pub mod clock;
pub mod i18n;
pub mod layout;
pub mod logo;
pub mod paint;
pub mod quiz;

pub use i18n::{Lang, Strings};
pub use layout::{Boxf, StageLayout, stage_layout};
pub use paint::{CardText, PaintError, StagePainter, blit_bgra};
pub use quiz::QuizStageDraw;
