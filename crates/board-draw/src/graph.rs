//! Drawing a graph: grid, axes with ticks, and the curves sampled from their formulas. A port of
//! the drawing half of `board/math/graph-draw.ts`; the numbers (ticks, samples, marks, shading)
//! are `replay_model::pack::sampling`.

use replay_model::pack::format::format_number;
use replay_model::pack::graph::{Graph, PlotSpace};
use replay_model::pack::sampling::{Geometry, Run, Shade, axis_ticks, geometry, sample_count};

use crate::canvas::{Canvas, Path, Rgba, TextAlign, TextBaseline};
use crate::maths::{Style, align, dot, halo_text, polyline, round_ends};

/// Below this on-screen width a graph is only drawn as its frame.
const MIN_DRAWN_PX: f64 = 24.0;
const TICK_LABEL: f64 = 13.0;

/// Where an axis sits: through the origin when visible, else on the nearest edge.
fn axis_position(origin: f64, lo: f64, hi: f64) -> f64 {
    hi.min(lo.max(origin))
}

fn runs_path(runs: &[Run]) -> Path {
    let mut path = Path::new();
    for run in runs {
        polyline(&mut path, run);
    }
    path
}

fn shade_path(shade: &Shade) -> Path {
    let mut path = Path::new();
    for polygon in &shade.polygons {
        polyline(&mut path, polygon);
        path.close_path();
    }
    path
}

fn draw_axes<C: Canvas>(ctx: &mut C, g: &Graph, s: &PlotSpace, size: f64, style: Style) {
    let hair = 1.25 / ctx.scale();
    let view = g.view;
    let x_ticks = axis_ticks(view.xmin, view.xmax, s.w, g.pi_ticks);
    let y_ticks = axis_ticks(view.ymin, view.ymax, s.h, false);
    let axis_y = axis_position(s.py(0.0), s.y, s.y + s.h);
    let axis_x = axis_position(s.px(0.0), s.x, s.x + s.w);

    ctx.set_line_width(hair);
    if g.grid {
        ctx.set_stroke_style(if style.dark() {
            Rgba::with_alpha(226, 232, 240, 0.14)
        } else {
            Rgba::with_alpha(15, 23, 42, 0.12)
        });
        let mut grid = Path::new();
        for t in &x_ticks {
            grid.move_to(s.px(t.value), s.y);
            grid.line_to(s.px(t.value), s.y + s.h);
        }
        for t in &y_ticks {
            grid.move_to(s.x, s.py(t.value));
            grid.line_to(s.x + s.w, s.py(t.value));
        }
        ctx.stroke(&grid);
    }
    if !g.axes {
        return;
    }

    ctx.set_stroke_style(style.muted());
    ctx.set_fill_style(style.muted());
    ctx.set_line_width(1.5f64.max(size * 0.5));
    let head = 9.0;
    let mut arrows = Path::new();
    arrows.move_to(s.x, axis_y);
    arrows.line_to(s.x + s.w - 1.0, axis_y);
    arrows.move_to(s.x + s.w - head, axis_y - head / 2.2);
    arrows.line_to(s.x + s.w - 1.0, axis_y);
    arrows.line_to(s.x + s.w - head, axis_y + head / 2.2);
    arrows.move_to(axis_x, s.y + s.h);
    arrows.line_to(axis_x, s.y + 1.0);
    arrows.move_to(axis_x - head / 2.2, s.y + head);
    arrows.line_to(axis_x, s.y + 1.0);
    arrows.line_to(axis_x + head / 2.2, s.y + head);
    ctx.stroke(&arrows);

    ctx.set_font(false, 400, TICK_LABEL);
    ctx.set_line_width(hair * 1.4);
    let mut ticks = Path::new();
    align(ctx, TextAlign::Center, TextBaseline::Top);
    let labels_below = axis_y < s.y + s.h - TICK_LABEL - 8.0;
    for t in &x_ticks {
        let px = s.px(t.value);
        ticks.move_to(px, axis_y - 3.0);
        ticks.line_to(px, axis_y + 3.0);
        if t.value == 0.0 {
            continue;
        }
        if px < s.x + 10.0 || px > s.x + s.w - 14.0 {
            continue;
        }
        let y = if labels_below {
            axis_y + 6.0
        } else {
            axis_y - 6.0 - TICK_LABEL
        };
        halo_text(ctx, &t.label, px, y, style);
    }
    let labels_left = axis_x > s.x + 34.0;
    align(
        ctx,
        if labels_left {
            TextAlign::Right
        } else {
            TextAlign::Left
        },
        TextBaseline::Middle,
    );
    for t in &y_ticks {
        let py = s.py(t.value);
        ticks.move_to(axis_x - 3.0, py);
        ticks.line_to(axis_x + 3.0, py);
        if t.value == 0.0 {
            continue;
        }
        if py < s.y + 12.0 || py > s.y + s.h - 6.0 {
            continue;
        }
        let x = if labels_left {
            axis_x - 7.0
        } else {
            axis_x + 7.0
        };
        halo_text(ctx, &t.label, x, py, style);
    }
    ctx.stroke(&ticks);
    if axis_x >= s.x && axis_y >= s.y {
        align(
            ctx,
            if labels_left {
                TextAlign::Right
            } else {
                TextAlign::Left
            },
            TextBaseline::Top,
        );
        if view.xmin < 0.0 && view.xmax > 0.0 && view.ymin < 0.0 && view.ymax > 0.0 {
            let x = if labels_left {
                axis_x - 6.0
            } else {
                axis_x + 6.0
            };
            halo_text(ctx, "O", x, axis_y + 4.0, style);
        }
    }
    // Axis names at the arrow tips.
    ctx.set_font(true, 400, TICK_LABEL + 1.0);
    align(ctx, TextAlign::Right, TextBaseline::Bottom);
    halo_text(ctx, "x", s.x + s.w - 4.0, axis_y - 8.0, style);
    align(ctx, TextAlign::Left, TextBaseline::Top);
    halo_text(ctx, "y", axis_x + 8.0, s.y + 4.0, style);
}

pub(crate) fn draw_graph<C: Canvas>(ctx: &mut C, g: &Graph, size: f64, style: Style) {
    let s = g.plot_space();
    let scale = ctx.scale();
    ctx.save();
    // The frame is always drawn; it is what a graph too small to read shows.
    ctx.set_stroke_style(if style.dark() {
        Rgba::with_alpha(226, 232, 240, 0.28)
    } else {
        Rgba::with_alpha(15, 23, 42, 0.22)
    });
    ctx.set_line_width(1.25 / scale);
    ctx.stroke_rect(s.x, s.y, s.w, s.h);
    if s.w * scale < MIN_DRAWN_PX {
        ctx.restore();
        return;
    }

    let mut clip = Path::new();
    clip.rect(s.x, s.y, s.w, s.h);
    ctx.clip(&clip);
    draw_axes(ctx, g, &s, size, style);

    let Geometry {
        curves,
        marks,
        points,
    } = geometry(g, sample_count(s.w * scale));
    let width = 8f64.min(1.5f64.max(size));
    round_ends(ctx);
    for c in &curves {
        let Some(shade) = &c.shade else { continue };
        ctx.save();
        ctx.set_global_alpha(0.18);
        ctx.set_fill_style(style.ink(c.color));
        ctx.fill(&shade_path(shade));
        ctx.restore();
    }
    for c in &curves {
        ctx.set_stroke_style(style.ink(c.color));
        ctx.set_line_width(width);
        ctx.set_line_dash(&if c.dash {
            vec![width * 3.0, width * 2.2]
        } else {
            vec![]
        });
        ctx.stroke(&runs_path(&c.runs));
    }
    ctx.set_line_dash(&[]);

    ctx.set_font(false, 400, TICK_LABEL);
    align(ctx, TextAlign::Left, TextBaseline::Bottom);
    for m in &marks {
        let (x, y) = (s.px(m.x), s.py(m.y));
        dot(ctx, x, y, 4.5, style.ink(m.color), style);
        ctx.set_fill_style(style.ink(m.color));
        halo_text(
            ctx,
            &m.label,
            (x + 7.0).min(s.x + s.w - 70.0),
            y - 6.0,
            style,
        );
    }
    for p in &points {
        let (x, y) = (s.px(p.x), s.py(p.y));
        dot(
            ctx,
            x,
            y,
            4.5f64.max(width * 1.4),
            style.ink(p.color),
            style,
        );
        if !p.label.is_empty() {
            ctx.set_fill_style(style.ink(p.color));
            ctx.set_font(true, 400, TICK_LABEL + 3.0);
            halo_text(ctx, &p.label, x + 8.0, y - 6.0, style);
        }
    }
    ctx.set_font(false, 400, TICK_LABEL);
    for c in &curves {
        let Some(shade) = c.shade.as_ref().filter(|s| s.area.is_finite()) else {
            continue;
        };
        ctx.set_fill_style(style.ink(c.color));
        align(ctx, TextAlign::Center, TextBaseline::Middle);
        let text = format!("S = {}", format_number(shade.area, 4));
        halo_text(ctx, &text, shade.at.0, shade.at.1, style);
    }
    ctx.restore();
}
