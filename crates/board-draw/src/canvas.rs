//! The part of the Canvas 2D API the board draws with, as a trait. The board code is written
//! against it exactly as the web player is written against `CanvasRenderingContext2D`, so the two
//! stay line for line comparable; `skia::SkiaCanvas` is the one backend.

use std::rc::Rc;

use replay_model::MeasureText;

/// A colour as CSS writes it: 8-bit channels and an alpha from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn with_alpha(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// `#rrggbb`.
    pub const fn hex(rgb: u32) -> Self {
        Self::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

/// One step of a path, in the terms of `Path2D`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathOp {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    QuadraticCurveTo {
        cx: f64,
        cy: f64,
        x: f64,
        y: f64,
    },
    Ellipse {
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        start: f64,
        end: f64,
        ccw: bool,
    },
    RoundRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        radius: f64,
    },
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    },
    ClosePath,
}

/// A path, built like `Path2D` and drawn by any canvas. Ellipses are never rotated: nothing on
/// the board is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    pub(crate) ops: Vec<PathOp>,
}

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ops(&self) -> &[PathOp] {
        &self.ops
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        self.ops.push(PathOp::MoveTo(x, y));
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        self.ops.push(PathOp::LineTo(x, y));
    }

    pub fn quadratic_curve_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        self.ops.push(PathOp::QuadraticCurveTo { cx, cy, x, y });
    }

    /// A circular arc, clockwise unless `ccw`; angles in radians from the positive x axis.
    pub fn arc(&mut self, cx: f64, cy: f64, r: f64, start: f64, end: f64, ccw: bool) {
        self.ellipse(cx, cy, r, r, start, end, ccw);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, start: f64, end: f64, ccw: bool) {
        self.ops.push(PathOp::Ellipse {
            cx,
            cy,
            rx,
            ry,
            start,
            end,
            ccw,
        });
    }

    pub fn round_rect(&mut self, x: f64, y: f64, w: f64, h: f64, radius: f64) {
        self.ops.push(PathOp::RoundRect { x, y, w, h, radius });
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.ops.push(PathOp::Rect { x, y, w, h });
    }

    pub fn close_path(&mut self) {
        self.ops.push(PathOp::ClosePath);
    }
}

/// A 2D drawing surface with the state and methods of `CanvasRenderingContext2D` that the board
/// uses. Text is always drawn with `textBaseline = 'top'` in the one pinned font family.
pub trait Canvas: Sized {
    /// A picture that can be drawn on any canvas of this kind.
    type Image: Clone;

    /// Width and height in device pixels.
    fn size(&self) -> (u32, u32);
    /// A new transparent canvas of this size that draws text the same way.
    fn offscreen(&self, width: u32, height: u32) -> Self;
    /// What is drawn so far, as an image (a canvas is an image source in the web API).
    fn image(&mut self) -> Self::Image;
    fn image_size(image: &Self::Image) -> (f64, f64);
    /// Measures text as the canvas draws it (`measureText(...).width`), memoized.
    fn measurer(&self) -> Rc<dyn MeasureText>;

    fn save(&mut self);
    fn restore(&mut self);
    fn set_transform(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64);
    fn clip(&mut self, path: &Path);

    fn set_fill_style(&mut self, color: Rgba);
    fn set_stroke_style(&mut self, color: Rgba);
    fn set_line_width(&mut self, width: f64);
    fn set_line_cap(&mut self, cap: LineCap);
    fn set_line_join(&mut self, join: LineJoin);
    /// An empty slice is a solid line.
    fn set_line_dash(&mut self, dash: &[f64]);
    fn set_global_alpha(&mut self, alpha: f64);
    /// The font is `${size}px` in the pinned family.
    fn set_font_size(&mut self, size: f64);

    fn fill(&mut self, path: &Path);
    fn stroke(&mut self, path: &Path);
    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64);
    fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64);
    fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64);
    fn draw_image(&mut self, image: &Self::Image, x: f64, y: f64, w: f64, h: f64);
    /// `fillText` with `textBaseline = 'top'`: `y` is the top of the line.
    fn fill_text(&mut self, text: &str, x: f64, y: f64);
}
