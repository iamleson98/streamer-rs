//! Drawing a symbol: its builder's lines turned by its quarter turns and fitted to its box, with
//! the label below. A port of `board/physics/symbol-draw.ts`.

use replay_model::element::BoardElement;
use replay_model::pack::symbol::{Drawing, MIN_SYMBOL, Symbol, build_symbol};

use crate::canvas::{Canvas, Path, TextAlign, TextBaseline};
use crate::maths::{Style, align, dot, halo_text, polyline, round_ends, stroke_runs};

const LABEL_ROOM: f64 = 24.0;

struct Label<'a> {
    text: &'a str,
    x: f64,
    y: f64,
    inside: bool,
}

struct Laid<'a> {
    solid: Path,
    dashed: Path,
    fills: Vec<Path>,
    dots: Vec<[f64; 3]>,
    texts: Vec<(&'static str, f64, f64, f64)>,
    label: Option<Label<'a>>,
}

/// Scales a drawing down (never up) to fit a room, centred.
fn fit_drawing(d: Drawing, w: f64, h: f64) -> Drawing {
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for l in &d.lines {
        for p in l.points.as_chunks::<2>().0 {
            min_x = min_x.min(p[0]);
            max_x = max_x.max(p[0]);
            min_y = min_y.min(p[1]);
            max_y = max_y.max(p[1]);
        }
    }
    if !min_x.is_finite() {
        return d;
    }
    let scale = 1f64
        .min((w * 0.94) / 1e-9f64.max(max_x - min_x))
        .min((h * 0.94) / 1e-9f64.max(max_y - min_y));
    let (cx, cy) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    let mut d = d;
    for l in &mut d.lines {
        for p in l.points.as_chunks_mut::<2>().0 {
            p[0] = (p[0] - cx) * scale;
            p[1] = (p[1] - cy) * scale;
        }
    }
    for dot in &mut d.dots {
        *dot = [(dot[0] - cx) * scale, (dot[1] - cy) * scale, dot[2] * scale];
    }
    for t in &mut d.texts {
        t.x = (t.x - cx) * scale;
        t.y = (t.y - cy) * scale;
    }
    d
}

/// The point turned `turns` quarter turns clockwise (y down).
fn turn(x: f64, y: f64, turns: u8) -> [f64; 2] {
    match turns {
        1 => [-y, x],
        2 => [-x, -y],
        3 => [y, -x],
        _ => [x, y],
    }
}

fn layout(el: &Symbol) -> Laid<'_> {
    let b = el.rect();
    let labelled = !el.label.is_empty() && !el.kind.label_inside();
    let room = if labelled {
        MIN_SYMBOL.max(b.h - LABEL_ROOM)
    } else {
        b.h
    };
    let sideways = el.turns % 2 == 1;
    let (bw, bh) = if sideways { (room, b.w) } else { (b.w, room) };
    let mut drawing = build_symbol(el.kind, bw, bh, el.value);
    if el.kind.fitted() {
        drawing = fit_drawing(drawing, bw, bh);
    }
    let (cx, cy) = (b.x + b.w / 2.0, b.y + room / 2.0);
    let place = |x: f64, y: f64| {
        let t = turn(x, y, el.turns);
        [cx + t[0], cy + t[1]]
    };

    let (mut solid, mut dashed) = (Path::new(), Path::new());
    let mut fills = Vec::new();
    for l in &drawing.lines {
        let points: Vec<f64> = l
            .points
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| place(p[0], p[1]))
            .collect();
        polyline(if l.dashed { &mut dashed } else { &mut solid }, &points);
        if l.fill {
            let mut f = Path::new();
            polyline(&mut f, &points);
            f.close_path();
            fills.push(f);
        }
    }
    let label = (!el.label.is_empty()).then(|| {
        if el.kind.label_inside() {
            Label {
                text: &el.label,
                x: b.x + b.w / 2.0,
                y: b.y + b.h / 2.0,
                inside: true,
            }
        } else {
            Label {
                text: &el.label,
                x: b.x + b.w / 2.0,
                y: b.y + b.h - LABEL_ROOM / 2.0,
                inside: false,
            }
        }
    });
    Laid {
        solid,
        dashed,
        fills,
        dots: drawing
            .dots
            .iter()
            .map(|d| {
                let p = place(d[0], d[1]);
                [p[0], p[1], d[2]]
            })
            .collect(),
        texts: drawing
            .texts
            .iter()
            .map(|t| {
                let p = place(t.x, t.y);
                (t.text, p[0], p[1], t.size)
            })
            .collect(),
        label,
    }
}

pub(crate) fn draw_symbol<C: Canvas>(
    ctx: &mut C,
    el: &Symbol,
    common: &BoardElement,
    style: Style,
) {
    let Laid {
        solid,
        dashed,
        fills,
        dots,
        texts,
        label,
    } = layout(el);
    let color = style.ink(common.color);
    let width = 5f64.min(1.5f64.max(common.size * 0.7));
    ctx.save();
    round_ends(ctx);
    ctx.set_stroke_style(color);
    ctx.set_fill_style(color);
    ctx.set_global_alpha(0.14);
    for f in &fills {
        ctx.fill(f);
    }
    ctx.set_global_alpha(1.0);
    stroke_runs(ctx, &solid, &dashed, width);
    for [x, y, r] in dots {
        dot(ctx, x, y, r, color, style);
    }
    align(ctx, TextAlign::Center, TextBaseline::Middle);
    ctx.set_fill_style(color);
    for (text, x, y, size) in texts {
        ctx.set_font(true, 600, size);
        halo_text(ctx, text, x, y, style);
    }
    if let Some(l) = label {
        ctx.set_font(l.inside, if l.inside { 600 } else { 400 }, 16.0);
        halo_text(ctx, l.text, l.x, l.y, style);
    }
    ctx.restore();
}
