//! A description of where the board's text and elements are, as numbers: what the parity check
//! compares with the web player's (a pixel can look right with text a pixel off).
//!
//! The web side writes the same JSON from the same functions (`parity/harness.ts`), so a field
//! added here is added there.

use replay_model::element::{LINE_HEIGHT, element_bounds};
use replay_model::{
    BoardElement, BoardReplay, ElementKind, MeasureText, OrderedElement, board_viewport,
};
use serde_json::{Value, json};

fn describe(id: &str, el: &BoardElement, measure: &dyn MeasureText) -> Value {
    let b = element_bounds(el, measure);
    let mut d =
        json!({"id": id, "type": el.type_name(), "bounds": [b.min_x, b.min_y, b.max_x, b.max_y]});
    if let ElementKind::Text { text, .. } = &el.kind {
        d["lines"] = text
            .split('\n')
            .map(|line| json!({"text": line, "width": measure.width(line, el.size)}))
            .collect();
        d["line_height"] = json!(el.size * LINE_HEIGHT);
    }
    d
}

/// The board of `replay` as it would be drawn in a canvas of `width` x `height`: the camera, every
/// element's bounds in drawing order and, for text, its lines with their measured widths; then the
/// overlays the same way.
pub fn layout_dump(
    replay: &BoardReplay,
    measure: &dyn MeasureText,
    width: f64,
    height: f64,
) -> Value {
    let elements = replay.scene().ordered();
    let v = board_viewport(replay.scene(), measure, width, height);
    json!({
        "width": width,
        "height": height,
        "viewport": {"x": v.x, "y": v.y, "zoom": v.zoom},
        "elements": elements
            .iter()
            .map(|OrderedElement { id, el }| describe(id, el, measure))
            .collect::<Vec<_>>(),
        "overlays": replay
            .remote()
            .filter_map(|(u, p)| p.preview.as_ref().map(|el| describe(u, el, measure)))
            .collect::<Vec<_>>(),
    })
}
