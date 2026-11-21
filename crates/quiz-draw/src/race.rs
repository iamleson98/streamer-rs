//! The race's stage (placeholder while the screens are written).
use replay_model::quiz::QuizReplay;
use taffy::prelude::NodeId;

use crate::kit::Cx;
use crate::ui::{S, Visual};

pub fn body(c: &mut Cx, _replay: &QuizReplay, _now: f64, _w: f32, _h: f32) -> NodeId {
    c.b.div("div", S::block(), Visual::default(), &[])
}
