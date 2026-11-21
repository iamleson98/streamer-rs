//! Drawing a solid: its flat primitives fitted to its box, with hidden edges dashed and vertex
//! letters beside the vertices. A port of `board/math/solid-draw.ts`.

use replay_model::element::BoardElement;
use replay_model::pack::Solid;
use replay_model::pack::solid_flat::{flat_bounds, flatten_solid};

use crate::canvas::{Canvas, Path, TextAlign, TextBaseline};
use crate::maths::{Style, align, dot, halo_text, polyline, round_ends, stroke_runs};

const PAD: f64 = 26.0;
const LABEL_OFFSET: f64 = 13.0;

struct Laid {
    solid: Path,
    dashed: Path,
    fills: Vec<Path>,
    labels: Vec<(String, f64, f64)>,
    dots: Vec<[f64; 2]>,
}

fn layout(el: &Solid) -> Laid {
    let flat = flatten_solid(el);
    let [min_x, min_y, max_x, max_y] = flat_bounds(&flat);
    let b = el.rect();
    let w = 1e-9f64.max(max_x - min_x);
    let h = 1e-9f64.max(max_y - min_y);
    let scale = ((b.w - PAD * 2.0) / w).min((b.h - PAD * 2.0) / h);
    let cx = b.x + b.w / 2.0 - ((min_x + max_x) / 2.0) * scale;
    let cy = b.y + b.h / 2.0 + ((min_y + max_y) / 2.0) * scale;
    // Plane (y up) to board (y down).
    let at = |x: f64, y: f64| [cx + x * scale, cy - y * scale];
    let at_all = |points: &[f64]| -> Vec<f64> {
        points
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| at(p[0], p[1]))
            .collect()
    };

    let (mut solid, mut dashed) = (Path::new(), Path::new());
    for line in &flat.lines {
        polyline(
            if line.hidden { &mut dashed } else { &mut solid },
            &at_all(&line.points),
        );
    }
    let fills = flat
        .fills
        .iter()
        .map(|polygon| {
            let mut path = Path::new();
            polyline(&mut path, &at_all(polygon));
            path.close_path();
            path
        })
        .collect();

    // Letters sit outside the solid: away from the middle of the drawing.
    let middle = (b.x + b.w / 2.0, b.y + b.h / 2.0);
    let labels = flat
        .labels
        .iter()
        .map(|l| {
            let [x, y] = at(l.x, l.y);
            let (mut dx, mut dy) = (x - middle.0, y - middle.1);
            let len = dx.hypot(dy);
            if len < 6.0 {
                dx = 0.7;
                dy = 0.7;
            } else {
                dx /= len;
                dy /= len;
            }
            (l.text.clone(), x + dx * LABEL_OFFSET, y + dy * LABEL_OFFSET)
        })
        .collect();
    Laid {
        solid,
        dashed,
        fills,
        labels,
        dots: flat.dots.iter().map(|d| at(d[0], d[1])).collect(),
    }
}

pub(crate) fn draw_solid<C: Canvas>(ctx: &mut C, el: &Solid, common: &BoardElement, style: Style) {
    let Laid {
        solid,
        dashed,
        fills,
        labels,
        dots,
    } = layout(el);
    let color = style.ink(common.color);
    let width = 6f64.min(1.5f64.max(common.size * 0.75));
    ctx.save();
    round_ends(ctx);
    if el.tint {
        ctx.set_global_alpha(0.12);
        ctx.set_fill_style(color);
        for f in &fills {
            ctx.fill(f);
        }
        ctx.set_global_alpha(1.0);
    }
    ctx.set_stroke_style(color);
    stroke_runs(ctx, &solid, &dashed, width);
    for [x, y] in dots {
        dot(ctx, x, y, 3.0, color, style);
    }
    if el.labels {
        ctx.set_font(true, 600, 17.0);
        align(ctx, TextAlign::Center, TextBaseline::Middle);
        ctx.set_fill_style(color);
        for (text, x, y) in &labels {
            halo_text(ctx, text, *x, *y, style);
        }
    }
    ctx.restore();
}
