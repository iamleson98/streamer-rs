//! The seam for the quiz and Quest stage. `stage-draw` knows nothing of a game: the native engine
//! holds one object behind this trait (made by `quiz-draw`, the quiz screens' crate) and asks it
//! what the frame loop needs. A render with no game holds none.
//!
//! The contract, in the order the frame loop uses it for frame time `t` (ms into the lesson,
//! moving only forwards):
//!
//! 1. `on(t)`: is a game on the stage at `t` (it began and has not ended)? The loop remembers the
//!    time `on` last turned true (`quizSince` of `export/stage.ts`) and passes it to
//!    [`choose_stage`](replay_model::timeline::choose_stage).
//! 2. When the stage is the quiz, `draw(canvas, stage, t)` paints the whole stage box (its
//!    background included) at `t`. Seeking forwards is cheap; the object keeps its own state.
//!
//! The trait is `Send` so the compositor can be built on one thread and moved to its own.

use skia_safe::{Canvas, Rect};

pub trait QuizStageDraw: Send {
    /// A game is on the stage at `t_ms`.
    fn on(&self, t_ms: f64) -> bool;

    /// Paints the stage box `stage` (position and size in pixels) as it is at `t_ms`.
    fn draw(&mut self, canvas: &Canvas, stage: Rect, t_ms: f64);
}
