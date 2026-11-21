//! A small retained UI tree: boxes laid out by `taffy` (flexbox, grid and block, as the player's CSS
//! uses them), text measured by the text engine, and a painter that draws them as Chromium does
//! (pixel-snapped boxes, borders and rings, gradients in OKLab). Every screen builds one tree per frame
//! that mirrors the DOM of the player's component, so the layout dump can list the same text lines and
//! painted boxes the page dump lists.

use std::rc::Rc;

use skia_safe::gradient::{Colors as GradColors, Gradient, Interpolation, interpolation};
use skia_safe::{
    BlurStyle, Canvas, ClipOp, MaskFilter, Matrix, Paint, PathBuilder, Point, RRect, Rect,
    TileMode, canvas::SaveLayerRec, color_filters,
};
use taffy::geometry::{Point as TPoint, Rect as TRect};
use taffy::prelude::{
    AlignContent, AlignItems, AlignSelf, AvailableSpace, Display, FlexDirection, FlexWrap,
    GridTemplateComponent, JustifyContent, LengthPercentage, LengthPercentageAuto, NodeId,
    Position, Size, Style, TaffyTree, TrackSizingFunction, length, percent,
};
use taffy::style::Overflow;

use crate::art::Art;
use crate::color::Rgba;
use crate::text::{TextEngine, TextSpec};

pub type Tree = TaffyTree<Ctx>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fill {
    Solid(Rgba),
    /// `linear-gradient(to bottom right in oklab, from, to)` (Tailwind's `bg-linear-to-br`).
    ToBottomRight(Rgba, Rgba),
}

/// `translate(tx, ty) rotate(deg) scale(sx, sy)` about the centre, or the bottom edge's middle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xform {
    pub tx: f32,
    pub ty: f32,
    pub rot: f32,
    pub sx: f32,
    pub sy: f32,
    pub from_bottom: bool,
    /// Skew along x, in degrees (the shine's sweep).
    pub skew_x: f32,
    /// A further translation in widths of the box itself (`translateX(420%)`).
    pub tx_w: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Visual {
    pub bg: Option<Fill>,
    pub border: Option<(f32, Rgba)>,
    pub radius: f32,
    /// `ring-N`: a box-shadow of that spread around the box.
    pub ring: Option<(f32, Rgba)>,
    /// `inset-ring-N`: the same inside the box.
    pub inset_ring: Option<(f32, Rgba)>,
    /// The `::before` overlay of an option tile: white from the top, fading out.
    pub tile_overlay: bool,
    /// An inset blurred shadow (`shadow-[inset_0_0_90px_12px_...]`): blur, spread, colour.
    pub glow: Option<(f32, f32, Rgba)>,
    pub clip: bool,
    /// `rounded-t-*`: the bottom corners are square.
    pub flat_bottom: bool,
    pub opacity: Option<f32>,
    pub grayscale: bool,
    /// A drop shadow: offset x, offset y, blur, spread, colour (`shadow-2xl`).
    pub drop_shadow: Option<(f32, f32, f32, f32, Rgba)>,
    /// A motion transform around the box's centre (or its bottom edge): the podium's rise, pop and confetti.
    pub xform: Option<Xform>,
    pub z: i32,
    /// An `overflow-y: auto` box: the cut that keeps half rows out of the video looks at it.
    pub scroll: bool,
    /// The text of this box is centred vertically in it (a flex box with `items-center` around a text run).
    pub center_y: bool,
    /// `transform: scaleX(n)` from the left edge (the bars inside a tile).
    pub scale_x: Option<f32>,
    /// `visibility: hidden`: laid out, never painted, never listed.
    pub invisible: bool,
    /// An absolutely positioned box whose containing block is the root, not its parent: Taffy reports its
    /// offset in the containing block's own coordinates.
    pub from_root: bool,
    /// The DOM tag, for the layout dump and for the rows of a list.
    pub tag: &'static str,
}

impl Xform {
    pub const fn new(tx: f32, ty: f32, rot: f32, sx: f32, sy: f32) -> Self {
        Self {
            tx,
            ty,
            rot,
            sx,
            sy,
            from_bottom: false,
            skew_x: 0.0,
            tx_w: 0.0,
        }
    }
}

impl Visual {
    pub fn bg(mut self, c: Rgba) -> Self {
        self.bg = Some(Fill::Solid(c));
        self
    }
    pub fn grad(mut self, from: Rgba, to: Rgba) -> Self {
        self.bg = Some(Fill::ToBottomRight(from, to));
        self
    }
    pub fn border(mut self, w: f32, c: Rgba) -> Self {
        self.border = Some((w, c));
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }
    pub fn ring(mut self, w: f32, c: Rgba) -> Self {
        self.ring = Some((w, c));
        self
    }
    pub fn inset_ring(mut self, w: f32, c: Rgba) -> Self {
        self.inset_ring = Some((w, c));
        self
    }
    pub fn clipped(mut self) -> Self {
        self.clip = true;
        self
    }
    pub fn scroll(mut self) -> Self {
        self.clip = true;
        self.scroll = true;
        self
    }
    pub fn vcenter(mut self) -> Self {
        self.center_y = true;
        self
    }
    pub fn hidden(mut self) -> Self {
        self.invisible = true;
        self
    }
    pub fn z(mut self, z: i32) -> Self {
        self.z = z;
        self
    }

    /// Whether the page's dump lists this box (it paints something).
    fn painted(&self, content: &Content) -> bool {
        self.bg.is_some()
            || self.border.is_some()
            || self.ring.is_some()
            || self.inset_ring.is_some()
            || self.glow.is_some()
            || matches!(content, Content::Art { .. } | Content::Ring { .. })
    }
}

#[derive(Debug, Clone)]
pub enum Content {
    None,
    Text(TextSpec),
    /// An SVG picture with its `currentColor`.
    Art {
        art: Rc<Art>,
        tint: Rgba,
        fill: Option<Rgba>,
        stroke_width: Option<f32>,
    },
    /// The countdown ring (`quiz/components/ring.svelte`): a 100x100 viewBox, track and arc.
    Ring {
        fraction: f32,
        color: Rgba,
    },
}

#[derive(Debug, Clone)]
pub struct Ctx {
    pub vis: Visual,
    pub content: Content,
}

/// What a built tree is for: the layout the page measures (a tall viewport) or the one the picture
/// is made in (the stage's own height). Height-based media queries read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Env {
    pub viewport_h: u32,
    /// The tree the page measures (the cut), not the one the picture is made from.
    pub page: bool,
}

impl Env {
    pub const fn min_h(self, px: u32) -> bool {
        self.viewport_h >= px
    }
    pub const fn max_h(self, px: u32) -> bool {
        self.viewport_h <= px
    }
}

// ───────────────────────── style builder ─────────────────────────

#[derive(Debug, Clone, Default)]
pub struct S(pub Style);

fn lp(v: f32) -> LengthPercentage {
    length(v)
}

impl S {
    pub fn block() -> Self {
        Self(Style {
            display: Display::Block,
            ..Default::default()
        })
    }
    pub fn row() -> Self {
        Self(Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            ..Default::default()
        })
    }
    pub fn col() -> Self {
        Self(Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            ..Default::default()
        })
    }
    pub fn grid() -> Self {
        Self(Style {
            display: Display::Grid,
            ..Default::default()
        })
    }
    pub fn none() -> Self {
        Self(Style {
            display: Display::None,
            ..Default::default()
        })
    }
    pub fn w(mut self, v: f32) -> Self {
        self.0.size.width = length(v);
        self
    }
    pub fn h(mut self, v: f32) -> Self {
        self.0.size.height = length(v);
        self
    }
    pub fn size(self, v: f32) -> Self {
        self.w(v).h(v)
    }
    pub fn w_pct(mut self, v: f32) -> Self {
        self.0.size.width = percent(v);
        self
    }
    pub fn full(mut self) -> Self {
        self.0.size = Size {
            width: percent(1.0),
            height: percent(1.0),
        };
        self
    }
    pub fn w_full(mut self) -> Self {
        self.0.size.width = percent(1.0);
        self
    }
    pub fn min_h(mut self, v: f32) -> Self {
        self.0.min_size.height = length(v);
        self
    }
    pub fn min_w(mut self, v: f32) -> Self {
        self.0.min_size.width = length(v);
        self
    }
    pub fn max_w(mut self, v: f32) -> Self {
        self.0.max_size.width = length(v);
        self
    }
    pub fn max_w_pct(mut self, v: f32) -> Self {
        self.0.max_size.width = percent(v);
        self
    }
    /// `min-w-0 min-h-0`: the item may shrink below its content.
    pub fn min0(mut self) -> Self {
        self.0.min_size = Size {
            width: length(0.0),
            height: length(0.0),
        };
        self
    }
    pub fn gap(mut self, v: f32) -> Self {
        self.0.gap = Size {
            width: lp(v),
            height: lp(v),
        };
        self
    }
    pub fn gap_xy(mut self, x: f32, y: f32) -> Self {
        self.0.gap = Size {
            width: lp(x),
            height: lp(y),
        };
        self
    }
    pub fn p(mut self, v: f32) -> Self {
        self.0.padding = TRect {
            left: lp(v),
            right: lp(v),
            top: lp(v),
            bottom: lp(v),
        };
        self
    }
    pub fn pad(mut self, l: f32, t: f32, r: f32, b: f32) -> Self {
        self.0.padding = TRect {
            left: lp(l),
            right: lp(r),
            top: lp(t),
            bottom: lp(b),
        };
        self
    }
    pub fn px_py(self, x: f32, y: f32) -> Self {
        self.pad(x, y, x, y)
    }
    pub fn border(mut self, v: f32) -> Self {
        self.0.border = TRect {
            left: lp(v),
            right: lp(v),
            top: lp(v),
            bottom: lp(v),
        };
        self
    }
    pub fn grow(mut self, v: f32) -> Self {
        self.0.flex_grow = v;
        self
    }
    /// `flex-1`: grow 1, shrink 1, basis 0.
    pub fn flex1(mut self) -> Self {
        self.0.flex_grow = 1.0;
        self.0.flex_shrink = 1.0;
        self.0.flex_basis = length(0.0);
        self
    }
    pub fn shrink0(mut self) -> Self {
        self.0.flex_shrink = 0.0;
        self
    }
    pub fn items(mut self, a: AlignItems) -> Self {
        self.0.align_items = a;
        self
    }
    pub fn self_(mut self, a: AlignSelf) -> Self {
        self.0.align_self = Some(a);
        self
    }
    pub fn justify(mut self, a: JustifyContent) -> Self {
        self.0.justify_content = a;
        self
    }
    pub fn content(mut self, a: AlignContent) -> Self {
        self.0.align_content = a;
        self
    }
    pub fn wrap(mut self) -> Self {
        self.0.flex_wrap = FlexWrap::Wrap;
        self
    }
    /// `overflow: hidden` or `auto` (no scrollbar is drawn).
    pub fn clip(mut self) -> Self {
        self.0.overflow = TPoint {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
        };
        self
    }
    /// `position: relative`: the box the absolutely positioned boxes inside it are placed in.
    pub fn relative(mut self) -> Self {
        self.0.position = Position::Relative;
        self
    }
    /// Positioned absolutely, inset by these (`None` is `auto`).
    pub fn abs(mut self, l: Option<f32>, t: Option<f32>, r: Option<f32>, b: Option<f32>) -> Self {
        self.0.position = Position::Absolute;
        let a = |v: Option<f32>| v.map_or_else(LengthPercentageAuto::auto, length);
        self.0.inset = TRect {
            left: a(l),
            top: a(t),
            right: a(r),
            bottom: a(b),
        };
        self
    }
    pub fn m_auto(mut self) -> Self {
        self.0.margin = TRect {
            left: LengthPercentageAuto::auto(),
            right: LengthPercentageAuto::auto(),
            top: LengthPercentageAuto::auto(),
            bottom: LengthPercentageAuto::auto(),
        };
        self
    }
    pub fn mx_auto(mut self) -> Self {
        self.0.margin.left = LengthPercentageAuto::auto();
        self.0.margin.right = LengthPercentageAuto::auto();
        self
    }
    pub fn aspect(mut self, r: f32) -> Self {
        self.0.aspect_ratio = Some(r);
        self
    }
    pub fn cols(mut self, tracks: Vec<GridTemplateComponent<String>>) -> Self {
        self.0.grid_template_columns = tracks;
        self
    }
    pub fn auto_rows(mut self, rows: Vec<TrackSizingFunction>) -> Self {
        self.0.grid_auto_rows = rows;
        self
    }
    pub fn align_start(mut self) -> Self {
        self.0.align_content = AlignContent::START;
        self
    }
}

/// Builds the tree.
#[derive(Debug)]
pub struct Builder {
    pub tree: Tree,
    /// Every node in the order it was made (the DOM order), so two trees of one screen correspond.
    pub order: Vec<NodeId>,
    pub text: Rc<TextEngine>,
    pub env: Env,
}

impl Builder {
    pub fn new(text: Rc<TextEngine>, env: Env) -> Self {
        let mut tree = Tree::new();
        tree.disable_rounding();
        Self {
            tree,
            order: Vec::new(),
            text,
            env,
        }
    }

    pub fn node(&mut self, s: S, vis: Visual, content: Content, kids: &[NodeId]) -> NodeId {
        let ctx = Ctx { vis, content };
        let id = if kids.is_empty() {
            self.tree.new_leaf_with_context(s.0, ctx).expect("leaf")
        } else {
            let id = self.tree.new_with_children(s.0, kids).expect("node");
            self.tree.set_node_context(id, Some(ctx)).expect("context");
            id
        };
        self.order.push(id);
        id
    }

    pub fn div(&mut self, tag: &'static str, s: S, vis: Visual, kids: &[NodeId]) -> NodeId {
        self.node(s, Visual { tag, ..vis }, Content::None, kids)
    }

    pub fn text(&mut self, tag: &'static str, s: S, vis: Visual, spec: TextSpec) -> NodeId {
        self.node(s, Visual { tag, ..vis }, Content::Text(spec), &[])
    }

    pub fn art(
        &mut self,
        tag: &'static str,
        s: S,
        vis: Visual,
        art: Rc<Art>,
        tint: Rgba,
    ) -> NodeId {
        self.node(
            s,
            Visual { tag, ..vis },
            Content::Art {
                art,
                tint,
                fill: None,
                stroke_width: None,
            },
            &[],
        )
    }
}

// ───────────────────────── layout ─────────────────────────

/// Lays the tree out in a box of `w` x `h`.
pub fn compute(tree: &mut Tree, root: NodeId, text: &TextEngine, w: f32, h: f32) {
    tree.compute_layout_with_measure(
        root,
        Size {
            width: AvailableSpace::Definite(w),
            height: AvailableSpace::Definite(h),
        },
        |inputs, _id, ctx, style| {
            taffy::compute_leaf_layout(
                inputs,
                style,
                |_, _| 0.0,
                |known, avail| measure(text, known, avail, ctx.as_deref()),
            )
        },
    )
    .expect("layout");
}

/// Chromium's layout units are 1/64 px and a text's width is rounded up to one.
fn ceil64(v: f32) -> f32 {
    (v * 64.0).ceil() / 64.0
}

fn measure(
    text: &TextEngine,
    known: Size<Option<f32>>,
    avail: Size<AvailableSpace>,
    ctx: Option<&Ctx>,
) -> Size<f32> {
    let Some(Ctx {
        content: Content::Text(spec),
        ..
    }) = ctx
    else {
        return Size {
            width: known.width.unwrap_or(0.0),
            height: known.height.unwrap_or(0.0),
        };
    };
    if let (Some(width), Some(height)) = (known.width, known.height) {
        return Size { width, height };
    }
    let (max, min) = text.intrinsic(spec);
    let one_line = text.laid(spec, 1.0e6);
    let width = match (known.width, avail.width) {
        (Some(w), _) => w,
        (None, AvailableSpace::Definite(w)) => ceil64(max).min(w.max(0.0)),
        (None, AvailableSpace::MaxContent) => ceil64(max),
        (None, AvailableSpace::MinContent) => ceil64(min),
    };
    let height = known.height.unwrap_or_else(|| {
        if spec.max_lines == Some(1) {
            one_line.height
        } else {
            text.laid(spec, width.max(1.0)).height
        }
    });
    Size { width, height }
}

// ───────────────────────── painting ─────────────────────────

fn snap(r: f32) -> f32 {
    r.round()
}

/// A box's rect as Chromium paints it: each edge rounded to a whole pixel.
fn snapped(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(snap(x), snap(y), snap(x + w), snap(y + h))
}

fn rrect(r: Rect, radius: f32) -> RRect {
    // `rounded-full` is a huge radius: it is clamped to half the shorter side.
    let k = radius.min(r.width() / 2.0).min(r.height() / 2.0);
    RRect::new_rect_xy(r, k, k)
}

fn box_shape(r: Rect, vis: &Visual) -> RRect {
    if !vis.flat_bottom {
        return rrect(r, vis.radius);
    }
    let k = skia_safe::Point::new(vis.radius, vis.radius);
    let z = skia_safe::Point::new(0.0, 0.0);
    RRect::new_rect_radii(r, &[k, k, z, z])
}

fn fill_paint(c: Rgba, opacity: f32) -> Paint {
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_color(c.skia(opacity));
    p
}

fn gradient_paint(r: Rect, from: Rgba, to: Rgba) -> Paint {
    let (w, h) = (r.width(), r.height());
    let len = 2.0 * w * h / (w * w + h * h).sqrt();
    let norm = (w * w + h * h).sqrt();
    let (dx, dy) = (h / norm, w / norm);
    let c = Point::new(r.center_x(), r.center_y());
    let p0 = Point::new(c.x - dx * len / 2.0, c.y - dy * len / 2.0);
    let p1 = Point::new(c.x + dx * len / 2.0, c.y + dy * len / 2.0);
    gradient(p0, p1, &[from, to], &[0.0, 1.0])
}

/// A linear gradient interpolated in OKLab, as Tailwind's `bg-linear-*` asks for.
fn gradient(p0: Point, p1: Point, colors: &[Rgba], pos: &[f32]) -> Paint {
    let cols: Vec<skia_safe::Color4f> = colors
        .iter()
        .map(|c| {
            skia_safe::Color4f::new(
                f32::from(c.r) / 255.0,
                f32::from(c.g) / 255.0,
                f32::from(c.b) / 255.0,
                c.a,
            )
        })
        .collect();
    let interp = Interpolation {
        color_space: interpolation::ColorSpace::OKLab,
        in_premul: interpolation::InPremul::Yes,
        hue_method: interpolation::HueMethod::Shorter,
    };
    let grad = Gradient::new(
        GradColors::new(&cols, Some(pos), TileMode::Clamp, None),
        interp,
    );
    let mut p = Paint::default();
    p.set_anti_alias(true);
    p.set_shader(skia_safe::shaders::linear_gradient((p0, p1), &grad, None));
    p
}

/// What the layout dump of the painted tree says (the fields the page's dump writes).
#[derive(Debug, Default, Clone)]
pub struct Dump {
    pub texts: Vec<serde_json::Value>,
    pub boxes: Vec<serde_json::Value>,
    pub missing: usize,
}

#[derive(Debug)]
pub struct Painter<'a> {
    pub tree: &'a Tree,
    pub text: &'a TextEngine,
    /// Nodes the cut has hidden (`display: none`), by id.
    pub hidden: &'a std::collections::HashSet<NodeId>,
    /// Where the root is drawn (set by `paint` and `dump`).
    pub origin: std::cell::Cell<(f32, f32)>,
}

impl Painter<'_> {
    pub fn paint(&self, canvas: &Canvas, root: NodeId, x: f32, y: f32) {
        self.origin.set((x, y));
        self.node(canvas, root, x, y, &Matrix::new_identity(), None);
    }

    pub fn dump(&self, root: NodeId, x: f32, y: f32) -> Dump {
        self.origin.set((x, y));
        let mut d = Dump::default();
        let mut surface = skia_safe::surfaces::raster_n32_premul((1, 1)).expect("surface");
        self.node(
            surface.canvas(),
            root,
            x,
            y,
            &Matrix::new_identity(),
            Some(&mut d),
        );
        d
    }

    fn node(
        &self,
        canvas: &Canvas,
        id: NodeId,
        ox: f32,
        oy: f32,
        outer: &Matrix,
        mut dump: Option<&mut Dump>,
    ) {
        if self.hidden.contains(&id) {
            return;
        }
        let layout = self.tree.layout(id).expect("layout");
        let style = self.tree.style(id).expect("style");
        if style.display == Display::None {
            return;
        }
        let ctx = self.tree.get_node_context(id).cloned().unwrap_or(Ctx {
            vis: Visual::default(),
            content: Content::None,
        });
        let vis = &ctx.vis;
        let opacity = vis.opacity.unwrap_or(1.0);
        if opacity == 0.0 || vis.invisible {
            return;
        }
        let (x, y) = if vis.from_root {
            (
                self.origin.get().0 + layout.location.x,
                self.origin.get().1 + layout.location.y,
            )
        } else {
            (ox + layout.location.x, oy + layout.location.y)
        };
        let (mut w, h) = (layout.size.width, layout.size.height);
        if let Some(k) = vis.scale_x {
            w *= k;
        }
        let rect = snapped(x, y, w, h);
        // This box's transform (a motion), and everything inside it moves with it.
        let local = vis.xform.map(|t| xform_matrix(&t, &rect));
        let matrix = local
            .as_ref()
            .map_or_else(|| *outer, |l| Matrix::concat(outer, l));
        if let Some(d) = dump.as_deref_mut()
            && vis.painted(&ctx.content)
            && w > 0.0
            && h > 0.0
        {
            let (r, _) = matrix.map_rect(Rect::from_xywh(x, y, w, h));
            // A box squeezed to nothing by its transform has no box in the page.
            if r.width() > 0.0 && r.height() > 0.0 {
                d.boxes.push(serde_json::json!({
                    "kind": vis.tag, "x": round2(r.left), "y": round2(r.top),
                    "w": round2(r.width()), "h": round2(r.height())
                }));
            }
        }
        let live = dump.is_none();
        let moved = live && local.is_some();
        if let (true, Some(l)) = (moved, local.as_ref()) {
            canvas.save();
            canvas.concat(l);
        }
        let layer = live && (opacity < 1.0 || vis.grayscale);
        if layer {
            let mut p = Paint::default();
            p.set_alpha_f(opacity);
            if vis.grayscale {
                p.set_color_filter(color_filters::matrix_row_major(
                    &[
                        0.2126, 0.7152, 0.0722, 0.0, 0.0, 0.2126, 0.7152, 0.0722, 0.0, 0.0, 0.2126,
                        0.7152, 0.0722, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
                    ],
                    None,
                ));
            }
            canvas.save_layer(
                &SaveLayerRec::default()
                    .bounds(&rect.with_outset({
                        // Room for what is painted outside the box: its ring and its shadow.
                        let reach = vis.ring.map_or(0.0, |r| r.0).max(vis.drop_shadow.map_or(
                            0.0,
                            |(dx, dy, blur, spread, _)| {
                                dx.abs().max(dy.abs()) + blur * 1.5 + spread.max(0.0)
                            },
                        ));
                        (reach, reach)
                    }))
                    .paint(&p),
            );
        }
        let shape = box_shape(rect, vis);
        if live {
            self.paint_box(canvas, &ctx, shape, rect);
        }
        match &ctx.content {
            Content::Text(spec) => self.paint_text(
                canvas,
                id,
                spec,
                (x, y, w, h),
                (vis.center_y, live, &matrix),
                dump.as_deref_mut(),
            ),
            Content::Art {
                art,
                tint,
                fill,
                stroke_width,
            } if live => art.draw_with(canvas, rect, *tint, *fill, *stroke_width),
            Content::Ring { fraction, color } if live => {
                crate::art::draw_ring(canvas, rect, *fraction, *color)
            }
            _ => {}
        }
        let clip = live && vis.clip;
        if clip {
            canvas.save();
            let bw = vis.border.map_or(0.0, |b| b.0);
            let inner = RRect::new_rect_xy(
                rect.with_inset((bw, bw)),
                (vis.radius - bw)
                    .max(0.0)
                    .min(rect.width() / 2.0)
                    .min(rect.height() / 2.0),
                (vis.radius - bw)
                    .max(0.0)
                    .min(rect.width() / 2.0)
                    .min(rect.height() / 2.0),
            );
            canvas.clip_rrect(inner, ClipOp::Intersect, true);
        }
        let mut kids = self.tree.children(id).expect("children");
        if live {
            kids.sort_by_key(|k| self.tree.get_node_context(*k).map_or(0, |c| c.vis.z));
        }
        for kid in kids {
            self.node(canvas, kid, x, y, &matrix, dump.as_deref_mut());
        }
        if clip {
            canvas.restore();
        }
        if layer {
            canvas.restore();
        }
        if moved {
            canvas.restore();
        }
    }

    fn paint_box(&self, canvas: &Canvas, ctx: &Ctx, shape: RRect, rect: Rect) {
        let vis = &ctx.vis;
        if let Some((dx, dy, blur, spread, c)) = vis.drop_shadow {
            let mut p = fill_paint(c, 1.0);
            p.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, blur / 2.0, true));
            let r = rect.with_outset((spread, spread)).with_offset((dx, dy));
            canvas.draw_rrect(rrect(r, vis.radius + spread), &p);
        }
        if let Some((w, c)) = vis.ring {
            let outer = rrect(rect.with_outset((w, w)), vis.radius + w);
            canvas.draw_drrect(outer, shape, &fill_paint(c, 1.0));
        }
        match vis.bg {
            Some(Fill::Solid(c)) => {
                canvas.draw_rrect(shape, &fill_paint(c, 1.0));
            }
            Some(Fill::ToBottomRight(a, b)) => {
                canvas.draw_rrect(shape, &gradient_paint(rect, a, b));
            }
            None => {}
        }
        if let Some((w, c)) = vis.inset_ring {
            let inner = rrect(rect.with_inset((w, w)), (vis.radius - w).max(0.0));
            canvas.draw_drrect(shape, inner, &fill_paint(c, 1.0));
        }
        if vis.tile_overlay {
            canvas.save();
            canvas.clip_rrect(shape, ClipOp::Intersect, true);
            let white = Rgba::WHITE;
            let p = gradient(
                Point::new(rect.left, rect.top),
                Point::new(rect.left, rect.bottom),
                &[
                    white.with_alpha(0.2),
                    white.with_alpha(0.05),
                    white.with_alpha(0.0),
                ],
                &[0.0, 0.5, 1.0],
            );
            canvas.draw_rect(rect, &p);
            canvas.restore();
        }
        if let Some((blur, spread, c)) = vis.glow {
            canvas.save();
            canvas.clip_rrect(shape, ClipOp::Intersect, true);
            let inner = rect.with_inset((spread, spread));
            let mut path = PathBuilder::new();
            path.add_rect(rect.with_outset((blur * 2.0, blur * 2.0)), None, None);
            path.add_rect(inner, None, None);
            path.set_fill_type(skia_safe::PathFillType::EvenOdd);
            let mut p = fill_paint(c, 1.0);
            p.set_mask_filter(MaskFilter::blur(BlurStyle::Normal, blur / 2.0, true));
            canvas.draw_path(&path.detach(), &p);
            canvas.restore();
        }
        if let Some((w, c)) = vis.border {
            let inner = rrect(rect.with_inset((w, w)), (vis.radius - w).max(0.0));
            canvas.draw_drrect(shape, inner, &fill_paint(c, 1.0));
        }
    }

    fn paint_text(
        &self,
        canvas: &Canvas,
        id: NodeId,
        spec: &TextSpec,
        (x, y, w, h): (f32, f32, f32, f32),
        (center_y, live, matrix): (bool, bool, &Matrix),
        dump: Option<&mut Dump>,
    ) {
        let style = self.tree.style(id).expect("style");
        let pad_l = resolve(style.padding.left);
        let pad_t = resolve(style.padding.top);
        let bl = resolve(style.border.left);
        let bt = resolve(style.border.top);
        let pr = resolve(style.padding.right) + resolve(style.border.right);
        let pb = resolve(style.padding.bottom) + resolve(style.border.bottom);
        let width = (w - pad_l - bl - pr).max(0.0);
        let laid = self.text.laid(spec, width.max(1.0));
        let (tx, mut ty) = (x + pad_l + bl, y + pad_t + bt);
        if center_y {
            ty += ((h - pad_t - bt - pb - laid.height) / 2.0).max(0.0);
        }
        if live {
            laid.paragraph.paint(canvas, (tx, ty + laid.shift));
        }
        if let Some(d) = dump {
            let from = d.texts.len();
            crate::dump::text_lines(self.text, spec, &laid, tx, ty, width, d);
            // Lines of a box in motion are where it moved them (the page reports the transformed boxes).
            if !matrix.is_identity() {
                for line in &mut d.texts[from..] {
                    let f = |k: &str| line[k].as_f64().unwrap_or(0.0) as f32;
                    let (r, _) = matrix.map_rect(Rect::from_xywh(f("x"), f("y"), f("w"), f("h")));
                    for (k, v) in [
                        ("x", r.left),
                        ("y", r.top),
                        ("w", r.width()),
                        ("h", r.height()),
                    ] {
                        line[k] = serde_json::json!(round2(v));
                    }
                }
            }
        }
    }
}

/// `translate rotate scale skew` of an `Xform` about its box's centre or bottom edge.
fn xform_matrix(t: &Xform, rect: &Rect) -> Matrix {
    let pivot = (
        rect.center_x(),
        if t.from_bottom {
            rect.bottom
        } else {
            rect.center_y()
        },
    );
    let mut m = Matrix::translate((t.tx + t.tx_w * rect.width(), t.ty));
    m.pre_translate(pivot);
    m.pre_rotate(t.rot, None);
    m.pre_scale((t.sx, t.sy), None);
    if t.skew_x != 0.0 {
        m.pre_skew((t.skew_x.to_radians().tan(), 0.0), None);
    }
    m.pre_translate((-pivot.0, -pivot.1));
    m
}

fn resolve(v: LengthPercentage) -> f32 {
    // Only fixed lengths are used for padding and borders.
    let raw = v.into_raw();
    if raw.tag() == taffy::style::CompactLength::LENGTH_TAG {
        raw.value()
    } else {
        0.0
    }
}

fn round2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}
