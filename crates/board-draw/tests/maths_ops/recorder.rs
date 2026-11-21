//! A canvas that writes down what it is asked to draw, in the same terms as the web-side mock that
//! made `ops.json`: every call that paints, with the state it paints in, and paths as the points
//! of their subpaths (count, sums, first and last).

use std::rc::Rc;

use board_draw::canvas::{PathOp, TextAlign, TextBaseline};
use board_draw::{Canvas, LineCap, LineJoin, Path, Rgba};
use replay_model::MeasureText;
use serde_json::{Value, json};

#[derive(Clone)]
struct State {
    fill: Rgba,
    stroke: Rgba,
    line_width: f64,
    cap: LineCap,
    join: LineJoin,
    dash: Vec<f64>,
    alpha: f64,
    font: (bool, u16, f64),
    align: TextAlign,
    baseline: TextBaseline,
}

pub struct Recorder {
    pub events: Vec<Value>,
    state: State,
    saved: Vec<State>,
    scale: f64,
}

impl Recorder {
    pub fn new(scale: f64) -> Self {
        let black = Rgba::rgb(0, 0, 0);
        Self {
            events: Vec::new(),
            state: State {
                fill: black,
                stroke: black,
                line_width: 1.0,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
                dash: Vec::new(),
                alpha: 1.0,
                font: (false, 400, 10.0),
                align: TextAlign::Left,
                baseline: TextBaseline::Top,
            },
            saved: Vec::new(),
            scale,
        }
    }

    fn colour(c: Rgba) -> Value {
        json!([c.r, c.g, c.b, (f64::from(c.a) * 1e6).round() / 1e6])
    }

    /// The state a stroke is made in.
    fn line(&self) -> Value {
        let s = &self.state;
        json!({
            "stroke": Self::colour(s.stroke), "lw": s.line_width, "dash": s.dash, "alpha": s.alpha,
            "join": match s.join { LineJoin::Round => "round", LineJoin::Miter => "miter", LineJoin::Bevel => "bevel" },
            "cap": match s.cap { LineCap::Round => "round", LineCap::Butt => "butt", LineCap::Square => "square" },
        })
    }

    fn text(&self) -> Value {
        let s = &self.state;
        json!({
            "font": {"italic": s.font.0, "weight": s.font.1, "size": s.font.2},
            "align": match s.align { TextAlign::Left => "left", TextAlign::Center => "center", TextAlign::Right => "right" },
            "baseline": match s.baseline { TextBaseline::Top => "top", TextBaseline::Middle => "middle", TextBaseline::Bottom => "bottom" },
        })
    }

    fn push(&mut self, mut event: Value, extra: &[Value]) {
        for e in extra {
            event
                .as_object_mut()
                .unwrap()
                .extend(e.as_object().unwrap().clone());
        }
        self.events.push(event);
    }

    fn path(path: &Path) -> Value {
        let (mut subs, mut arcs, mut rects, mut closed): (
            Vec<Vec<f64>>,
            Vec<Value>,
            Vec<Value>,
            u32,
        ) = (Vec::new(), Vec::new(), Vec::new(), 0);
        for op in path.ops() {
            match *op {
                PathOp::MoveTo(x, y) => subs.push(vec![x, y]),
                PathOp::LineTo(x, y) => subs.last_mut().expect("a subpath").extend([x, y]),
                PathOp::Ellipse { cx, cy, rx, .. } => arcs.push(json!([cx, cy, rx])),
                PathOp::Rect { x, y, w, h } => rects.push(json!([x, y, w, h])),
                PathOp::ClosePath => closed += 1,
                other => panic!("the maths drawings do not use {other:?}"),
            }
        }
        let subs: Vec<Value> = subs
            .iter()
            .map(|p| {
                let (sx, sy) = p
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .fold((0.0, 0.0), |(a, b), q| (a + q[0], b + q[1]));
                let n = p.len();
                json!([n / 2, sx, sy, p[0], p[1], p[n - 2], p[n - 1]])
            })
            .collect();
        json!({"subs": subs, "arcs": arcs, "rects": rects, "closed": closed})
    }
}

struct NoMeasure;

impl MeasureText for NoMeasure {
    fn width(&self, _: &str, _: f64) -> f64 {
        unreachable!("the maths drawings never measure text")
    }
}

impl Canvas for Recorder {
    type Image = ();

    fn size(&self) -> (u32, u32) {
        unreachable!()
    }
    fn offscreen(&self, _: u32, _: u32) -> Self {
        unreachable!()
    }
    fn image(&mut self) {}
    fn image_size(_: &()) -> (f64, f64) {
        unreachable!()
    }
    fn measurer(&self) -> Rc<dyn MeasureText> {
        Rc::new(NoMeasure)
    }

    fn save(&mut self) {
        self.saved.push(self.state.clone());
        self.events.push(json!({"op": "save"}));
    }
    fn restore(&mut self) {
        self.state = self.saved.pop().expect("a saved state");
        self.events.push(json!({"op": "restore"}));
    }
    fn set_transform(&mut self, _: f64, _: f64, _: f64, _: f64, _: f64, _: f64) {
        unreachable!()
    }
    fn clip(&mut self, path: &Path) {
        self.events
            .push(json!({"op": "clip", "path": Self::path(path)}));
    }

    fn set_fill_style(&mut self, color: Rgba) {
        self.state.fill = color;
    }
    fn set_stroke_style(&mut self, color: Rgba) {
        self.state.stroke = color;
    }
    fn set_line_width(&mut self, width: f64) {
        self.state.line_width = width;
    }
    fn set_line_cap(&mut self, cap: LineCap) {
        self.state.cap = cap;
    }
    fn set_line_join(&mut self, join: LineJoin) {
        self.state.join = join;
    }
    fn set_line_dash(&mut self, dash: &[f64]) {
        self.state.dash = dash.to_vec();
    }
    fn set_global_alpha(&mut self, alpha: f64) {
        self.state.alpha = alpha;
    }
    fn set_font(&mut self, italic: bool, weight: u16, size: f64) {
        self.state.font = (italic, weight, size);
    }
    fn set_text_align(&mut self, align: TextAlign) {
        self.state.align = align;
    }
    fn set_text_baseline(&mut self, baseline: TextBaseline) {
        self.state.baseline = baseline;
    }
    fn scale(&mut self) -> f64 {
        self.scale
    }

    fn fill(&mut self, path: &Path) {
        let colour = Self::colour(self.state.fill);
        self.push(
            json!({"op": "fill", "path": Self::path(path)}),
            &[json!({"fill": colour, "alpha": self.state.alpha})],
        );
    }
    fn stroke(&mut self, path: &Path) {
        let line = self.line();
        self.push(json!({"op": "stroke", "path": Self::path(path)}), &[line]);
    }
    fn fill_rect(&mut self, _: f64, _: f64, _: f64, _: f64) {
        unreachable!()
    }
    fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let line = self.line();
        self.push(json!({"op": "strokeRect", "rect": [x, y, w, h]}), &[line]);
    }
    fn clear_rect(&mut self, _: f64, _: f64, _: f64, _: f64) {
        unreachable!()
    }
    fn draw_image(&mut self, _: &(), _: f64, _: f64, _: f64, _: f64) {
        unreachable!()
    }
    fn fill_text(&mut self, text: &str, x: f64, y: f64) {
        let (colour, text_state) = (Self::colour(self.state.fill), self.text());
        self.push(
            json!({"op": "fillText", "text": text, "x": x, "y": y}),
            &[
                json!({"fill": colour, "alpha": self.state.alpha}),
                text_state,
            ],
        );
    }
    fn stroke_text(&mut self, text: &str, x: f64, y: f64) {
        let (line, text_state) = (self.line(), self.text());
        self.push(
            json!({"op": "strokeText", "text": text, "x": x, "y": y}),
            &[line, text_state],
        );
    }
}
