//! Compares the Rust frames with the web player's: the layout dump (text lines, measured widths,
//! element bounds, the camera) and the pixels (SSIM, PSNR, and the share of pixels that differ by
//! more than 8/255, overall and outside the text boxes) and, on the Rust side, the glyphs that came
//! out as a missing-glyph box (any is a failure). Writes a Markdown summary and a
//! self-contained HTML report (by group, with the three pictures of every frame as thumbnails and,
//! as far as `--max-mb` allows, at full size on tap), and exits with 1 when a threshold is broken.
//! A difference the owner accepted (`parity/accepted.json`) passes its check as "accepted" while it
//! stays within its limit; an entry whose frame now passes cleanly is stale, and fails the run.
//!
//!   cargo run -p board-draw --example compare --locked -- --corpus DIR --frames DIR
//!       [--report FILE.md] [--html FILE.html] [--replay-commit SHA] [--max-mb N]
//!       [--max-layout PX] [--min-ssim X] [--max-off-percent P] [--accepted FILE.json] [--extra-corpus DIR]

mod support;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use board_draw::{BOARD_FONT_FILES, FALLBACK_FONT_FILES, decode_rgba, encode_png};
use serde_json::Value;

/// The pixel difference that counts, out of 255.
const PIXEL_TOLERANCE: i32 = 8;
/// Room around a text element's box that is ignored in the pixel count (its anti-aliased edge).
const TEXT_PAD: f64 = 3.0;

struct Thresholds {
    layout: f64,
    ssim: f64,
    off_percent: f64,
    /// The accepted differences that apply on this platform.
    accepted: Vec<Accepted>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Check {
    Layout,
    Ssim,
    Pixels,
}

impl Check {
    fn label(self) -> &'static str {
        match self {
            Check::Layout => "layout",
            Check::Ssim => "SSIM",
            Check::Pixels => "pixels",
        }
    }
}

/// How a frame stands on one check.
#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Pass,
    Accepted,
    Fail,
}

/// One entry of `accepted.json`: the worst value of one check the owner accepted for one frame.
struct Accepted {
    frame: String,
    check: Check,
    limit: f64,
    reason: String,
    accepted: String,
}

impl Accepted {
    fn parse(entry: &Value) -> Option<Accepted> {
        let text = |k: &str| {
            entry[k]
                .as_str()
                .unwrap_or_else(|| panic!("accepted.json: {k}"))
        };
        if entry
            .get("os")
            .and_then(Value::as_str)
            .is_some_and(|os| os != std::env::consts::OS)
        {
            return None;
        }
        Some(Accepted {
            frame: text("frame").to_owned(),
            check: match text("check") {
                "layout" => Check::Layout,
                "ssim" => Check::Ssim,
                "pixels" => Check::Pixels,
                other => panic!("accepted.json: unknown check {other}"),
            },
            limit: entry["limit"].as_f64().expect("accepted.json: limit"),
            reason: text("reason").to_owned(),
            accepted: text("accepted").to_owned(),
        })
    }
}

/// The entries of `accepted.json` that apply on this platform (an entry may name an `os`).
fn load_accepted(path: &Path) -> Vec<Accepted> {
    let entries: Value = serde_json::from_slice(&read(path)).expect("accepted.json");
    entries
        .as_array()
        .expect("accepted.json is a list")
        .iter()
        .filter_map(Accepted::parse)
        .collect()
}

/// One number of the layout dump that differs, or a structural difference (`diff` is infinite).
struct Mismatch {
    who: String,
    field: String,
    ts: String,
    rust: String,
    diff: f64,
}

struct Row {
    name: String,
    group: String,
    scene: String,
    time: String,
    /// What the scene is about (shown with its first frame).
    about: Option<String>,
    layout_max: f64,
    mismatches: Vec<Mismatch>,
    ssim: f64,
    psnr: f64,
    off_outside: f64,
    off_all: f64,
    /// Glyphs of the Rust frame that resolved to a font's missing-glyph box.
    missing: u64,
    /// TS, Rust and difference at full size.
    images: [Vec<u8>; 3],
    /// The same at half size (the difference keeps the larger of each 2x2 block, so a thin line
    /// does not fade away).
    thumbs: [Vec<u8>; 3],
}

impl Row {
    /// Whether the check passes on the thresholds alone.
    fn clean(&self, c: Check, t: &Thresholds) -> bool {
        match c {
            Check::Layout => {
                self.layout_max <= t.layout && self.mismatches.iter().all(|m| m.diff.is_finite())
            }
            Check::Ssim => self.ssim >= t.ssim,
            Check::Pixels => self.off_outside <= t.off_percent,
        }
    }
    /// Whether the check holds against an accepted limit (a mismatch that cannot be compared is
    /// never within one).
    fn within(&self, c: Check, limit: f64) -> bool {
        match c {
            Check::Layout => {
                self.layout_max <= limit && self.mismatches.iter().all(|m| m.diff.is_finite())
            }
            Check::Ssim => self.ssim >= limit,
            Check::Pixels => self.off_outside <= limit,
        }
    }
    fn value(&self, c: Check) -> f64 {
        match c {
            Check::Layout => self.layout_max,
            Check::Ssim => self.ssim,
            Check::Pixels => self.off_outside,
        }
    }
    fn waiver<'a>(&self, c: Check, t: &'a Thresholds) -> Option<&'a Accepted> {
        t.accepted
            .iter()
            .find(|a| a.frame == self.name && a.check == c)
    }
    fn state(&self, c: Check, t: &Thresholds) -> State {
        if self.clean(c, t) {
            State::Pass
        } else if self.waiver(c, t).is_some_and(|a| self.within(c, a.limit)) {
            State::Accepted
        } else {
            State::Fail
        }
    }
    fn glyphs_ok(&self) -> bool {
        self.missing == 0
    }
    fn ok(&self, t: &Thresholds) -> bool {
        self.glyphs_ok() && CHECKS.iter().all(|&c| self.state(c, t) != State::Fail)
    }
    fn is_accepted(&self, t: &Thresholds) -> bool {
        self.ok(t) && CHECKS.iter().any(|&c| self.state(c, t) == State::Accepted)
    }
}

const CHECKS: [Check; 3] = [Check::Layout, Check::Ssim, Check::Pixels];

/// Entries whose frame passes the check cleanly (or is not in the corpus): the list must not hide
/// a case that has been fixed.
fn stale<'a>(rows: &[Row], t: &'a Thresholds) -> Vec<&'a Accepted> {
    t.accepted
        .iter()
        .filter(|a| {
            rows.iter()
                .find(|r| r.name == a.frame)
                .is_none_or(|r| r.clean(a.check, t))
        })
        .collect()
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

/// Every number of the two layout dumps, side by side; `max` is the largest difference.
fn layout_diff(rust: &Value, ts: &Value, limit: f64) -> (f64, Vec<Mismatch>) {
    if ts.get("texts").is_some() {
        return stage_layout_diff(rust, ts, limit);
    }
    let mut max = 0f64;
    let mut out = Vec::new();
    let mut take = |who: &str, field: &str, ts: f64, rust: f64| {
        let diff = (ts - rust).abs();
        if diff.is_nan() {
            out.push(Mismatch {
                who: who.into(),
                field: field.into(),
                ts: ts.to_string(),
                rust: rust.to_string(),
                diff: f64::INFINITY,
            });
            return;
        }
        max = max.max(diff);
        if diff > limit {
            out.push(Mismatch {
                who: who.into(),
                field: field.into(),
                ts: format!("{ts:.3}"),
                rust: format!("{rust:.3}"),
                diff,
            });
        }
    };
    for k in ["x", "y", "zoom"] {
        take(
            "camera",
            k,
            num(&ts["viewport"][k]),
            num(&rust["viewport"][k]),
        );
    }
    let mut structural = Vec::new();
    for list in ["elements", "overlays"] {
        let (xs, ys) = (rust[list].as_array().unwrap(), ts[list].as_array().unwrap());
        if xs.len() != ys.len() {
            structural.push(Mismatch {
                who: list.into(),
                field: "count".into(),
                ts: ys.len().to_string(),
                rust: xs.len().to_string(),
                diff: f64::INFINITY,
            });
            continue;
        }
        for (x, y) in xs.iter().zip(ys) {
            let who = format!(
                "{} {} ({})",
                list.trim_end_matches('s'),
                x["id"].as_str().unwrap_or("?"),
                x["type"].as_str().unwrap_or("?")
            );
            if x["id"] != y["id"] || x["type"] != y["type"] {
                structural.push(Mismatch {
                    who,
                    field: "identity".into(),
                    ts: format!("{} {}", y["id"], y["type"]),
                    rust: format!("{} {}", x["id"], x["type"]),
                    diff: f64::INFINITY,
                });
                continue;
            }
            for (i, side) in ["min x", "min y", "max x", "max y"].iter().enumerate() {
                take(
                    &who,
                    &format!("bounds {side}"),
                    num(&y["bounds"][i]),
                    num(&x["bounds"][i]),
                );
            }
            match (x["lines"].as_array(), y["lines"].as_array()) {
                (Some(lx), Some(ly)) if lx.len() == ly.len() => {
                    for (n, (p, q)) in lx.iter().zip(ly).enumerate() {
                        if p["text"] != q["text"] {
                            structural.push(Mismatch {
                                who: who.clone(),
                                field: format!("line {n} text"),
                                ts: q["text"].to_string(),
                                rust: p["text"].to_string(),
                                diff: f64::INFINITY,
                            });
                        }
                        take(
                            &who,
                            &format!("line {n} width"),
                            num(&q["width"]),
                            num(&p["width"]),
                        );
                    }
                }
                (Some(lx), Some(ly)) => structural.push(Mismatch {
                    who,
                    field: "line count".into(),
                    ts: ly.len().to_string(),
                    rust: lx.len().to_string(),
                    diff: f64::INFINITY,
                }),
                (None, None) => {}
                _ => structural.push(Mismatch {
                    who,
                    field: "lines".into(),
                    ts: y["lines"].to_string(),
                    rust: x["lines"].to_string(),
                    diff: f64::INFINITY,
                }),
            }
        }
    }
    out.extend(structural);
    (max, out)
}

/// The layout dump of a quiz stage (`quiz-draw`): the text lines as laid out and the painted boxes, in document
/// order, each with its position and size. A line an ellipsis cut keeps its place and height, not its text and
/// width (the page cannot say where the ellipsis fell).
fn stage_layout_diff(rust: &Value, ts: &Value, limit: f64) -> (f64, Vec<Mismatch>) {
    let mut max = 0f64;
    let mut out = Vec::new();
    let mut structural = Vec::new();
    for (list, noun) in [("boxes", "box"), ("texts", "text")] {
        let (xs, ys) = (rust[list].as_array().unwrap(), ts[list].as_array().unwrap());
        if xs.len() != ys.len() {
            structural.push(Mismatch {
                who: list.into(),
                field: "count".into(),
                ts: ys.len().to_string(),
                rust: xs.len().to_string(),
                diff: f64::INFINITY,
            });
            continue;
        }
        for (n, (x, y)) in xs.iter().zip(ys).enumerate() {
            let label = if list == "texts" {
                let t: String = y["text"].as_str().unwrap_or("").chars().take(24).collect();
                format!("{noun} {n} \"{t}\"")
            } else {
                format!("{noun} {n} ({})", y["kind"].as_str().unwrap_or("?"))
            };
            let clipped = y["clipped"] == true;
            if list == "texts" && !clipped && x["text"] != y["text"] {
                structural.push(Mismatch {
                    who: label.clone(),
                    field: "text".into(),
                    ts: y["text"].to_string(),
                    rust: x["text"].to_string(),
                    diff: f64::INFINITY,
                });
            }
            if list == "texts" && x.get("color").is_some() && y.get("color").is_some() {
                // The colour a line is painted in: each channel within 2 of 255 (the CSS colour spaces round).
                let (a, b) = (
                    x["color"].as_array().unwrap(),
                    y["color"].as_array().unwrap(),
                );
                if a.iter().zip(b).any(|(p, q)| (num(p) - num(q)).abs() > 2.0) {
                    structural.push(Mismatch {
                        who: label.clone(),
                        field: "colour".into(),
                        ts: y["color"].to_string(),
                        rust: x["color"].to_string(),
                        diff: f64::INFINITY,
                    });
                }
            }
            for k in ["x", "y", "w", "h"] {
                if clipped && k == "w" {
                    continue;
                }
                let diff = (num(&x[k]) - num(&y[k])).abs();
                max = max.max(if diff.is_nan() { 0.0 } else { diff });
                if diff > limit || diff.is_nan() {
                    out.push(Mismatch {
                        who: label.clone(),
                        field: k.into(),
                        ts: format!("{:.3}", num(&y[k])),
                        rust: format!("{:.3}", num(&x[k])),
                        diff: if diff.is_nan() { f64::INFINITY } else { diff },
                    });
                }
            }
        }
    }
    out.extend(structural);
    (max, out)
}

fn luma(px: &[u8]) -> Vec<f64> {
    px.as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]))
        .collect()
}

/// Mean structural similarity of the two luma images over 8x8 windows, 4 pixels apart.
fn ssim(a: &[f64], b: &[f64], w: usize, h: usize) -> f64 {
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (mut sum, mut n) = (0.0, 0u32);
    let mut y = 0;
    while y + 8 <= h {
        let mut x = 0;
        while x + 8 <= w {
            let (mut ma, mut mb) = (0.0, 0.0);
            for j in 0..8 {
                for i in 0..8 {
                    ma += a[(y + j) * w + x + i];
                    mb += b[(y + j) * w + x + i];
                }
            }
            let (ma, mb) = (ma / 64.0, mb / 64.0);
            let (mut va, mut vb, mut cov) = (0.0, 0.0, 0.0);
            for j in 0..8 {
                for i in 0..8 {
                    let (da, db) = (a[(y + j) * w + x + i] - ma, b[(y + j) * w + x + i] - mb);
                    va += da * da;
                    vb += db * db;
                    cov += da * db;
                }
            }
            let (va, vb, cov) = (va / 63.0, vb / 63.0, cov / 63.0);
            sum += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            n += 1;
            x += 4;
        }
        y += 4;
    }
    sum / f64::from(n)
}

/// The text elements' boxes in pixels, from the layout.
fn text_boxes(layout: &Value) -> Vec<[f64; 4]> {
    if let Some(texts) = layout.get("texts").and_then(Value::as_array) {
        return texts
            .iter()
            .map(|t| {
                [
                    num(&t["x"]) - TEXT_PAD,
                    num(&t["y"]) - TEXT_PAD,
                    num(&t["x"]) + num(&t["w"]) + TEXT_PAD,
                    num(&t["y"]) + num(&t["h"]) + TEXT_PAD,
                ]
            })
            .collect();
    }
    let (vx, vy, z) = (
        num(&layout["viewport"]["x"]),
        num(&layout["viewport"]["y"]),
        num(&layout["viewport"]["zoom"]),
    );
    ["elements", "overlays"]
        .iter()
        .flat_map(|l| layout[*l].as_array().unwrap().iter())
        .filter(|e| e["type"] == "text")
        .map(|e| {
            let b = &e["bounds"];
            [
                (num(&b[0]) - vx) * z - TEXT_PAD,
                (num(&b[1]) - vy) * z - TEXT_PAD,
                (num(&b[2]) - vx) * z + TEXT_PAD,
                (num(&b[3]) - vy) * z + TEXT_PAD,
            ]
        })
        .collect()
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(char::from(A[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Halves a picture: the mean of each 2x2 block, or the largest value in it.
fn halve(px: &[u8], w: usize, h: usize, largest: bool) -> (u32, u32, Vec<u8>) {
    let (w2, h2) = (w / 2, h / 2);
    let mut out = vec![255u8; w2 * h2 * 4];
    for y in 0..h2 {
        for x in 0..w2 {
            for c in 0..3 {
                let v = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .map(|(dx, dy)| u32::from(px[((2 * y + dy) * w + 2 * x + dx) * 4 + c]));
                out[(y * w2 + x) * 4 + c] = if largest {
                    v.into_iter().max().unwrap_or(0) as u8
                } else {
                    ((v.iter().sum::<u32>() + 2) / 4) as u8
                };
            }
        }
    }
    (w2 as u32, h2 as u32, out)
}

struct FrameInfo {
    name: String,
    group: String,
    scene: String,
    time: String,
    about: Option<String>,
}

fn frame_row(info: FrameInfo, frames: &Path, limit: f64) -> Row {
    let FrameInfo {
        name,
        group,
        scene,
        time,
        about,
    } = info;
    let file = |side: &str, ext: &str| frames.join(format!("{name}.{side}.{ext}"));
    let rust_layout: Value = serde_json::from_slice(&read(&file("rust", "json"))).unwrap();
    let ts_layout: Value = serde_json::from_slice(&read(&file("ts", "json"))).unwrap();
    let (layout_max, mismatches) = layout_diff(&rust_layout, &ts_layout, limit);
    let (rpng, tpng) = (read(&file("rust", "png")), read(&file("ts", "png")));
    let (rw, rh, rpx) = decode_rgba(&rpng).expect("rust png");
    let (tw, th, tpx) = decode_rgba(&tpng).expect("ts png");
    assert_eq!((rw, rh), (tw, th), "{name}: sizes differ");
    let (w, h) = (rw as usize, rh as usize);
    let boxes = text_boxes(&rust_layout);
    let masked = |x: usize, y: usize| {
        boxes.iter().any(|b| {
            (x as f64) >= b[0] && (x as f64) <= b[2] && (y as f64) >= b[1] && (y as f64) <= b[3]
        })
    };
    let (mut off_all, mut off_out, mut open, mut sq) = (0u32, 0u32, 0u32, 0f64);
    // |difference| x4 on opaque black.
    let mut diff = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let mut worst = 0;
            for c in 0..3 {
                let d = (i32::from(rpx[i + c]) - i32::from(tpx[i + c])).abs();
                worst = worst.max(d);
                sq += f64::from(d * d);
                diff[i + c] = (d * 4).min(255) as u8;
            }
            let outside = !masked(x, y);
            open += u32::from(outside);
            if worst > PIXEL_TOLERANCE {
                off_all += 1;
                off_out += u32::from(outside);
            }
        }
    }
    let mse = sq / (w * h * 3) as f64;
    let thumb = |px: &[u8], largest: bool| {
        let (tw, th, t) = halve(px, w, h, largest);
        encode_png(tw, th, &t).unwrap()
    };
    let thumbs = [thumb(&tpx, false), thumb(&rpx, false), thumb(&diff, true)];
    Row {
        name,
        group,
        scene,
        time,
        about,
        layout_max,
        mismatches,
        ssim: ssim(&luma(&rpx), &luma(&tpx), w, h),
        psnr: if mse == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0f64 * 255.0 / mse).log10()
        },
        off_outside: 100.0 * f64::from(off_out) / f64::from(open.max(1)),
        off_all: 100.0 * f64::from(off_all) / (w * h) as f64,
        missing: rust_layout["missing_glyphs"]
            .as_u64()
            .expect("the Rust layout counts missing glyphs"),
        images: [tpng, rpng, encode_png(rw, rh, &diff).unwrap()],
        thumbs,
    }
}

fn markdown(rows: &[Row], t: &Thresholds) -> String {
    let mut report = String::from(
        "| frame | layout max diff (px) | SSIM | PSNR (dB) | pixels off >8/255 outside text | pixels off >8/255 all | missing glyphs |\n|---|---|---|---|---|---|---|\n",
    );
    for r in rows {
        writeln!(
            report,
            "| {}{} | {:.3} | {:.4} | {:.1} | {:.3} % | {:.3} % | {} |",
            r.name,
            if !r.ok(t) {
                " **FAIL**"
            } else if r.is_accepted(t) {
                " accepted"
            } else {
                ""
            },
            r.layout_max,
            r.ssim,
            r.psnr,
            r.off_outside,
            r.off_all,
            r.missing
        )
        .unwrap();
    }
    writeln!(
        report,
        "\nThresholds: layout <= {} px, SSIM >= {}, off outside text <= {} %, missing glyphs 0 ({} in all).",
        t.layout,
        t.ssim,
        t.off_percent,
        rows.iter().map(|r| r.missing).sum::<u64>()
    )
    .unwrap();
    let (pass, accepted, fail) = counts(rows, t);
    writeln!(report, "\n{pass} pass, {accepted} accepted, {fail} fail.").unwrap();
    for a in &t.accepted {
        if let Some(r) = rows
            .iter()
            .find(|r| r.name == a.frame && !r.clean(a.check, t))
        {
            writeln!(
                report,
                "- accepted ({}): {}, {}, limit {}, now {:.4}; {}",
                a.accepted,
                a.frame,
                a.check.label(),
                a.limit,
                r.value(a.check),
                a.reason
            )
            .unwrap();
        }
    }
    for a in stale(rows, t) {
        writeln!(
            report,
            "- STALE: {} {} no longer fails; remove its entry from accepted.json",
            a.frame,
            a.check.label()
        )
        .unwrap();
    }
    report
}

/// Frames that pass outright, pass on an accepted difference, and fail.
fn counts(rows: &[Row], t: &Thresholds) -> (usize, usize, usize) {
    counts_of(&rows.iter().collect::<Vec<_>>(), t)
}

fn counts_of(rows: &[&Row], t: &Thresholds) -> (usize, usize, usize) {
    let fail = rows.iter().filter(|r| !r.ok(t)).count();
    let accepted = rows.iter().filter(|r| r.is_accepted(t)).count();
    (rows.len() - fail - accepted, accepted, fail)
}

const CSS: &str = r#":root{--bg:#f6f7f9;--card:#fff;--text:#1b1f24;--muted:#5b6571;--line:#d9dee5;--ok:#1a7f37;--ok-bg:#dcf5e3;--bad:#c4281c;--bad-bg:#fde4e1;--warn:#8a5a00;--warn-bg:#fff0cc;--accent:#1971c2}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--bg:#14171b;--card:#1d2127;--text:#e6e9ee;--muted:#9aa4b0;--line:#323941;--ok:#56d364;--ok-bg:#16301f;--bad:#ff7b72;--bad-bg:#3a1b19;--warn:#ffc552;--warn-bg:#3a2c0c;--accent:#4dabf7}}
:root[data-theme="dark"]{--bg:#14171b;--card:#1d2127;--text:#e6e9ee;--muted:#9aa4b0;--line:#323941;--ok:#56d364;--ok-bg:#16301f;--bad:#ff7b72;--bad-bg:#3a1b19;--warn:#ffc552;--warn-bg:#3a2c0c;--accent:#4dabf7}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--text);font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1200px;margin:0 auto;padding:16px}
h1{font-size:1.5rem;margin:.5rem 0}h2{font-size:1.15rem;margin:0 0 .5rem}h3{font-size:1rem;margin:0 0 .25rem;overflow-wrap:anywhere}
p,li,dd{overflow-wrap:anywhere}
a{color:var(--accent)}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:16px;margin:16px 0}
.card.fail{border-color:var(--bad)}
.card.accepted{border-color:var(--warn)}
.group{margin:28px 0 0;padding-top:4px;border-top:2px solid var(--line)}
.group h2{font-size:1.3rem}
.meta{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,16rem),1fr));gap:8px 24px;margin:0}
.meta dt{color:var(--muted);font-size:.85rem}.meta dd{margin:0 0 .5rem}
.checks{display:flex;flex-wrap:wrap;gap:8px;margin:.5rem 0}
.chip{border-radius:999px;padding:2px 10px;font-size:.85rem;border:1px solid var(--line);text-decoration:none}
.ok{color:var(--ok);background:var(--ok-bg);border-color:var(--ok)}.warn{color:var(--warn);background:var(--warn-bg);border-color:var(--warn)}.bad{color:var(--bad);background:var(--bad-bg);border-color:var(--bad)}
table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}
th,td{text-align:left;padding:4px 10px 4px 0;border-bottom:1px solid var(--line);vertical-align:top}
td.n,th.n{text-align:right;padding-right:16px}
.tablewrap{overflow-x:auto}
.shots{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,17rem),1fr));gap:12px}
figure{margin:0;min-width:0}figcaption{font-size:.85rem;color:var(--muted);margin-bottom:4px}
.pic{overflow:auto;border:1px solid var(--line);border-radius:6px;background:#000;max-height:80vh}
.pic img{display:block;width:100%;height:auto;cursor:zoom-in}
.pic.zoom img{width:auto;max-width:none;cursor:zoom-out}
.nums{display:flex;flex-wrap:wrap;gap:4px 20px;margin:.75rem 0;font-variant-numeric:tabular-nums}
.nums span{white-space:nowrap}
ul.mm{margin:.25rem 0 0;padding-left:1.2rem}
.hint{color:var(--muted);font-size:.85rem}
details summary{cursor:pointer;font-weight:600}
code{font-size:.9em}"#;

const JS: &str = r#"document.addEventListener('click',function(e){var i=e.target;var p=i.closest&&i.closest('.pic');if(!p||i.tagName!=='IMG')return;if(i.dataset.full&&!i.dataset.on){i.dataset.thumb=i.src;i.src=i.dataset.full;i.dataset.on='1';p.classList.add('zoom')}else if(i.dataset.on){i.src=i.dataset.thumb;delete i.dataset.on;p.classList.remove('zoom')}else{p.classList.toggle('zoom')}});"#;

fn chip(state: State, label: &str) -> String {
    let (class, word) = match state {
        State::Pass => ("ok", "pass"),
        State::Accepted => ("warn", "accepted"),
        State::Fail => ("bad", "FAIL"),
    };
    format!("<span class=\"chip {class}\">{word} {label}</span>")
}

/// The worst state of a check over some rows.
fn worst(states: impl Iterator<Item = State>) -> State {
    states.fold(State::Pass, |a, b| match (a, b) {
        (State::Fail, _) | (_, State::Fail) => State::Fail,
        (State::Accepted, _) | (_, State::Accepted) => State::Accepted,
        _ => State::Pass,
    })
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// What each metric means and where the thresholds sit.
fn how_to_read(t: &Thresholds) -> String {
    format!(
        "<section class=\"card\"><details open><summary>How to read this</summary>\
<p>Every frame is drawn twice from the same board log: by the web player's own drawing code in headless Chromium (the reference, \"TS\"), and by the Rust renderer. Each frame is compared three ways, and passes only if all four hold:</p>\
<ul>\
<li><b>Layout</b> (at most {} px): the numbers behind the picture, written by both sides from the same functions: the camera (x, y, zoom), every element's bounding box and every text line's measured width. A line of text must also be the same text. This catches a glyph or an advance that is off by a fraction of a pixel, which a picture can hide.</li>\
<li><b>SSIM</b> (at least {}): structural similarity of the two pictures' brightness over 8x8 windows, from 1 (identical) down; it notices a missing, moved or reshaped mark whatever its colour.</li>\
<li><b>Pixels off</b> (at most {} % outside text): the share of pixels where any colour channel differs by more than 8 of 255. The text elements' boxes (plus 3 px) are left out of this count, because two rasterisers anti-alias glyph edges differently; text is held to the layout number and to SSIM. \"All\" counts every pixel and is shown for information.</li>\
<li><b>Missing glyphs</b> (none allowed): the Rust side counts the glyphs of the frame's text that came out as a font's missing-glyph box (a character none of the fonts has), a rectangle a viewer would read as a broken video. The web side has no such count: the browser would take such a character from a system font.</li>\
<li><b>PSNR</b> (dB, no threshold): the mean squared error in decibels, higher is closer; infinite means identical.</li>\
</ul>\
<p><b>Accepted differences.</b> A difference the owner has accepted is listed in <code>parity/accepted.json</code> with a limit, the worst value accepted. Its frame passes that check as \"accepted\" (amber) while it stays within the limit and fails if it gets worse; every other check and frame is held to the thresholds above. An entry whose frame passes cleanly is stale and fails the run, so the list never hides a fixed case.</p>\
<p>The difference picture is |TS - Rust| x 4 on black: black is identical, anything lit is a difference. A thumbnail is half size (its difference keeps the largest value of each 2x2 block); tap a picture for the full-size one, where the report has room for it.</p>\
<p><b>The reference browser.</b> Chromium is started with <code>--font-render-hinting=none --enable-font-subpixel-positioning</code>, which is how a desktop browser shows the board (and the player): glyphs advance by fractions of a pixel. Headless Linux Chromium without those flags, which is how today's videos are made, rounds every glyph's advance to a whole pixel, so the text of today's videos can sit up to about 10 px off the player's. The Rust renderer follows the player, not today's videos.</p>\
<p><b>Scripts and emoji outside the board's fonts.</b> The web player draws Chinese, Japanese, Korean, Thai, Lao, Khmer, Arabic, Hebrew, Hindi and emoji with the viewer's system font, which differs from device to device; the video has no system fonts, so the Rust renderer draws them from Noto (listed below: a fallback chain behind the board's six faces, used only for characters none of those has, so everything the board fonts cover is unchanged). The glyph shapes of these scripts in the video can therefore look different from a given device's, while their placement (where each run starts, its width, the line height, which characters stay together) follows the same rules. To compare them like for like, Chromium here is given the same Noto files, appended to the board's font stack as <code>@font-face</code> families; that appended stack exists only in this check, not in the player. Han characters come from the Simplified Chinese face of Noto Sans CJK, on both sides. Japanese and Chinese lines also follow Chromium's <code>text-spacing-trim</code> (adjacent full-width punctuation loses half its width) and its kana kerning, which the Rust side reproduces with <code>halt</code> font features. A run on macOS (a developer's machine) cannot draw Noto Color Emoji, because Skia's macOS font manager cannot; Linux, and so production, can.</p>\
<p><b>What is not compared.</b> Graphs, solids and symbols (pack elements) are not drawn by the Rust renderer yet and are not in the corpus. Only the light theme at density 1 is drawn, as the video is.</p>\
</details></section>",
        t.layout, t.ssim, t.off_percent
    )
}

/// The "Accepted differences" section: each entry in use, with its reason, limit and today's value.
fn accepted_section(rows: &[Row], t: &Thresholds) -> String {
    let mut s = String::from("<section class=\"card\"><h2>Accepted differences</h2>");
    if t.accepted.is_empty() {
        s.push_str("<p class=\"hint\">None on this platform.</p></section>");
        return s;
    }
    s.push_str("<p class=\"hint\">Differences the owner accepted (<code>parity/accepted.json</code>). A frame passes its check as accepted while it stays within the limit, and fails if it gets worse; an entry whose frame passes cleanly is stale and fails the run.</p><div class=\"tablewrap\"><table><thead><tr><th>Frame</th><th>Check</th><th>Reason</th><th class=\"n\">Limit</th><th class=\"n\">Today</th><th>Accepted</th></tr></thead><tbody>");
    for a in &t.accepted {
        let row = rows.iter().find(|r| r.name == a.frame);
        let (today, state) = row.map_or(("not in the corpus".to_owned(), State::Fail), |r| {
            (
                format!("{:.4}", r.value(a.check)),
                if r.clean(a.check, t) {
                    State::Fail
                } else {
                    r.state(a.check, t)
                },
            )
        });
        write!(
            s,
            "<tr><td><a href=\"#f-{}\">{}</a></td><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\"><span class=\"chip {}\">{}{}</span></td><td>{}</td></tr>",
            slug(&a.frame),
            escape(&a.frame),
            a.check.label(),
            escape(&a.reason),
            a.limit,
            if state == State::Accepted { "warn" } else { "bad" },
            today,
            if row.is_some_and(|r| r.clean(a.check, t)) { " (stale)" } else { "" },
            escape(&a.accepted)
        )
        .unwrap();
    }
    s.push_str("</tbody></table></div></section>");
    s
}

fn html(
    rows: &[Row],
    t: &Thresholds,
    frames: &Path,
    replay_commit: &str,
    groups: &[String],
    budget: usize,
) -> String {
    let browser =
        std::fs::read_to_string(frames.join("browser.txt")).unwrap_or_else(|_| "unknown".into());
    let stale = stale(rows, t);
    let (n_pass, n_accepted, n_fail) = counts(rows, t);
    let all_ok = n_fail == 0 && stale.is_empty();
    let count = |f: &dyn Fn(&Row) -> bool| rows.iter().filter(|r| f(r)).count();
    let scenes = {
        let mut names: Vec<&str> = rows.iter().map(|r| r.scene.as_str()).collect();
        names.dedup();
        names.len()
    };

    // Full-size pictures for as many frames as the budget has room for, the failing and the least
    // alike first.
    let thumbs: usize = rows
        .iter()
        .flat_map(|r| r.thumbs.iter())
        .map(|b| b.len() * 4 / 3)
        .sum();
    let mut room = budget.saturating_sub(thumbs + 400_000);
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        (rows[a].ok(t), rows[a].ssim)
            .partial_cmp(&(rows[b].ok(t), rows[b].ssim))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut full = vec![false; rows.len()];
    for i in order {
        let size: usize = rows[i].images.iter().map(|b| b.len() * 4 / 3).sum();
        if size <= room {
            room -= size;
            full[i] = true;
        }
    }
    let with_full = full.iter().filter(|f| **f).count();

    let check_chips: String = CHECKS
        .iter()
        .map(|&c| {
            let states = || rows.iter().map(move |r| r.state(c, t));
            let state = worst(states());
            let okay = states().filter(|&x| x != State::Fail).count();
            let accepted = states().filter(|&x| x == State::Accepted).count();
            chip(
                state,
                &format!(
                    "{} {okay}/{}{}",
                    c.label(),
                    rows.len(),
                    if accepted > 0 {
                        format!(" ({accepted} accepted)")
                    } else {
                        String::new()
                    }
                ),
            )
        })
        .chain(std::iter::once(chip(
            if rows.iter().all(Row::glyphs_ok) {
                State::Pass
            } else {
                State::Fail
            },
            &format!(
                "no missing glyphs {}/{} ({} missing in all)",
                count(&|r| r.glyphs_ok()),
                rows.len(),
                rows.iter().map(|r| r.missing).sum::<u64>()
            ),
        )))
        .collect();

    let mut s = String::new();
    write!(
        s,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Render parity report</title><style>{CSS}</style></head><body><main><h1>Render parity report</h1>\
<p class=\"hint\">The web player's board drawing in Chromium (reference) against the Rust renderer, on a corpus of {} frames from {} scenes, each named for what it tests.</p>",
        rows.len(),
        scenes
    )
    .unwrap();
    write!(
        s,
        "<section class=\"card {}\"><h2>Summary</h2><div class=\"checks\">{}{}</div>",
        if all_ok { "" } else { "fail" },
        chip(
            if !all_ok {
                State::Fail
            } else if n_accepted > 0 {
                State::Accepted
            } else {
                State::Pass
            },
            &format!(
                "overall: {n_pass} pass, {n_accepted} accepted, {n_fail} fail of {}",
                rows.len()
            )
        ),
        check_chips,
    )
    .unwrap();
    s.push_str("<div class=\"tablewrap\"><table><thead><tr><th>Group</th><th class=\"n\">Scenes</th><th class=\"n\">Frames</th><th class=\"n\">Pass</th><th class=\"n\">Accepted</th><th class=\"n\">Fail</th><th class=\"n\">Missing glyphs</th></tr></thead><tbody>");
    for g in groups {
        let rs: Vec<&Row> = rows.iter().filter(|r| &r.group == g).collect();
        let (pass, accepted, fails) = counts_of(&rs, t);
        let mut names: Vec<&str> = rs.iter().map(|r| r.scene.as_str()).collect();
        names.dedup();
        write!(
            s,
            "<tr><td><a href=\"#g-{}\">{}</a></td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
            slug(g),
            escape(g),
            names.len(),
            rs.len(),
            pass,
            if accepted == 0 {
                "0".to_owned()
            } else {
                format!("<span class=\"chip warn\">{accepted}</span>")
            },
            if fails == 0 {
                "0".to_owned()
            } else {
                format!("<span class=\"chip bad\">{fails}</span>")
            },
            {
                let missing: u64 = rs.iter().map(|r| r.missing).sum();
                if missing == 0 {
                    "0".to_owned()
                } else {
                    format!("<span class=\"chip bad\">{missing}</span>")
                }
            }
        )
        .unwrap();
    }
    s.push_str("</tbody></table></div>");
    let failing: Vec<&Row> = rows.iter().filter(|r| !r.ok(t)).collect();
    if !failing.is_empty() {
        s.push_str("<p>Failing frames:</p><ul class=\"mm\">");
        for r in &failing {
            write!(
                s,
                "<li><a href=\"#f-{}\">{}</a></li>",
                slug(&r.name),
                escape(&r.name)
            )
            .unwrap();
        }
        s.push_str("</ul>");
    }
    if !stale.is_empty() {
        s.push_str("<p>Stale entries in <code>accepted.json</code> (the frame passes cleanly now; remove the entry):</p><ul class=\"mm\">");
        for a in &stale {
            write!(
                s,
                "<li><a href=\"#f-{}\">{}</a>, {}</li>",
                slug(&a.frame),
                escape(&a.frame),
                a.check.label()
            )
            .unwrap();
        }
        s.push_str("</ul>");
    }
    write!(
        s,
        "<dl class=\"meta\"><div><dt>Thresholds</dt><dd>layout: at most {} px; SSIM: at least {}; pixels off by more than 8/255 outside text: at most {} %</dd></div>\
<div><dt>Platform</dt><dd>{} {}</dd></div><div><dt>Reference browser</dt><dd>{}</dd></div>\
<div><dt>lesson-replay commit</dt><dd>{}</dd></div><div><dt>Font files (stack order)</dt><dd>{}</dd></div><div><dt>Fallback font files (after those)</dt><dd>{}</dd></div>\
<div><dt>Pictures</dt><dd>Thumbnails for every frame; full size on tap for {} of {} frames (the failing and the lowest SSIM first, as the report's size allows)</dd></div></dl></section>",
        t.layout,
        t.ssim,
        t.off_percent,
        std::env::consts::OS,
        std::env::consts::ARCH,
        escape(browser.trim()),
        escape(replay_commit),
        BOARD_FONT_FILES.iter().map(|f| escape(f)).collect::<Vec<_>>().join(", "),
        FALLBACK_FONT_FILES.iter().map(|f| escape(f.name)).collect::<Vec<_>>().join(", "),
        with_full,
        rows.len(),
    )
    .unwrap();
    s.push_str(&how_to_read(t));
    s.push_str(&accepted_section(rows, t));

    for g in groups {
        write!(
            s,
            "<div class=\"group\" id=\"g-{}\"><h2>{}</h2></div>",
            slug(g),
            escape(g)
        )
        .unwrap();
        for (i, r) in rows.iter().enumerate().filter(|(_, r)| &r.group == g) {
            let img = |label: &str, k: usize| {
                let src = |bytes: &[u8]| format!("data:image/png;base64,{}", base64(bytes));
                format!(
                    "<figure><figcaption>{label}</figcaption><div class=\"pic\"><img alt=\"{label}, {}\" src=\"{}\"{}></div></figure>",
                    escape(&r.name),
                    src(&r.thumbs[k]),
                    if full[i] {
                        format!(" data-full=\"{}\"", src(&r.images[k]))
                    } else {
                        String::new()
                    }
                )
            };
            write!(
                s,
                "<section class=\"card {}\" id=\"f-{}\"><h3>{} <span class=\"hint\">at {} ms</span></h3>",
                if !r.ok(t) {
                    "fail"
                } else if r.is_accepted(t) {
                    "accepted"
                } else {
                    ""
                },
                slug(&r.name),
                escape(&r.scene),
                escape(&r.time)
            )
            .unwrap();
            if let Some(about) = &r.about {
                write!(s, "<p class=\"hint\">{}</p>", escape(about)).unwrap();
            }
            for a in t.accepted.iter().filter(|a| a.frame == r.name) {
                write!(
                    s,
                    "<p><span class=\"chip warn\">accepted {}</span> {} <span class=\"hint\">({})</span></p>",
                    a.check.label(),
                    escape(&a.reason),
                    escape(&a.accepted)
                )
                .unwrap();
            }
            write!(
                s,
                "<div class=\"shots\">{}{}{}</div>\
<div class=\"nums\"><span>SSIM {}</span><span>PSNR {} dB</span><span>off &gt;8/255, all: {:.3} %</span><span>off &gt;8/255, outside text: {:.3} %</span><span>layout max error {:.3} px</span><span>missing glyphs {}</span></div>\
<div class=\"checks\">{}{}{}{}</div>",
                img("TS (Chromium)", 0),
                img("Rust", 1),
                img("Difference", 2),
                format_args!("{:.4}", r.ssim),
                if r.psnr.is_finite() { format!("{:.1}", r.psnr) } else { "infinite".into() },
                r.off_all,
                r.off_outside,
                r.layout_max,
                r.missing,
                chip(r.state(Check::Layout, t), "layout"),
                chip(r.state(Check::Ssim, t), "SSIM"),
                chip(r.state(Check::Pixels, t), "pixels"),
                chip(if r.glyphs_ok() { State::Pass } else { State::Fail }, "glyphs"),
            )
            .unwrap();
            if !r.mismatches.is_empty() {
                write!(
                    s,
                    "<p>Layout mismatches (over {} px, or not comparable):</p><ul class=\"mm\">",
                    t.layout
                )
                .unwrap();
                for m in &r.mismatches {
                    write!(
                        s,
                        "<li><b>{}</b>, {}: TS {} / Rust {}{}</li>",
                        escape(&m.who),
                        escape(&m.field),
                        escape(&m.ts),
                        escape(&m.rust),
                        if m.diff.is_finite() {
                            format!(" (off by {:.3} px)", m.diff)
                        } else {
                            String::new()
                        }
                    )
                    .unwrap();
                }
                s.push_str("</ul>");
            }
            s.push_str("</section>");
        }
    }
    write!(s, "</main><script>{JS}</script></body></html>").unwrap();
    s
}

fn main() -> ExitCode {
    let corpus = PathBuf::from(support::required("--corpus"));
    let frames = PathBuf::from(support::required("--frames"));
    let float =
        |name: &str, default: f64| support::opt(name).map_or(default, |v| v.parse().expect(name));
    let t = Thresholds {
        layout: float("--max-layout", 0.5),
        ssim: float("--min-ssim", 0.97),
        off_percent: float("--max-off-percent", 0.5),
        accepted: load_accepted(&support::opt("--accepted").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("parity/accepted.json"),
            PathBuf::from,
        )),
    };
    let mut rows = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    // The board's corpus, and the corpora of other stages (`--extra-corpus`, the quiz's) beside it.
    let dirs: Vec<PathBuf> = std::iter::once(corpus)
        .chain(support::opt("--extra-corpus").map(PathBuf::from))
        .collect();
    for dir in dirs {
        let index: Value = serde_json::from_slice(&read(&dir.join("index.json"))).unwrap();
        for scene in index["scenes"].as_array().unwrap() {
            let name = scene["name"].as_str().unwrap();
            for (i, time) in scene["times"].as_array().unwrap().iter().enumerate() {
                rows.push(frame_row(
                    FrameInfo {
                        name: format!("{name}-{time}"),
                        group: scene["group"].as_str().unwrap().to_owned(),
                        scene: name.to_owned(),
                        time: time.to_string(),
                        about: (i == 0).then(|| scene["about"].as_str().unwrap().to_owned()),
                    },
                    &frames,
                    t.layout,
                ));
            }
        }
        for g in index["groups"].as_array().unwrap() {
            let g = g.as_str().unwrap().to_owned();
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
    }
    let report = markdown(&rows, &t);
    print!("{report}");
    if let Some(path) = support::opt("--report") {
        std::fs::write(path, &report).unwrap();
    }
    if let Some(path) = support::opt("--html") {
        let commit = support::opt("--replay-commit").unwrap_or_else(|| "unknown".into());
        let budget = (float("--max-mb", 14.0) * 1.0e6) as usize;
        std::fs::write(path, html(&rows, &t, &frames, &commit, &groups, budget)).unwrap();
    }
    ExitCode::from(u8::from(
        !rows.iter().all(|r| r.ok(&t)) || !stale(&rows, &t).is_empty(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(layout_max: f64, ssim: f64) -> Row {
        Row {
            name: "frame".into(),
            group: String::new(),
            scene: String::new(),
            time: String::new(),
            about: None,
            layout_max,
            mismatches: Vec::new(),
            ssim,
            psnr: 30.0,
            off_outside: 0.0,
            off_all: 0.0,
            missing: 0,
            images: Default::default(),
            thumbs: Default::default(),
        }
    }

    fn thresholds(check: Check, limit: f64) -> Thresholds {
        Thresholds {
            layout: 0.5,
            ssim: 0.97,
            off_percent: 0.5,
            accepted: vec![Accepted {
                frame: "frame".into(),
                check,
                limit,
                reason: "because".into(),
                accepted: "owner".into(),
            }],
        }
    }

    #[test]
    fn a_frame_within_its_limit_is_accepted_and_only_for_that_check() {
        let t = thresholds(Check::Layout, 14.5);
        let r = row(14.0, 0.99);
        assert_eq!(r.state(Check::Layout, &t), State::Accepted);
        assert!(r.ok(&t) && r.is_accepted(&t));
        assert!(stale(&[row(14.0, 0.99)], &t).is_empty());
        // Another check of the same frame keeps the global threshold.
        assert_eq!(row(14.0, 0.9).state(Check::Ssim, &t), State::Fail);
    }

    #[test]
    fn a_worse_value_fails() {
        let t = thresholds(Check::Layout, 14.5);
        let r = row(14.6, 0.99);
        assert_eq!(r.state(Check::Layout, &t), State::Fail);
        assert!(!r.ok(&t));
        let t = thresholds(Check::Ssim, 0.9653);
        assert_eq!(row(0.0, 0.9654).state(Check::Ssim, &t), State::Accepted);
        assert_eq!(row(0.0, 0.9652).state(Check::Ssim, &t), State::Fail);
    }

    #[test]
    fn a_passing_frame_with_an_entry_is_stale() {
        let t = thresholds(Check::Layout, 14.5);
        let rows = [row(0.2, 0.99)];
        assert_eq!(rows[0].state(Check::Layout, &t), State::Pass);
        assert_eq!(stale(&rows, &t).len(), 1);
        assert_eq!(stale(&[], &t).len(), 1);
    }
}
