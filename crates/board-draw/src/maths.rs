//! Graphs, solids and symbols on the board: the shared helpers of `board/math/canvas.ts`, and the entry
//! that draws a pack element. The drawings themselves are in [`crate::graph`] and
//! [`crate::solid`], each a call-for-call port of the web player's.

use replay_model::element::{BoardElement, Pack};
use replay_model::pack::PackBody;

use crate::canvas::{Canvas, LineCap, LineJoin, Path, Rgba, TextAlign, TextBaseline};
use crate::render::{BoardTheme, ink};

/// How a drawing inks itself: the board's colours for the theme.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Style {
    pub theme: BoardTheme,
}

impl Style {
    pub fn dark(self) -> bool {
        self.theme == BoardTheme::Dark
    }

    pub fn ink(self, color: replay_model::ColorToken) -> Rgba {
        ink(color, self.theme)
    }

    /// Axes, grid and labels.
    pub fn muted(self) -> Rgba {
        if self.dark() {
            Rgba::hex(0xa1a1aa)
        } else {
            Rgba::hex(0x475569)
        }
    }

    /// The board's colour, behind lettering and round the edge of a dot.
    pub fn halo(self) -> Rgba {
        if self.dark() {
            Rgba::hex(0x18181b)
        } else {
            Rgba::hex(0xffffff)
        }
    }
}

/// Draws a graph, a solid or a symbol as the board does (after it has set the colour, width, cap and join of
/// the element).
pub fn draw_pack<C: Canvas>(ctx: &mut C, el: &BoardElement, pack: &Pack, theme: BoardTheme) {
    let style = Style { theme };
    match &pack.body {
        PackBody::Graph(g) => crate::graph::draw_graph(ctx, g, el.size, style),
        PackBody::Solid(s) => crate::solid::draw_solid(ctx, s, el, style),
        PackBody::Symbol(s) => crate::symbol::draw_symbol(ctx, s, el, style),
    }
}

/// Text with a halo in the board's colour, so a line passing behind it does not cut it.
pub(crate) fn halo_text<C: Canvas>(ctx: &mut C, text: &str, x: f64, y: f64, style: Style) {
    ctx.save();
    ctx.set_line_join(LineJoin::Round);
    ctx.set_line_width(4.0);
    ctx.set_stroke_style(style.halo());
    ctx.stroke_text(text, x, y);
    ctx.restore();
    ctx.fill_text(text, x, y);
}

pub(crate) fn dot<C: Canvas>(ctx: &mut C, x: f64, y: f64, r: f64, color: Rgba, style: Style) {
    let mut path = Path::new();
    path.arc(x, y, r, 0.0, std::f64::consts::TAU, false);
    ctx.set_fill_style(color);
    ctx.fill(&path);
    ctx.set_line_width(1.5);
    ctx.set_stroke_style(style.halo());
    ctx.stroke(&path);
}

/// A polyline of flat `[x, y, x, y...]` points added to `path` (nothing for fewer than two).
pub(crate) fn polyline(path: &mut Path, points: &[f64]) {
    if points.len() < 4 {
        return;
    }
    path.move_to(points[0], points[1]);
    for p in points[2..].as_chunks::<2>().0 {
        path.line_to(p[0], p[1]);
    }
}

pub(crate) fn align<C: Canvas>(ctx: &mut C, align: TextAlign, baseline: TextBaseline) {
    ctx.set_text_align(align);
    ctx.set_text_baseline(baseline);
}

pub(crate) fn round_ends<C: Canvas>(ctx: &mut C) {
    ctx.set_line_join(LineJoin::Round);
    ctx.set_line_cap(LineCap::Round);
}

/// Strokes a drawing's solid lines, then its dashed ones (thinner, dash after the stroke width).
pub(crate) fn stroke_runs<C: Canvas>(ctx: &mut C, solid: &Path, dashed: &Path, width: f64) {
    ctx.set_line_width(width);
    ctx.stroke(solid);
    ctx.set_line_dash(&[width * 3.0, width * 2.4]);
    ctx.set_line_width(1.2f64.max(width * 0.8));
    ctx.stroke(dashed);
    ctx.set_line_dash(&[]);
}
