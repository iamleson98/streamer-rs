//! A description of where the board's text and elements are, as numbers: what the parity check
//! compares with the web player's (a pixel can look right with text a pixel off).
//!
//! The web side writes the same JSON from the same functions (`parity/harness.ts`), so a field
//! added here is added there.

use replay_model::element::{LINE_HEIGHT, element_bounds};
use replay_model::{
    BoardElement, BoardReplay, ElementKind, MeasureText, OrderedElement, Viewport, board_viewport,
};
use serde_json::{Value, json};

use crate::canvas::Canvas;
use crate::maths::draw_pack;
use crate::render::BoardTheme;
use crate::skia::SkiaCanvas;

/// What the dump needs of the canvas the board is drawn with: its measuring, a scratch canvas to
/// draw pack elements on, the theme and the camera's zoom.
struct Probe<'a> {
    canvas: &'a SkiaCanvas,
    theme: BoardTheme,
    zoom: f64,
}

/// The boxes of the lines of text a graph, solid or symbol draws (axis numbers, labels, units...),
/// in board units: the pack element drawn once on a scratch canvas at the camera's scale, as the
/// web side does with a recording context.
fn pack_text(el: &BoardElement, probe: &Probe) -> Vec<Value> {
    let ElementKind::Pack(pack) = &el.kind else {
        return Vec::new();
    };
    let mut scratch = probe.canvas.offscreen(8, 8);
    scratch.log_text();
    scratch.set_transform(probe.zoom, 0.0, 0.0, probe.zoom, 0.0, 0.0);
    draw_pack(&mut scratch, el, pack, probe.theme);
    scratch
        .take_text_boxes()
        .into_iter()
        .map(|t| json!({"text": t.text, "bounds": t.bounds}))
        .collect()
}

fn describe(id: &str, el: &BoardElement, measure: &dyn MeasureText, probe: &Probe) -> Value {
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
    if matches!(el.kind, ElementKind::Pack(_)) {
        d["ink_text"] = pack_text(el, probe).into();
    }
    d
}

/// The board of `replay` as it would be drawn in a canvas of `width` x `height`: the camera, every
/// element's bounds in drawing order and, for text, its lines with their measured widths, for a
/// graph, solid or symbol the boxes of the text it draws (`ink_text`); then the overlays the same
/// way.
pub fn layout_dump(
    replay: &BoardReplay,
    canvas: &SkiaCanvas,
    theme: BoardTheme,
    width: f64,
    height: f64,
) -> Value {
    let v = board_viewport(replay.scene(), canvas.measurer().as_ref(), width, height);
    layout_dump_at(replay, canvas, theme, width, height, v)
}

/// [`layout_dump`] with the camera given.
pub fn layout_dump_at(
    replay: &BoardReplay,
    canvas: &SkiaCanvas,
    theme: BoardTheme,
    width: f64,
    height: f64,
    v: Viewport,
) -> Value {
    let elements = replay.scene().ordered();
    let measurer = canvas.measurer();
    let measure = measurer.as_ref();
    let probe = Probe {
        canvas,
        theme,
        zoom: v.zoom,
    };
    json!({
        "width": width,
        "height": height,
        "viewport": {"x": v.x, "y": v.y, "zoom": v.zoom},
        "elements": elements
            .iter()
            .map(|OrderedElement { id, el }| describe(id, el, measure, &probe))
            .collect::<Vec<_>>(),
        "overlays": replay
            .remote()
            .filter_map(|(u, p)| p.preview.as_ref().map(|el| describe(u, el, measure, &probe)))
            .collect::<Vec<_>>(),
    })
}

/// How many glyphs of the board's text, in elements and overlays, are a font's missing-glyph box:
/// `missing(line, size)` counts them in one line. Any count above 0 is text a viewer would see as
/// empty rectangles.
pub fn missing_glyphs(replay: &BoardReplay, missing: impl Fn(&str, f64) -> usize) -> usize {
    let in_element = |el: &BoardElement| match &el.kind {
        ElementKind::Text { text, .. } => text.split('\n').map(|line| missing(line, el.size)).sum(),
        _ => 0,
    };
    let elements: usize = replay
        .scene()
        .ordered()
        .iter()
        .map(|e| in_element(&e.el))
        .sum();
    let overlays: usize = replay
        .remote()
        .filter_map(|(_, p)| p.preview.as_ref())
        .map(in_element)
        .sum();
    elements + overlays
}
