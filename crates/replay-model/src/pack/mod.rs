//! The maths and physics packs' elements (graphs, solids and symbols), read and measured without drawing: ports of
//! `board/math/` and `board/box.ts` of the web player. `board-draw` paints what this decodes.

pub mod expression;
pub mod format;
pub mod graph;
pub mod sampling;
pub mod solid;
pub mod solid_flat;
pub mod symbol;

use serde_json::Value;

pub use graph::Graph;
pub use solid::Solid;
pub use symbol::Symbol;

/// The typed content of a pack element.
#[derive(Debug, Clone, PartialEq)]
pub enum PackBody {
    Graph(Box<Graph>),
    Solid(Box<Solid>),
    Symbol(Box<Symbol>),
}

/// The box of an element placed by dragging a rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

const MAX_BOX: f64 = 4000.0;

/// `Math.min(hi, Math.max(lo, v))`.
pub(crate) fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    hi.min(lo.max(v))
}

/// The box as a position and a size, no smaller than `min` either way (`boxOf`).
pub fn box_of(x1: f64, y1: f64, x2: f64, y2: f64, min: f64) -> BoxRect {
    BoxRect {
        x: x1.min(x2),
        y: y1.min(y2),
        w: clamp((x2 - x1).abs(), min, MAX_BOX),
        h: clamp((y2 - y1).abs(), min, MAX_BOX),
    }
}

/// `typeof v === 'number' && Number.isFinite(v)`.
pub(crate) fn finite(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// Four finite numbers `x1, y1, x2, y2`.
pub(crate) fn corners(d: &serde_json::Map<String, Value>) -> Option<[f64; 4]> {
    Some([
        finite(d.get("x1"))?,
        finite(d.get("y1"))?,
        finite(d.get("x2"))?,
        finite(d.get("y2"))?,
    ])
}

/// The number of UTF-16 code units of `s` (JavaScript's `length`).
pub(crate) fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.slice(0, n)` in UTF-16 units. A surrogate pair the cut would split is left out (the web
/// player keeps half of it, which draws as a replacement character).
pub(crate) fn utf16_prefix(s: &str, n: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in s.chars() {
        units += c.len_utf16();
        if units > n {
            break;
        }
        out.push(c);
    }
    out
}

/// `Math.round`: halves go up, and a small negative number gives -0.
pub(crate) fn js_round(x: f64) -> f64 {
    if (-0.5..0.0).contains(&x) {
        return -0.0;
    }
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

/// What JavaScript's `String.prototype.trim` removes.
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// `Math.min`: a NaN argument makes a NaN.
pub(crate) fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// `Math.max`: a NaN argument makes a NaN.
pub(crate) fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}
