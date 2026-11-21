//! The Canvas backend on Skia: a CPU raster surface, text through `skia_safe::textlayout` with
//! the font bytes the caller passes in. There are no system fonts: a glyph the given faces do not
//! have draws as nothing, on every machine alike.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use replay_model::MeasureText;
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextStyle, TypefaceFontProvider,
};
use skia_safe::{
    AlphaType, ClipOp, Color, ColorType, Data, EncodedImageFormat, FilterMode, Font, FontMgr,
    Image, ImageInfo, MipmapMode, Paint, PaintCap, PaintJoin, PaintStyle, PathBuilder,
    PathDirection, PathEffect, RRect, Rect, SamplingOptions, Surface, Typeface, surfaces,
};

use crate::canvas::{Canvas, LineCap, LineJoin, Path, PathOp, Rgba};

#[derive(Debug, thiserror::Error)]
pub enum SkiaError {
    #[error("font face {0} did not parse")]
    Font(usize),
    #[error("no font faces were given")]
    NoFonts,
    #[error("a {0}x{1} surface could not be made")]
    Surface(u32, u32),
}

/// The board's font files in the order of its font stack (`"Inter Variable", "Noto Sans",
/// "Noto Sans Math", "Noto Sans Symbols 2"`), as they are in the lesson-replay submodule's
/// `static/fonts/`: pass their bytes to [`FontSet::new`] in this order, so that each character is
/// drawn from the face the web player takes it from.
pub const BOARD_FONT_FILES: [&str; 6] = [
    "inter-5.3.0-latin-var-wght.woff2",
    "inter-5.3.0-vietnamese-var-wght.woff2",
    "inter-4.1-board-var-wght.woff2",
    "noto-sans-2.015-superscripts.woff2",
    "noto-sans-math-3.000-symbols.woff2",
    "noto-sans-symbols-2-2.008-symbols.woff2",
];

/// The fonts the board's text is drawn with: the faces in the order they are tried for each glyph.
pub struct FontSet {
    collection: FontCollection,
    families: Vec<String>,
    first: Typeface,
}

impl std::fmt::Debug for FontSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontSet")
            .field("families", &self.families)
            .finish()
    }
}

impl FontSet {
    /// `faces` are font files (woff2, TrueType or OpenType, possibly variable), most preferred
    /// first. Each is its own family: a glyph is taken from the first face that has it. The board's
    /// subsets hold only the characters their `unicode-range` gives them, so the order alone makes
    /// the same choice as the browser. Nothing else is ever consulted (no system fonts).
    pub fn new(faces: &[&[u8]]) -> Result<Rc<Self>, SkiaError> {
        let mgr = FontMgr::default();
        let mut provider = TypefaceFontProvider::new();
        let mut families = Vec::new();
        let mut first = None;
        for (i, bytes) in faces.iter().enumerate() {
            let typeface = mgr
                .new_from_data(Data::new_copy(bytes), None)
                .ok_or(SkiaError::Font(i))?;
            let alias = format!("board-face-{i}");
            provider.register_typeface(typeface.clone(), alias.as_str());
            families.push(alias);
            first.get_or_insert(typeface);
        }
        let first = first.ok_or(SkiaError::NoFonts)?;
        let mut collection = FontCollection::new();
        collection.set_asset_font_manager(Some(provider.into()));
        collection.disable_font_fallback();
        Ok(Rc::new(Self {
            collection,
            families,
            first,
        }))
    }

    fn paragraph(&self, text: &str, size: f64, color: Color) -> Paragraph {
        let mut style = TextStyle::new();
        style.set_font_size(size as f32);
        style.set_font_families(&self.families);
        style.set_color(color);
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_style(&style);
        paragraph_style.set_apply_rounding_hack(false);
        let mut builder = ParagraphBuilder::new(&paragraph_style, &self.collection);
        builder.push_style(&style);
        builder.add_text(text);
        let mut paragraph = builder.build();
        // Never wrapped: a line of the board ends where its text does.
        paragraph.layout(1.0e7);
        paragraph
    }

    /// Where the baseline of a `textBaseline = 'top'` line is below its top: the top of the em
    /// box, which is the font's ascent scaled so that ascent and descent make up one em (what
    /// `measureText` calls `emHeightAscent`).
    fn top_to_baseline(&self, size: f64) -> f64 {
        let (_, m) = Font::from_typeface(&self.first, size as f32).metrics();
        let (ascent, descent) = (f64::from(-m.ascent), f64::from(m.descent));
        size * ascent / (ascent + descent)
    }
}

/// `measureText` memoized per (size, line).
struct SkiaMeasure {
    fonts: Rc<FontSet>,
    cache: RefCell<HashMap<(u64, String), f64>>,
}

impl MeasureText for SkiaMeasure {
    fn width(&self, line: &str, font_size: f64) -> f64 {
        let key = (font_size.to_bits(), line.to_owned());
        if let Some(w) = self.cache.borrow().get(&key) {
            return *w;
        }
        let w = f64::from(
            self.fonts
                .paragraph(line, font_size, Color::BLACK)
                .max_intrinsic_width(),
        );
        let mut cache = self.cache.borrow_mut();
        if cache.len() > 5000 {
            cache.clear();
        }
        cache.insert(key, w);
        w
    }
}

#[derive(Clone)]
struct Style {
    fill: Rgba,
    stroke: Rgba,
    line_width: f64,
    cap: LineCap,
    join: LineJoin,
    dash: Vec<f64>,
    alpha: f64,
    font_size: f64,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fill: Rgba::rgb(0, 0, 0),
            stroke: Rgba::rgb(0, 0, 0),
            line_width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            dash: Vec::new(),
            alpha: 1.0,
            font_size: 10.0,
        }
    }
}

/// A CPU raster canvas.
pub struct SkiaCanvas {
    surface: Surface,
    fonts: Rc<FontSet>,
    measure: Rc<SkiaMeasure>,
    style: Style,
    saved: Vec<Style>,
}

impl std::fmt::Debug for SkiaCanvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkiaCanvas")
            .field("size", &(self.surface.width(), self.surface.height()))
            .finish()
    }
}

fn color_of(c: Rgba, alpha: f64) -> Color {
    let a = (f64::from(c.a) * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::from_argb(a, c.r, c.g, c.b)
}

impl SkiaCanvas {
    /// A transparent canvas of `width` x `height` device pixels.
    pub fn new(width: u32, height: u32, fonts: Rc<FontSet>) -> Result<Self, SkiaError> {
        let surface = surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or(SkiaError::Surface(width, height))?;
        let measure = Rc::new(SkiaMeasure {
            fonts: Rc::clone(&fonts),
            cache: RefCell::default(),
        });
        Ok(Self {
            surface,
            fonts,
            measure,
            style: Style::default(),
            saved: Vec::new(),
        })
    }

    /// The pixels as 8-bit RGBA (not premultiplied), row by row.
    pub fn rgba(&mut self) -> Vec<u8> {
        let (w, h) = (self.surface.width(), self.surface.height());
        let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let ok = self
            .surface
            .read_pixels(&info, &mut pixels, (w * 4) as usize, (0, 0));
        assert!(ok, "reading the surface's pixels");
        pixels
    }

    /// The canvas as a PNG file.
    pub fn png(&mut self) -> Vec<u8> {
        let image = self.surface.image_snapshot();
        image
            .encode(None, EncodedImageFormat::PNG, None)
            .expect("PNG encoding of a raster image")
            .as_bytes()
            .to_vec()
    }

    fn paint(&self, style: PaintStyle) -> Paint {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_style(style);
        match style {
            PaintStyle::Stroke => {
                p.set_color(color_of(self.style.stroke, self.style.alpha));
                p.set_stroke_width(self.style.line_width as f32);
                p.set_stroke_cap(match self.style.cap {
                    LineCap::Butt => PaintCap::Butt,
                    LineCap::Round => PaintCap::Round,
                    LineCap::Square => PaintCap::Square,
                });
                p.set_stroke_join(match self.style.join {
                    LineJoin::Miter => PaintJoin::Miter,
                    LineJoin::Round => PaintJoin::Round,
                    LineJoin::Bevel => PaintJoin::Bevel,
                });
                if !self.style.dash.is_empty() {
                    let mut intervals: Vec<f32> =
                        self.style.dash.iter().map(|d| *d as f32).collect();
                    // An odd list is repeated, as the web API does.
                    if intervals.len() % 2 == 1 {
                        intervals.extend_from_within(..);
                    }
                    p.set_path_effect(PathEffect::dash(&intervals, 0.0));
                }
            }
            _ => {
                p.set_color(color_of(self.style.fill, self.style.alpha));
            }
        }
        p
    }

    fn rect_path(x: f64, y: f64, w: f64, h: f64) -> skia_safe::Path {
        let mut b = PathBuilder::new();
        b.add_rect(
            Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
            None,
            None,
        );
        b.detach()
    }
}

/// A full turn or more is drawn as two half turns (Skia treats a sweep of 360 degrees as none).
fn add_ellipse(b: &mut PathBuilder, c: (f64, f64), r: (f64, f64), start: f64, end: f64, ccw: bool) {
    use std::f64::consts::TAU;
    let oval = Rect::new(
        (c.0 - r.0) as f32,
        (c.1 - r.1) as f32,
        (c.0 + r.0) as f32,
        (c.1 + r.1) as f32,
    );
    let delta = end - start;
    let sweep = if !ccw && delta >= TAU {
        TAU
    } else if ccw && -delta >= TAU {
        -TAU
    } else if !ccw {
        delta.rem_euclid(TAU)
    } else {
        -(-delta).rem_euclid(TAU)
    };
    let start_deg = start.to_degrees() as f32;
    if sweep.abs() >= TAU {
        let half = (sweep.signum() * 180.0) as f32;
        b.arc_to(oval, start_deg, half, false);
        b.arc_to(oval, start_deg + half, half, false);
    } else {
        b.arc_to(oval, start_deg, sweep.to_degrees() as f32, false);
    }
}

fn to_skia(path: &Path) -> skia_safe::Path {
    let mut b = PathBuilder::new();
    for op in path.ops() {
        match *op {
            PathOp::MoveTo(x, y) => {
                b.move_to((x as f32, y as f32));
            }
            PathOp::LineTo(x, y) => {
                b.line_to((x as f32, y as f32));
            }
            PathOp::QuadraticCurveTo { cx, cy, x, y } => {
                b.quad_to((cx as f32, cy as f32), (x as f32, y as f32));
            }
            PathOp::Ellipse {
                cx,
                cy,
                rx,
                ry,
                start,
                end,
                ccw,
            } => {
                add_ellipse(&mut b, (cx, cy), (rx, ry), start, end, ccw);
            }
            PathOp::RoundRect { x, y, w, h, radius } => {
                let rect = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32);
                b.add_rrect(
                    RRect::new_rect_xy(rect, radius as f32, radius as f32),
                    PathDirection::CW,
                    None,
                );
            }
            PathOp::Rect { x, y, w, h } => {
                b.add_rect(
                    Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
                    None,
                    None,
                );
            }
            PathOp::ClosePath => {
                b.close();
            }
        }
    }
    b.detach()
}

/// A picture, decoded from an encoded file (PNG, JPEG), for a canvas of this backend.
pub fn decode_image(bytes: &[u8]) -> Option<Image> {
    Image::from_encoded(Data::new_copy(bytes))
}

/// A PNG (or other encoded picture) as 8-bit RGBA, not premultiplied: width, height, pixels.
pub fn decode_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let image = decode_image(bytes)?;
    let (w, h) = (image.width(), image.height());
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    image
        .read_pixels(
            &info,
            &mut pixels,
            (w * 4) as usize,
            (0, 0),
            skia_safe::image::CachingHint::Allow,
        )
        .then_some((w as u32, h as u32, pixels))
}

/// 8-bit RGBA pixels (not premultiplied) as a PNG file.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let image =
        skia_safe::images::raster_from_data(&info, Data::new_copy(rgba), (width * 4) as usize)?;
    image
        .encode(None, EncodedImageFormat::PNG, None)
        .map(|d| d.as_bytes().to_vec())
}

impl Canvas for SkiaCanvas {
    type Image = Image;

    fn size(&self) -> (u32, u32) {
        (self.surface.width() as u32, self.surface.height() as u32)
    }

    fn offscreen(&self, width: u32, height: u32) -> Self {
        Self::new(width, height, Rc::clone(&self.fonts)).expect("an offscreen surface")
    }

    fn image(&mut self) -> Image {
        self.surface.image_snapshot()
    }

    fn image_size(image: &Image) -> (f64, f64) {
        (f64::from(image.width()), f64::from(image.height()))
    }

    fn measurer(&self) -> Rc<dyn MeasureText> {
        Rc::clone(&self.measure) as Rc<dyn MeasureText>
    }

    fn save(&mut self) {
        self.surface.canvas().save();
        self.saved.push(self.style.clone());
    }

    fn restore(&mut self) {
        if let Some(style) = self.saved.pop() {
            self.style = style;
            self.surface.canvas().restore();
        }
    }

    fn set_transform(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        let m = skia_safe::Matrix::new_all(
            a as f32, c as f32, e as f32, b as f32, d as f32, f as f32, 0.0, 0.0, 1.0,
        );
        let canvas = self.surface.canvas();
        canvas.reset_matrix();
        canvas.concat(&m);
    }

    fn clip(&mut self, path: &Path) {
        self.surface
            .canvas()
            .clip_path(&to_skia(path), ClipOp::Intersect, true);
    }

    fn set_fill_style(&mut self, color: Rgba) {
        self.style.fill = color;
    }

    fn set_stroke_style(&mut self, color: Rgba) {
        self.style.stroke = color;
    }

    fn set_line_width(&mut self, width: f64) {
        self.style.line_width = width;
    }

    fn set_line_cap(&mut self, cap: LineCap) {
        self.style.cap = cap;
    }

    fn set_line_join(&mut self, join: LineJoin) {
        self.style.join = join;
    }

    fn set_line_dash(&mut self, dash: &[f64]) {
        self.style.dash = dash.to_vec();
    }

    fn set_global_alpha(&mut self, alpha: f64) {
        self.style.alpha = alpha;
    }

    fn set_font_size(&mut self, size: f64) {
        self.style.font_size = size;
    }

    fn fill(&mut self, path: &Path) {
        let paint = self.paint(PaintStyle::Fill);
        self.surface.canvas().draw_path(&to_skia(path), &paint);
    }

    fn stroke(&mut self, path: &Path) {
        let paint = self.paint(PaintStyle::Stroke);
        self.surface.canvas().draw_path(&to_skia(path), &paint);
    }

    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let paint = self.paint(PaintStyle::Fill);
        self.surface
            .canvas()
            .draw_path(&Self::rect_path(x, y, w, h), &paint);
    }

    fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let paint = self.paint(PaintStyle::Stroke);
        self.surface
            .canvas()
            .draw_path(&Self::rect_path(x, y, w, h), &paint);
    }

    fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let mut paint = Paint::default();
        paint.set_blend_mode(skia_safe::BlendMode::Clear);
        self.surface.canvas().draw_rect(
            Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
            &paint,
        );
    }

    fn draw_image(&mut self, image: &Image, x: f64, y: f64, w: f64, h: f64) {
        let mut paint = Paint::default();
        paint.set_alpha_f(self.style.alpha as f32);
        let dst = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32);
        self.surface.canvas().draw_image_rect_with_sampling_options(
            image,
            None,
            dst,
            SamplingOptions::new(FilterMode::Linear, MipmapMode::None),
            &paint,
        );
    }

    fn fill_text(&mut self, text: &str, x: f64, y: f64) {
        if text.is_empty() {
            return;
        }
        let size = self.style.font_size;
        let paragraph =
            self.fonts
                .paragraph(text, size, color_of(self.style.fill, self.style.alpha));
        let top = y + self.fonts.top_to_baseline(size) - f64::from(paragraph.alphabetic_baseline());
        paragraph.paint(self.surface.canvas(), (x as f32, top as f32));
    }
}
