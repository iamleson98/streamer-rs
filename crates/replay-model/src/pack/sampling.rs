//! A graph's numbers, without any drawing: tick steps, the curves sampled from their formulas
//! (split where a curve jumps or leaves the plane), the marked points and the shaded area. A port
//! of the geometry half of `board/math/graph-draw.ts`.
//!
//! Cost is bounded by pixels, not by what the element says: a curve is sampled about once per two
//! screen pixels (64 to 800 samples).

use std::f64::consts::PI;

use super::expression::{Expression, Variable, compile};
use super::format::format_number;
use super::graph::{FnSeries, Graph, Mark, ParamSeries, PlotSpace, PointSeries, Series};
use super::{js_max, js_min};
use crate::element::ColorToken;

const MIN_SAMPLES: f64 = 64.0;
const MAX_SAMPLES: f64 = 800.0;
/// A jump larger than this many view heights splits the curve (asymptotes).
const BREAK: f64 = 3.0;
const MAX_MARKS: usize = 12;
const TICK_GAP: f64 = 64.0;
/// Tick steps in quarters of π: π/4, π/2, π, 2π, 4π, 10π...
const PI_STEPS: [i64; 10] = [1, 2, 4, 8, 16, 40, 80, 160, 400, 800];

/// A 1, 2 or 5 step so that about `target` ticks fit in `span`.
pub fn nice_step(span: f64, target: f64) -> f64 {
    let raw = span / target.max(1.0);
    let mag = 10f64.powf(raw.log10().floor());
    let n = raw / mag;
    (if n < 1.5 {
        1.0
    } else if n < 3.0 {
        2.0
    } else if n < 7.0 {
        5.0
    } else {
        10.0
    }) * mag
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub value: f64,
    pub label: String,
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// `k` quarters of π as a label: π/2, 3π/4, -π, 2π.
pub fn pi_label(k: i64, quarters: i64) -> String {
    if k == 0 {
        return "0".to_owned();
    }
    let g = gcd(k.abs(), quarters);
    let (n, d) = (k / g, quarters / g);
    let sign = if n < 0 { "-" } else { "" };
    let abs = n.abs();
    let top = if abs == 1 {
        "π".to_owned()
    } else {
        format!("{abs}π")
    };
    if d == 1 {
        format!("{sign}{top}")
    } else {
        format!("{sign}{top}/{d}")
    }
}

pub fn axis_ticks(min: f64, max: f64, length_px: f64, pi: bool) -> Vec<Tick> {
    let mut out = Vec::new();
    if pi {
        // Steps of π/4, π/2, π, 2π... the first that leaves room for the labels.
        let quarters = PI_STEPS
            .iter()
            .copied()
            .find(|q| (*q as f64 * PI) / 4.0 / (max - min) * length_px >= TICK_GAP * 0.75)
            .unwrap_or(PI_STEPS[PI_STEPS.len() - 1]);
        let step = (quarters as f64 * PI) / 4.0;
        let mut k = (min / step - 1e-9).ceil();
        while k * step <= max + 1e-9 && out.len() < 200 {
            out.push(Tick {
                value: k * step,
                label: pi_label(k as i64 * quarters, 4),
            });
            k += 1.0;
        }
        return out;
    }
    let step = nice_step(max - min, length_px / TICK_GAP);
    let mut k = (min / step - 1e-9).ceil();
    while k * step <= max + step * 1e-9 && out.len() < 200 {
        let value = if (k * step).abs() < step * 1e-9 {
            0.0
        } else {
            k * step
        };
        out.push(Tick {
            value,
            label: format_number(value, 6),
        });
        k += 1.0;
    }
    out
}

/// How many samples a curve gets on a graph this wide on screen.
pub fn sample_count(width_px: f64) -> usize {
    let n = (width_px / 2.0 / 32.0).ceil() * 32.0;
    js_min(MAX_SAMPLES, js_max(MIN_SAMPLES, n)) as usize
}

/// A continuous stretch of a curve: flat `[x, y, x, y...]` in board units.
pub type Run = Vec<f64>;

#[derive(Debug, Clone, PartialEq)]
pub struct MarkedPoint {
    pub x: f64,
    pub y: f64,
    pub label: String,
    pub color: ColorToken,
}

/// The shaded area under a curve: closed polygons (flat points), its value and where to say it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shade {
    pub polygons: Vec<Vec<f64>>,
    pub area: f64,
    pub at: (f64, f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub color: ColorToken,
    pub dash: bool,
    pub runs: Vec<Run>,
    pub shade: Option<Shade>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub curves: Vec<Curve>,
    pub marks: Vec<MarkedPoint>,
    pub points: Vec<PointSeries>,
}

struct Sampled {
    runs: Vec<Run>,
    xs: Vec<f64>,
    ys: Vec<f64>,
}

fn reach(v: f64, lo: f64, hi: f64) -> f64 {
    js_max(lo, js_min(hi, v))
}

/// Splits a sampled curve into continuous runs of board coordinates.
fn sample_fn(e: &Expression, g: &Graph, space: &PlotSpace, n: usize) -> Sampled {
    let view = g.view;
    let span_y = view.ymax - view.ymin;
    let limit = span_y * 50.0;
    let step = (view.xmax - view.xmin) / n as f64;
    let mut runs = Vec::new();
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    let mut run: Run = Vec::new();
    let mut prev = f64::NAN;
    for i in 0..=n {
        let x = view.xmin + i as f64 * step;
        let y = e.eval(x, &g.params);
        xs.push(x);
        ys.push(y);
        let ok = y.is_finite();
        if (!ok || (prev.is_finite() && (y - prev).abs() > span_y * BREAK)) && !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
        if ok {
            run.push(space.px(x));
            run.push(space.py(reach(y, view.ymin - limit, view.ymax + limit)));
        }
        prev = y;
    }
    if !run.is_empty() {
        runs.push(run);
    }
    Sampled { runs, xs, ys }
}

fn sample_param(s: &ParamSeries, g: &Graph, space: &PlotSpace, n: usize) -> Vec<Run> {
    let (Ok(fx), Ok(fy)) = (
        compile(&s.x, Variable::T, g.degrees),
        compile(&s.y, Variable::T, g.degrees),
    ) else {
        return Vec::new();
    };
    let view = g.view;
    let reach_x = (view.xmax - view.xmin) * 50.0;
    let reach_y = (view.ymax - view.ymin) * 50.0;
    let (t0, t1) = s.t;
    let mut runs = Vec::new();
    let mut run: Run = Vec::new();
    let count = n.max(90);
    for i in 0..=count {
        let t = t0 + ((t1 - t0) * i as f64) / count as f64;
        let x = fx.eval(t, &g.params);
        let y = fy.eval(t, &g.params);
        if !x.is_finite() || !y.is_finite() {
            if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
            continue;
        }
        run.push(space.px(reach(x, view.xmin - reach_x, view.xmax + reach_x)));
        run.push(space.py(reach(y, view.ymin - reach_y, view.ymax + reach_y)));
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

fn clean(v: f64) -> f64 {
    if v.abs() < 1e-9 { 0.0 } else { v }
}

fn mark_label(x: f64, y: f64) -> String {
    format!(
        "({}, {})",
        format_number(clean(x), 3),
        format_number(clean(y), 3)
    )
}

/// Roots, extrema and the y-intercept of a sampled curve, refined numerically.
fn find_marks(s: &FnSeries, g: &Graph, e: &Expression, sample: &Sampled) -> Vec<MarkedPoint> {
    let (xs, ys) = (&sample.xs, &sample.ys);
    let f = |x: f64| e.eval(x, &g.params);
    let view = g.view;
    let span_y = view.ymax - view.ymin;
    let mut found: Vec<MarkedPoint> = Vec::new();
    let mut add = |x: f64, y: f64| {
        if found.len() >= MAX_MARKS {
            return;
        }
        if x < view.xmin
            || x > view.xmax
            || y < view.ymin - span_y * 0.05
            || y > view.ymax + span_y * 0.05
        {
            return;
        }
        if found
            .iter()
            .any(|m| (m.x - x).abs() < 1e-9 && (m.y - y).abs() < 1e-9)
        {
            return;
        }
        found.push(MarkedPoint {
            x,
            y,
            label: mark_label(x, y),
            color: s.color,
        });
    };
    let smooth = |i: usize| {
        ys[i].is_finite() && ys[i + 1].is_finite() && (ys[i + 1] - ys[i]).abs() < span_y * BREAK
    };
    if s.marks.contains(&Mark::Roots) {
        for i in 0..ys.len() - 1 {
            if !smooth(i) {
                continue;
            }
            if ys[i] == 0.0 {
                add(xs[i], 0.0);
            } else if ys[i] * ys[i + 1] < 0.0 {
                let (mut lo, mut hi) = (xs[i], xs[i + 1]);
                for _ in 0..48 {
                    let mid = (lo + hi) / 2.0;
                    if f(lo) * f(mid) <= 0.0 {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                let root = (lo + hi) / 2.0;
                if f(root).abs() < span_y * 0.01 {
                    add(root, 0.0);
                }
            }
        }
    }
    if s.marks.contains(&Mark::Extrema) {
        for i in 1..ys.len() - 1 {
            if !smooth(i - 1) || !smooth(i) {
                continue;
            }
            let up = ys[i] - ys[i - 1];
            let down = ys[i + 1] - ys[i];
            if up * down >= 0.0 {
                continue;
            }
            let maximum = up > 0.0;
            let (mut lo, mut hi) = (xs[i - 1], xs[i + 1]);
            for _ in 0..60 {
                let a = lo + (hi - lo) / 3.0;
                let b = hi - (hi - lo) / 3.0;
                if if maximum { f(a) < f(b) } else { f(a) > f(b) } {
                    lo = a;
                } else {
                    hi = b;
                }
            }
            let x = (lo + hi) / 2.0;
            add(x, f(x));
        }
    }
    if s.marks.contains(&Mark::Intercept) && view.xmin <= 0.0 && view.xmax >= 0.0 {
        let y = f(0.0);
        if y.is_finite() {
            add(0.0, y);
        }
    }
    found
}

/// ∫ f over [a, b] by the trapezoid rule on a fine grid; NaN across a pole.
fn area(f: &dyn Fn(f64) -> f64, a: f64, b: f64) -> f64 {
    let n = 400;
    let h = (b - a) / f64::from(n);
    let mut sum = 0.0;
    let mut prev = f(a);
    for i in 1..=n {
        let y = f(a + f64::from(i) * h);
        if !y.is_finite() || !prev.is_finite() {
            return f64::NAN;
        }
        sum += ((prev + y) / 2.0) * h;
        prev = y;
    }
    sum
}

fn build_shade(
    s: &FnSeries,
    g: &Graph,
    space: &PlotSpace,
    f: &dyn Fn(f64) -> f64,
) -> Option<Shade> {
    let (s0, s1) = s.shade?;
    let view = g.view;
    let a = s0.max(view.xmin);
    let b = s1.min(view.xmax);
    if b <= a {
        return None;
    }
    let n =
        MAX_SAMPLES.min(16f64.max((((b - a) / (view.xmax - view.xmin)) * 400.0).ceil())) as usize;
    let base = space.py(0.0);
    let limit = (view.ymax - view.ymin) * 50.0;
    let mut polygons: Vec<Vec<f64>> = Vec::new();
    let mut open: Option<Vec<f64>> = None;
    let mut last_x = 0.0;
    for i in 0..=n {
        let x = a + ((b - a) * i as f64) / n as f64;
        let y = f(x);
        if !y.is_finite() {
            if let Some(mut poly) = open.take() {
                poly.extend([last_x, base]);
                polygons.push(poly);
            }
            continue;
        }
        let px = space.px(x);
        let poly = open.get_or_insert_with(|| vec![px, base]);
        poly.extend([px, space.py(reach(y, view.ymin - limit, view.ymax + limit))]);
        last_x = px;
    }
    if let Some(mut poly) = open.take() {
        poly.extend([last_x, base]);
        polygons.push(poly);
    }
    let mid = (a + b) / 2.0;
    Some(Shade {
        polygons,
        area: area(f, s0, s1),
        at: (
            space.px(mid),
            space.py(js_max(0.0, js_min(view.ymax, f(mid) / 2.0))),
        ),
    })
}

/// The curves, marked points and points of a graph, for a curve sampled `samples` times.
pub fn geometry(g: &Graph, samples: usize) -> Geometry {
    let space = g.plot_space();
    let mut out = Geometry {
        curves: Vec::new(),
        marks: Vec::new(),
        points: Vec::new(),
    };
    for s in &g.series {
        match s {
            Series::Point(p) => out.points.push(p.clone()),
            Series::Fn(s) => {
                let Ok(e) = compile(&s.expr, Variable::X, g.degrees) else {
                    continue;
                };
                let sampled = sample_fn(&e, g, &space, samples);
                let f = |x: f64| e.eval(x, &g.params);
                let shade = build_shade(s, g, &space, &f);
                if !s.marks.is_empty() {
                    out.marks.extend(find_marks(s, g, &e, &sampled));
                }
                out.curves.push(Curve {
                    color: s.color,
                    dash: s.dash,
                    runs: sampled.runs,
                    shade,
                });
            }
            Series::Param(s) => out.curves.push(Curve {
                color: s.color,
                dash: s.dash,
                runs: sample_param(s, g, &space, samples),
                shade: None,
            }),
        }
    }
    out
}
