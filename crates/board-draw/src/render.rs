//! Draws the board on a canvas. Port of `board/render.ts`: pure drawing, with a bitmap of the
//! committed elements that is redrawn only when they, the camera, the size or the theme change,
//! so the strokes being drawn (updated twenty times a second each) cost one copy of the bitmap
//! and their own drawing, whatever the size of the board.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use perfect_freehand::{InputPoint, StrokeOptions, get_stroke};
use replay_model::element::{LINE_HEIGHT, element_bounds, stroke_radius};
use replay_model::element::{Page, ShapeKind};
use replay_model::viewport::visible_rect;
use replay_model::{BoardElement, ColorToken, ElementKind, MeasureText, OrderedElement, Viewport};

use crate::canvas::{Canvas, LineCap, LineJoin, Path, Rgba};
use crate::pictures::{PictureState, Pictures};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardTheme {
    Light,
    Dark,
}

/// Palette tokens per theme: the same ink stays readable on both boards.
pub fn ink(color: ColorToken, theme: BoardTheme) -> Rgba {
    use BoardTheme::{Dark, Light};
    let (light, dark) = match color {
        ColorToken::Ink => (0x1e1e1e, 0xececec),
        ColorToken::Red => (0xe03131, 0xff6b6b),
        ColorToken::Orange => (0xe8590c, 0xffa94d),
        ColorToken::Green => (0x2f9e44, 0x69db7c),
        ColorToken::Blue => (0x1971c2, 0x4dabf7),
        ColorToken::Violet => (0x7048e8, 0xb197fc),
    };
    Rgba::hex(match theme {
        Light => light,
        Dark => dark,
    })
}

const GRID: f64 = 32.0;

/// The outline of a freehand stroke as a closed path of quadratic curves through midpoints.
fn stroke_outline(size: f64, highlighter: bool, points: &[f64]) -> Path {
    let mut input = Vec::with_capacity(points.len() / 3);
    let mut constant_pressure = true;
    for p in points.as_chunks::<3>().0 {
        input.push(InputPoint::Array([p[0], p[1]], Some(p[2])));
        if p[2] != 0.5 {
            constant_pressure = false;
        }
    }
    let options = StrokeOptions {
        size: Some(stroke_radius(size, highlighter) * 2.0),
        thinning: Some(if highlighter { 0.0 } else { 0.55 }),
        smoothing: Some(0.5),
        streamline: Some(0.45),
        simulate_pressure: Some(constant_pressure && !highlighter),
        last: Some(true),
        ..Default::default()
    };
    let outline = get_stroke(&input, &options);
    let mut path = Path::new();
    if outline.is_empty() {
        return path;
    }
    path.move_to(outline[0][0], outline[0][1]);
    for i in 1..outline.len() {
        let [x0, y0] = outline[i];
        let [x1, y1] = outline[(i + 1) % outline.len()];
        path.quadratic_curve_to(x0, y0, (x0 + x1) / 2.0, (y0 + y1) / 2.0);
    }
    path.close_path();
    path
}

/// Cached outlines: elements are immutable, so an outline is valid for as long as its element
/// lives. The element is held with it so its address is never reused while it is cached.
#[derive(Default)]
struct Outlines(HashMap<usize, (Arc<BoardElement>, Rc<Path>)>);

impl Outlines {
    fn of(&mut self, el: &Arc<BoardElement>) -> Rc<Path> {
        let ElementKind::Stroke {
            highlighter,
            points,
        } = &el.kind
        else {
            unreachable!("outlines are of strokes")
        };
        let entry = self.0.entry(Arc::as_ptr(el) as usize).or_insert_with(|| {
            (
                Arc::clone(el),
                Rc::new(stroke_outline(el.size, *highlighter, points)),
            )
        });
        Rc::clone(&entry.1)
    }
}

fn draw_element<C: Canvas>(
    ctx: &mut C,
    el: &BoardElement,
    outline: Option<&Path>,
    theme: BoardTheme,
) {
    let color = ink(el.color, theme);
    ctx.set_fill_style(color);
    ctx.set_stroke_style(color);
    ctx.set_line_width(el.size);
    ctx.set_line_cap(LineCap::Round);
    ctx.set_line_join(LineJoin::Round);
    match &el.kind {
        ElementKind::Stroke {
            highlighter,
            points,
        } => {
            ctx.save();
            if *highlighter {
                ctx.set_global_alpha(if theme == BoardTheme::Light {
                    0.35
                } else {
                    0.45
                });
            }
            match outline {
                Some(path) => ctx.fill(path),
                None => ctx.fill(&stroke_outline(el.size, *highlighter, points)),
            }
            ctx.restore();
        }
        ElementKind::Shape {
            shape: shape @ (ShapeKind::Line | ShapeKind::Arrow),
            x1,
            y1,
            x2,
            y2,
        } => {
            let mut path = Path::new();
            path.move_to(*x1, *y1);
            path.line_to(*x2, *y2);
            if *shape == ShapeKind::Arrow {
                let angle = (y2 - y1).atan2(x2 - x1);
                let head = (el.size * 3.5).max(12.0);
                for side in [-1.0, 1.0] {
                    let a = angle + std::f64::consts::PI - side * (std::f64::consts::PI / 7.0);
                    path.move_to(*x2, *y2);
                    path.line_to(x2 + a.cos() * head, y2 + a.sin() * head);
                }
            }
            ctx.stroke(&path);
        }
        ElementKind::Shape {
            shape: ShapeKind::Rect,
            x1,
            y1,
            x2,
            y2,
        } => {
            let (w, h) = ((x2 - x1).abs(), (y2 - y1).abs());
            let mut path = Path::new();
            path.round_rect(
                x1.min(*x2),
                y1.min(*y2),
                w,
                h,
                8f64.min(w / 4.0).min(h / 4.0),
            );
            ctx.stroke(&path);
        }
        ElementKind::Shape {
            shape: ShapeKind::Ellipse,
            x1,
            y1,
            x2,
            y2,
        } => {
            let mut path = Path::new();
            path.ellipse(
                (x1 + x2) / 2.0,
                (y1 + y2) / 2.0,
                (x2 - x1).abs() / 2.0,
                (y2 - y1).abs() / 2.0,
                0.0,
                std::f64::consts::TAU,
                false,
            );
            ctx.stroke(&path);
        }
        ElementKind::Pack(pack) => crate::maths::draw_pack(ctx, el, pack, theme),
        ElementKind::Text { x, y, text } => {
            ctx.set_font_size(el.size);
            for (i, line) in text.split('\n').enumerate() {
                ctx.fill_text(line, *x, y + i as f64 * el.size * LINE_HEIGHT);
            }
        }
        // Sheets are drawn by `draw_sheet`; the view is never drawn.
        ElementKind::Page(_) | ElementKind::View { .. } => {}
    }
}

/// Draws one sheet of the board: white paper with an edge, and the picture of its deck page
/// fitted inside it. While the picture is on its way the sheet shows a ring, and when it cannot be
/// had a crossed frame, so a sheet is never confused with an empty one. Paper stays white on a dark
/// board: slides are made for it.
fn draw_sheet<C: Canvas>(
    ctx: &mut C,
    page: &Page,
    picture: Option<PictureState<C::Image>>,
    theme: BoardTheme,
    zoom: f64,
) {
    ctx.save();
    ctx.set_fill_style(Rgba::hex(0xffffff));
    ctx.fill_rect(page.x, page.y, page.w, page.h);
    match picture {
        Some(PictureState::Ready(image)) => {
            let (iw, ih) = C::image_size(&image);
            let scale = (page.w / iw).min(page.h / ih);
            let (w, h) = (iw * scale, ih * scale);
            ctx.draw_image(
                &image,
                page.x + (page.w - w) / 2.0,
                page.y + (page.h - h) / 2.0,
                w,
                h,
            );
        }
        Some(PictureState::Loading) => draw_mark(ctx, page, true),
        Some(PictureState::Failed) => draw_mark(ctx, page, false),
        None => {}
    }
    ctx.set_stroke_style(match theme {
        BoardTheme::Light => Rgba::with_alpha(15, 23, 42, 0.28),
        BoardTheme::Dark => Rgba::with_alpha(226, 232, 240, 0.4),
    });
    // The edge stays visible when the board is zoomed far out.
    ctx.set_line_width(2f64.max(1.5 / zoom));
    ctx.stroke_rect(page.x, page.y, page.w, page.h);
    ctx.restore();
}

/// A ring while the picture loads, a crossed frame when it cannot be shown.
fn draw_mark<C: Canvas>(ctx: &mut C, page: &Page, loading: bool) {
    let cx = page.x + page.w / 2.0;
    let cy = page.y + page.h / 2.0;
    let r = page.w.min(page.h) / 10.0;
    ctx.set_stroke_style(Rgba::hex(0x94a3b8));
    ctx.set_line_width(r / 5.0);
    ctx.set_line_cap(LineCap::Round);
    let mut path = Path::new();
    if loading {
        path.arc(cx, cy, r, 0.0, std::f64::consts::PI * 1.5, false);
    } else {
        path.rect(cx - r, cy - r, r * 2.0, r * 2.0);
        path.move_to(cx - r, cy - r);
        path.line_to(cx + r, cy + r);
        path.move_to(cx + r, cy - r);
        path.line_to(cx - r, cy + r);
    }
    ctx.stroke(&path);
}

/// What one frame of the board is drawn from.
pub struct DrawOptions<'a> {
    pub elements: &'a [OrderedElement],
    /// Changes whenever `elements` do (the scene's revision).
    pub revision: u64,
    /// Drawn above the board: in-progress strokes (own and others').
    pub overlays: &'a [&'a BoardElement],
    /// Elements hidden from the board (being moved or edited).
    pub hidden: &'a [String],
    pub viewport: Viewport,
    /// The board's size in CSS pixels; the canvas has `dpr` device pixels to each.
    pub width: f64,
    pub height: f64,
    pub dpr: f64,
    pub theme: BoardTheme,
    pub measure: &'a dyn MeasureText,
}

impl std::fmt::Debug for DrawOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrawOptions")
            .field("revision", &self.revision)
            .field("viewport", &self.viewport)
            .field("size", &(self.width, self.height))
            .finish_non_exhaustive()
    }
}

/// What the committed layer was drawn from.
#[derive(Debug, PartialEq)]
struct LayerKey {
    revision: u64,
    viewport: Viewport,
    canvas: (u32, u32),
    dpr: f64,
    theme: BoardTheme,
    hidden: Vec<String>,
    pictures: u64,
}

pub struct BoardRenderer<C: Canvas> {
    layer: Option<(C, C::Image)>,
    key: Option<LayerKey>,
    outlines: Outlines,
}

impl<C: Canvas> std::fmt::Debug for BoardRenderer<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardRenderer")
            .field("key", &self.key)
            .finish()
    }
}

impl<C: Canvas> Default for BoardRenderer<C> {
    fn default() -> Self {
        Self {
            layer: None,
            key: None,
            outlines: Outlines::default(),
        }
    }
}

impl<C: Canvas> BoardRenderer<C> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws a frame: the committed layer, then the overlays. The canvas is cleared first, so the
    /// board is on transparent (the painter puts the paper under it).
    pub fn draw(&mut self, ctx: &mut C, o: &DrawOptions<'_>, pictures: &dyn Pictures<C::Image>) {
        self.update_layer(ctx, o, pictures);
        let (cw, ch) = ctx.size();
        ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        ctx.clear_rect(0.0, 0.0, f64::from(cw), f64::from(ch));
        if let Some((_, image)) = &self.layer {
            ctx.draw_image(image, 0.0, 0.0, f64::from(cw), f64::from(ch));
        }
        world_transform(ctx, o);
        for el in o.overlays {
            draw_element(ctx, el, None, o.theme);
        }
    }

    fn update_layer(&mut self, ctx: &C, o: &DrawOptions<'_>, pictures: &dyn Pictures<C::Image>) {
        let canvas = ctx.size();
        let key = LayerKey {
            revision: o.revision,
            viewport: o.viewport,
            canvas,
            dpr: o.dpr,
            theme: o.theme,
            hidden: o.hidden.to_vec(),
            pictures: pictures.revision(),
        };
        if self.key.as_ref() == Some(&key) && self.layer.is_some() {
            return;
        }
        let mut lctx = match self.layer.take() {
            Some((c, _)) if c.size() == canvas => c,
            _ => ctx.offscreen(canvas.0, canvas.1),
        };
        lctx.set_transform(o.dpr, 0.0, 0.0, o.dpr, 0.0, 0.0);
        lctx.clear_rect(0.0, 0.0, o.width, o.height);
        draw_grid(&mut lctx, o);
        world_transform(&mut lctx, o);
        let view = visible_rect(o.viewport, o.width, o.height);
        let mut seen = HashMap::new();
        for e in o.elements {
            if o.hidden.contains(&e.id) {
                continue;
            }
            if !element_bounds(&e.el, o.measure).intersects(&view) {
                continue;
            }
            match &e.el.kind {
                ElementKind::Page(page) => {
                    let picture = page.picture.as_ref().map(|p| pictures.get(&p.deck, p.n));
                    draw_sheet(&mut lctx, page, picture, o.theme, o.viewport.zoom);
                }
                ElementKind::Stroke { .. } => {
                    let outline = self.outlines.of(&e.el);
                    draw_element(&mut lctx, &e.el, Some(&outline), o.theme);
                    seen.insert(Arc::as_ptr(&e.el) as usize, ());
                }
                _ => draw_element(&mut lctx, &e.el, None, o.theme),
            }
        }
        // Outlines of elements that are gone (or off screen) are let go of.
        self.outlines.0.retain(|ptr, _| seen.contains_key(ptr));
        let image = lctx.image();
        self.layer = Some((lctx, image));
        self.key = Some(key);
    }
}

/// Maps board units to the canvas's pixels.
fn world_transform<C: Canvas>(ctx: &mut C, o: &DrawOptions<'_>) {
    let z = o.viewport.zoom;
    ctx.set_transform(
        o.dpr * z,
        0.0,
        0.0,
        o.dpr * z,
        -o.viewport.x * z * o.dpr,
        -o.viewport.y * z * o.dpr,
    );
}

/// A dot grid that pans and zooms with the board (hidden when too dense).
fn draw_grid<C: Canvas>(ctx: &mut C, o: &DrawOptions<'_>) {
    let step = GRID * o.viewport.zoom;
    if step < 12.0 {
        return;
    }
    ctx.set_fill_style(match o.theme {
        BoardTheme::Light => Rgba::with_alpha(15, 23, 42, 0.13),
        BoardTheme::Dark => Rgba::with_alpha(226, 232, 240, 0.12),
    });
    let offset_x = -((o.viewport.x * o.viewport.zoom) % step);
    let offset_y = -((o.viewport.y * o.viewport.zoom) % step);
    let r = 0.8f64.max(1.4f64.min(o.viewport.zoom));
    let mut x = offset_x;
    while x < o.width {
        let mut y = offset_y;
        while y < o.height {
            ctx.fill_rect(x - r / 2.0, y - r / 2.0, r, r);
            y += step;
        }
        x += step;
    }
}
