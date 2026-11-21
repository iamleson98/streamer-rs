//! The player's animations (`quiz/components/quiz-motion.css`, `quest-motion.css` and the running counts), as
//! functions of the time since the thing they belong to changed. A CSS animation eases each step between two
//! keyframes with its own timing function; `ease` is that curve.

use crate::kit::ease;

/// `cubic-bezier(0, 0, 0.58, 1)`.
fn ease_out(p: f32) -> f32 {
    ease(p, 0.0, 0.0, 0.58, 1.0)
}

/// `cubic-bezier(0.42, 0, 0.58, 1)`.
fn ease_in_out(p: f32) -> f32 {
    ease(p, 0.42, 0.0, 0.58, 1.0)
}

fn lerp(a: f32, b: f32, e: f32) -> f32 {
    a + (b - a) * e
}

/// `qz-pop` (380 ms, `cubic-bezier(0.34, 1.56, 0.64, 1)`): scale 0.5 and transparent, 1.1 and opaque at 70 %, then 1.
/// Returns (opacity, scale).
pub fn pop(age: f32) -> (f32, f32) {
    let p = (age / 380.0).clamp(0.0, 1.0);
    let e = |x: f32| ease(x, 0.34, 1.56, 0.64, 1.0);
    if p < 0.7 {
        let k = e(p / 0.7);
        (lerp(0.0, 1.0, k).clamp(0.0, 1.0), lerp(0.5, 1.1, k))
    } else {
        (1.0, lerp(1.1, 1.0, e((p - 0.7) / 0.3)))
    }
}

/// `qz-slide` (420 ms ease-out after `delay`): from transparent and 14 px low. Returns (opacity, translateY).
pub fn slide(age: f32, delay: f32) -> (f32, f32) {
    let e = ease_out(((age - delay) / 420.0).clamp(0.0, 1.0));
    (e, lerp(14.0, 0.0, e))
}

/// `qz-splash` (650 ms ease-out): in, hold, out. Returns (opacity, scale), `None` once it is over.
pub fn splash(age: f32) -> Option<(f32, f32)> {
    if !(0.0..650.0).contains(&age) {
        return None;
    }
    let p = age / 650.0;
    Some(if p < 0.25 {
        let e = ease_out(p / 0.25);
        (e, lerp(0.7, 1.0, e))
    } else if p < 0.7 {
        (1.0, lerp(1.0, 1.04, ease_out((p - 0.25) / 0.45)))
    } else {
        let e = ease_out((p - 0.7) / 0.3);
        (lerp(1.0, 0.0, e), lerp(1.04, 1.12, e))
    })
}

/// The position inside a 1 s loop of an animation that started `age` ms ago.
fn cycle(age: f32, period: f32) -> f32 {
    (age.rem_euclid(period)) / period
}

/// `qz-edge` (1 s, ease-in-out, forever): opacity 1, 0.35 at the middle, 1.
pub fn edge(age: f32) -> f32 {
    let p = cycle(age, 1000.0);
    if p < 0.5 {
        lerp(1.0, 0.35, ease_in_out(p / 0.5))
    } else {
        lerp(0.35, 1.0, ease_in_out((p - 0.5) / 0.5))
    }
}

/// `qz-ring-pulse` (1 s, ease-in-out, forever): scale 1, 1.12 at the middle, 1.
pub fn pulse(age: f32) -> f32 {
    let p = cycle(age, 1000.0);
    if p < 0.5 {
        lerp(1.0, 1.12, ease_in_out(p / 0.5))
    } else {
        lerp(1.12, 1.0, ease_in_out((p - 0.5) / 0.5))
    }
}

/// `qz-dot` (1.1 s, ease-in-out, forever, `delay` ms): up 6 px at 40 %, down again by 80 %.
pub fn dot(age: f32, delay: f32) -> f32 {
    let a = age - delay;
    if a < 0.0 {
        return 0.0;
    }
    let p = cycle(a, 1100.0);
    if p < 0.4 {
        lerp(0.0, -6.0, ease_in_out(p / 0.4))
    } else if p < 0.8 {
        lerp(-6.0, 0.0, ease_in_out((p - 0.4) / 0.4))
    } else {
        0.0
    }
}

/// `qz-shine` (900 ms ease-out after 150 ms): the sweep's x in widths of the sweep, from -1 to 4.2.
pub fn shine(age: f32) -> f32 {
    lerp(-1.0, 4.2, ease_out(((age - 150.0) / 900.0).clamp(0.0, 1.0)))
}

/// `qz-mover` (1.4 s ease-out): a row that moved is lit: opacity 0.4 to 1, scale 1.03 to 1. `None` when over.
pub fn mover(age: f32) -> Option<(f32, f32)> {
    if !(0.0..1400.0).contains(&age) {
        return None;
    }
    let e = ease_out(age / 1400.0);
    Some((lerp(0.4, 1.0, e), lerp(1.03, 1.0, e)))
}

/// `CountUp`: 800 ms, ease-out cubic, rounded.
pub fn count_up(age: f32, from: i64, to: i64) -> i64 {
    let p = (age / 800.0).clamp(0.0, 1.0);
    let e = 1.0 - (1.0 - p).powi(3);
    (from as f32 + (to - from) as f32 * e).round() as i64
}

/// A standings row sliding from one place to another (700 ms, `cubic-bezier(0.34, 1.2, 0.64, 1)`): its place.
pub fn row_place(age: f32, from: f32, to: f32) -> f32 {
    lerp(
        from,
        to,
        ease((age / 700.0).clamp(0.0, 1.0), 0.34, 1.2, 0.64, 1.0),
    )
}

/// Quest's board row sliding (650 ms, `cubic-bezier(0.22, 1, 0.36, 1)`).
pub fn quest_row_place(age: f32, from: f32, to: f32) -> f32 {
    lerp(
        from,
        to,
        ease((age / 650.0).clamp(0.0, 1.0), 0.22, 1.0, 0.36, 1.0),
    )
}
