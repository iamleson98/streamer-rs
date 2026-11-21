//! Quest's number formats (`quiz/quest/format.ts`).

/// Minutes and seconds of a time left, as a clock reads: `4:07`.
pub fn clock(ms: f64) -> String {
    let total = (ms / 1000.0).ceil().max(0.0) as i64;
    let s = total % 60;
    format!("{}:{}{}", total / 60, if s < 10 { "0" } else { "" }, s)
}

/// The share of right answers as a whole percentage, or `None` before any answer.
pub fn accuracy(correct: i64, answers: i64) -> Option<i64> {
    (answers > 0).then(|| (100.0 * correct as f64 / answers as f64).round() as i64)
}

/// The question's countdown as numbers the screens draw (`quiz/countdown.svelte.ts`): the deadline
/// is on the recording's clock, so nothing ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Countdown {
    pub left: f64,
    pub seconds: i64,
    pub fraction: f64,
    pub urgent: bool,
    pub over: bool,
}

pub fn countdown(ends_at: f64, limit_ms: f64, now: f64) -> Countdown {
    let left = (ends_at - now).max(0.0);
    let seconds = (left / 1000.0).ceil() as i64;
    Countdown {
        left,
        seconds,
        fraction: if limit_ms > 0.0 { (left / limit_ms).min(1.0) } else { 0.0 },
        urgent: left > 0.0 && seconds <= 5,
        over: left == 0.0,
    }
}
