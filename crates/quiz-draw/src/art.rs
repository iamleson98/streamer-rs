//! The player's SVG pictures (characters, the coin, icons) drawn with Skia: the few shapes they use
//! (paths, circles, ellipses, rectangles and lines, with fill, stroke and a rotation) read from the
//! SVG text of `art_data.rs`, and the countdown ring.

use skia_safe::{Canvas, Matrix, Paint, PaintCap, PaintJoin, PaintStyle, PathBuilder, Point, RRect, Rect, utils::parse_path};

use crate::color::Rgba;
use crate::tokens;

#[derive(Debug, Clone, Copy)]
enum Colour {
    None,
    Rgb(Rgba),
    Current,
}

#[derive(Debug, Clone)]
struct Shape {
    path: skia_safe::Path,
    fill: Colour,
    stroke: Colour,
    width: f32,
    round: bool,
}

#[derive(Debug)]
pub struct Art {
    view: Rect,
    shapes: Vec<Shape>,
}

fn colour(v: &str) -> Colour {
    match v {
        "none" => Colour::None,
        "currentColor" => Colour::Current,
        hex => {
            let h = hex.trim_start_matches('#');
            let h: String = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_owned() };
            let n = u32::from_str_radix(&h, 16).unwrap_or_else(|_| panic!("colour {v}"));
            Colour::Rgb(Rgba::new((n >> 16) as u8, (n >> 8) as u8, n as u8, 1.0))
        }
    }
}

impl Art {
    /// Reads an SVG written as `art_data.rs` has it.
    pub fn parse(svg: &str) -> Self {
        let doc = roxmltree::Document::parse(svg).expect("art svg");
        let root = doc.root_element();
        let nums: Vec<f32> = root
            .attribute("viewBox")
            .expect("viewBox")
            .split_whitespace()
            .map(|n| n.parse().expect("viewBox number"))
            .collect();
        let inherit = |el: roxmltree::Node, name: &str| -> Option<String> {
            el.attribute(name).or_else(|| root.attribute(name)).map(str::to_owned)
        };
        let mut shapes = Vec::new();
        for el in root.children().filter(roxmltree::Node::is_element) {
            let f = |n: &str| el.attribute(n).map_or(0.0, |v| v.parse::<f32>().expect("number"));
            let mut b = PathBuilder::new();
            match el.tag_name().name() {
                "path" => {
                    let p = parse_path::from_svg(el.attribute("d").expect("d")).expect("path data");
                    b.add_path(&p, None);
                }
                "circle" => {
                    b.add_oval(Rect::from_xywh(f("cx") - f("r"), f("cy") - f("r"), 2.0 * f("r"), 2.0 * f("r")), None, None);
                }
                "ellipse" => {
                    b.add_oval(Rect::from_xywh(f("cx") - f("rx"), f("cy") - f("ry"), 2.0 * f("rx"), 2.0 * f("ry")), None, None);
                }
                "rect" => {
                    let r = Rect::from_xywh(f("x"), f("y"), f("width"), f("height"));
                    let rx = f("rx");
                    b.add_rrect(RRect::new_rect_xy(r, rx, if el.attribute("ry").is_some() { f("ry") } else { rx }), None, None);
                }
                "line" => {
                    b.move_to((f("x1"), f("y1")));
                    b.line_to((f("x2"), f("y2")));
                }
                other => panic!("svg element {other}"),
            }
            let mut path = b.detach();
            if let Some(t) = el.attribute("transform") {
                // rotate(angle cx cy)
                let v: Vec<f32> = t
                    .trim_start_matches("rotate(")
                    .trim_end_matches(')')
                    .split_whitespace()
                    .map(|n| n.parse().expect("rotate"))
                    .collect();
                let m = Matrix::rotate_deg_pivot(v[0], Point::new(v[1], v[2]));
                path = path.with_transform(&m);
            }
            shapes.push(Shape {
                path,
                fill: colour(&inherit(el, "fill").unwrap_or_else(|| "black".into())),
                stroke: colour(&inherit(el, "stroke").unwrap_or_else(|| "none".into())),
                width: inherit(el, "stroke-width").map_or(1.0, |w| w.parse().expect("stroke-width")),
                round: inherit(el, "stroke-linecap").as_deref() == Some("round"),
            });
        }
        Self { view: Rect::from_xywh(nums[0], nums[1], nums[2], nums[3]), shapes }
    }

    /// Draws the picture in `rect` (scaled to fit), with `current` for `currentColor`.
    pub fn draw(&self, canvas: &Canvas, rect: Rect, current: Rgba) {
        self.draw_with(canvas, rect, current, None, None);
    }

    /// As `draw`, with the fill of shapes that fill with `currentColor`-less defaults replaced and the stroke
    /// width scaled by a factor (`strokeWidth={2.5}` on a lucide icon).
    pub fn draw_with(&self, canvas: &Canvas, rect: Rect, current: Rgba, fill: Option<Rgba>, stroke_width: Option<f32>) {
        canvas.save();
        let scale = (rect.width() / self.view.width()).min(rect.height() / self.view.height());
        canvas.translate((
            rect.left + (rect.width() - self.view.width() * scale) / 2.0,
            rect.top + (rect.height() - self.view.height() * scale) / 2.0,
        ));
        canvas.scale((scale, scale));
        canvas.translate((-self.view.left, -self.view.top));
        for s in &self.shapes {
            let pick = |c: Colour| match c {
                Colour::None => None,
                Colour::Rgb(c) => Some(c),
                Colour::Current => Some(current),
            };
            let fill_colour = match s.fill {
                Colour::None => fill,
                other => pick(other),
            };
            if let Some(c) = fill_colour {
                let mut p = Paint::default();
                p.set_anti_alias(true);
                p.set_color(c.skia(1.0));
                canvas.draw_path(&s.path, &p);
            }
            if let Some(c) = pick(s.stroke) {
                let mut p = Paint::default();
                p.set_anti_alias(true);
                p.set_style(PaintStyle::Stroke);
                p.set_stroke_width(stroke_width.unwrap_or(s.width));
                p.set_color(c.skia(1.0));
                if s.round {
                    p.set_stroke_cap(PaintCap::Round);
                    p.set_stroke_join(PaintJoin::Round);
                }
                canvas.draw_path(&s.path, &p);
            }
        }
        canvas.restore();
    }
}

/// The countdown ring (`quiz/components/ring.svelte`) in `rect`: a track and an arc that starts at the top
/// and runs clockwise. The arc's length is the fraction in steps of 1/60 of the circle, as the render's
/// painter rounds it (`quantiseRings` in `export/dom-painter.ts`).
pub fn draw_ring(canvas: &Canvas, rect: Rect, fraction: f32, color: Rgba) {
    const R: f32 = 44.0;
    let c = 2.0 * std::f32::consts::PI * R;
    let step = c / 60.0;
    let offset = c * (1.0 - fraction.clamp(0.0, 1.0));
    let quantised = ((offset / step).round() * step * 100.0).round() / 100.0;
    let shown = 1.0 - quantised / c;
    let scale = rect.width() / 100.0;
    let mid = Point::new(rect.center_x(), rect.center_y());
    let oval = Rect::from_xywh(mid.x - R * scale, mid.y - R * scale, 2.0 * R * scale, 2.0 * R * scale);
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_style(PaintStyle::Stroke);
    p.set_stroke_width(9.0 * scale);
    p.set_color(tokens::MUTED.skia(1.0));
    canvas.draw_oval(oval, &p);
    if shown > 0.0 {
        p.set_color(color.skia(1.0));
        p.set_stroke_cap(PaintCap::Round);
        if shown >= 1.0 {
            canvas.draw_oval(oval, &p);
        } else {
            let mut b = PathBuilder::new();
            b.arc_to(oval, -90.0, 360.0 * shown, true);
            canvas.draw_path(&b.detach(), &p);
        }
    }
}
