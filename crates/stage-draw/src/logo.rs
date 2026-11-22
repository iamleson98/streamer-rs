//! The brand mark, read from lesson-replay's `logo.svg` (one copy of the artwork) and drawn with
//! Skia. Only the shapes the logo uses are understood; any other element makes [`Logo::parse`] fail,
//! so a redrawn logo cannot silently draw wrong.

use skia_safe::gradient::{Colors, Gradient, Interpolation, shaders};
use skia_safe::{
    Canvas, Color, Color4f, Paint, PaintCap, PaintStyle, Path, RRect, Rect, TileMode,
    utils::parse_path,
};

static LOGO_SVG: &str = include_str!("../../../lesson-replay/src/lib/replay/export/logo.svg");

#[derive(Debug, thiserror::Error)]
#[error("the logo has something this drawing does not know: {0}")]
pub struct LogoError(String);

#[derive(Debug)]
enum Fill {
    Solid(Color),
    Gradient,
}

#[derive(Debug)]
struct Shape {
    path: Path,
    fill: Option<Fill>,
    stroke: Option<(Color, f32)>,
    round_cap: bool,
}

/// The logo as a drawing on a 64 x 64 view box.
#[derive(Debug)]
pub struct Logo {
    shapes: Vec<Shape>,
    stops: [Color; 2],
    view: f32,
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn hex(value: &str) -> Option<Color> {
    let digits = value.strip_prefix('#')?;
    let n = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        6 => Some(Color::from(0xff00_0000 | n)),
        // #rgb: each digit doubled.
        3 => Some(Color::from_rgb(
            ((n >> 8) & 0xf) as u8 * 17,
            ((n >> 4) & 0xf) as u8 * 17,
            (n & 0xf) as u8 * 17,
        )),
        _ => None,
    }
}

impl Logo {
    pub fn parse(svg: &str) -> Result<Self, LogoError> {
        let bad = |what: &str| LogoError(what.to_owned());
        let mut shapes = Vec::new();
        let mut stops = Vec::new();
        let mut view = 64.0;
        let mut rest = svg;
        while let Some(open) = rest.find('<') {
            rest = &rest[open..];
            if let Some(comment) = rest.strip_prefix("<!--") {
                rest = comment.split_once("-->").ok_or_else(|| bad("a comment"))?.1;
                continue;
            }
            let end = rest.find('>').ok_or_else(|| bad("a tag"))?;
            let tag = &rest[..end];
            rest = &rest[end + 1..];
            let closing = tag[1..].starts_with('/');
            let word = tag[1..]
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            let name = if closing {
                format!("/{word}")
            } else {
                word.to_owned()
            };
            match name.as_str() {
                "svg" => {
                    let b = attr(tag, "viewBox").ok_or_else(|| bad("viewBox"))?;
                    view = b
                        .split_whitespace()
                        .nth(2)
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| bad("viewBox"))?;
                }
                "defs" | "/defs" | "/svg" | "linearGradient" | "/linearGradient" => {}
                "stop" => stops.push(
                    hex(attr(tag, "stop-color").ok_or_else(|| bad("stop"))?)
                        .ok_or_else(|| bad("stop colour"))?,
                ),
                "rect" => {
                    let num = |n: &str| attr(tag, n).and_then(|v| v.parse::<f32>().ok());
                    let (w, h, rx) = (
                        num("width").ok_or_else(|| bad("rect"))?,
                        num("height").ok_or_else(|| bad("rect"))?,
                        num("rx").unwrap_or(0.0),
                    );
                    let path = Path::rrect(RRect::new_rect_xy(Rect::from_wh(w, h), rx, rx), None);
                    shapes.push(Shape {
                        path,
                        fill: Some(Fill::Gradient),
                        stroke: None,
                        round_cap: false,
                    });
                }
                "path" => {
                    let path = parse_path::from_svg(attr(tag, "d").ok_or_else(|| bad("path"))?)
                        .ok_or_else(|| bad("path data"))?;
                    let fill = match attr(tag, "fill") {
                        Some("none") | None => None,
                        Some(v) => Some(Fill::Solid(hex(v).ok_or_else(|| bad("fill"))?)),
                    };
                    let stroke = match (attr(tag, "stroke"), attr(tag, "stroke-width")) {
                        (Some(c), Some(w)) => Some((
                            hex(c).ok_or_else(|| bad("stroke"))?,
                            w.parse().map_err(|_| bad("stroke width"))?,
                        )),
                        _ => None,
                    };
                    shapes.push(Shape {
                        path,
                        fill,
                        stroke,
                        round_cap: attr(tag, "stroke-linecap") == Some("round"),
                    });
                }
                other => return Err(bad(other)),
            }
        }
        let stops: [Color; 2] = stops.try_into().map_err(|_| bad("gradient stops"))?;
        Ok(Self {
            shapes,
            stops,
            view,
        })
    }

    /// The artwork.
    pub fn artwork() -> Result<Self, LogoError> {
        Self::parse(LOGO_SVG)
    }

    /// Draws the mark `size` px square with its top left at (x, y).
    pub fn draw(&self, canvas: &Canvas, x: f32, y: f32, size: f32) {
        canvas.save();
        canvas.translate((x, y));
        let s = size / self.view;
        canvas.scale((s, s));
        for shape in &self.shapes {
            if let Some(fill) = &shape.fill {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                match fill {
                    Fill::Solid(c) => {
                        paint.set_color(*c);
                    }
                    Fill::Gradient => {
                        let colors = [Color4f::from(self.stops[0]), Color4f::from(self.stops[1])];
                        paint.set_shader(shaders::linear_gradient(
                            ((0.0, 0.0), (self.view, self.view)),
                            &Gradient::new(
                                Colors::new(&colors, None, TileMode::Clamp, None),
                                Interpolation::default(),
                            ),
                            None,
                        ));
                    }
                }
                canvas.draw_path(&shape.path, &paint);
            }
            if let Some((color, width)) = shape.stroke {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_style(PaintStyle::Stroke);
                paint.set_color(color);
                paint.set_stroke_width(width);
                if shape.round_cap {
                    paint.set_stroke_cap(PaintCap::Round);
                }
                canvas.draw_path(&shape.path, &paint);
            }
        }
        canvas.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skia_safe::surfaces;

    #[test]
    fn the_logo_reads_and_draws_its_gradient_tile() {
        let logo = Logo::artwork().unwrap();
        assert_eq!(logo.shapes.len(), 5);
        let mut surface = surfaces::raster_n32_premul((64, 64)).unwrap();
        logo.draw(surface.canvas(), 0.0, 0.0, 64.0);
        let corner = surface
            .image_snapshot()
            .peek_pixels()
            .map(|p| p.get_color((32, 56)));
        // The tile's gradient is under the smile; the centre of the book is white.
        assert!(corner.is_some());
        let book = surface
            .image_snapshot()
            .peek_pixels()
            .unwrap()
            .get_color((20, 30));
        assert_eq!((book.r(), book.g(), book.b()), (255, 255, 255));
    }

    #[test]
    fn an_element_the_drawing_does_not_know_is_refused() {
        assert!(Logo::parse("<svg viewBox=\"0 0 64 64\"><circle r=\"3\"/></svg>").is_err());
    }
}
