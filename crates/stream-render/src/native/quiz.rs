//! The quiz and Quest screens of a recorded lesson, from `quiz-draw`, behind `stage-draw`'s seam.

use std::rc::Rc;

use board_draw::FontSet;
use quiz_draw::{Locale, QuizStage};
use replay_model::quiz::read_quiz_log;
use skia_safe::{Canvas, Rect};
use stage_draw::{Lang, QuizStageDraw, QuizStageFactory};

struct Stage(QuizStage);

impl QuizStageDraw for Stage {
    fn on(&mut self, t_ms: f64) -> bool {
        self.0.on(t_ms)
    }

    fn draw(&mut self, canvas: &Canvas, stage: Rect, t_ms: f64) {
        self.0.draw(canvas, stage, t_ms);
    }
}

/// A maker of the quiz stage from the board log (gzip), built on the thread that draws.
/// `None` when the log holds no game.
pub fn factory(gz: &[u8], lang: Lang) -> Option<QuizStageFactory> {
    let records = read_quiz_log(gz).ok()?;
    if records.is_empty() {
        return None;
    }
    let locale = match lang {
        Lang::Vi => Locale::Vi,
        Lang::En => Locale::En,
    };
    Some(std::sync::Arc::new(move |fonts: Rc<FontSet>| {
        Box::new(Stage(QuizStage::new(records.clone(), locale, fonts))) as Box<dyn QuizStageDraw>
    }))
}
