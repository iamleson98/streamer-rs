//! Compares the Rust frames with the web player's: the layout dump (text lines, measured widths,
//! element bounds, the camera) and the pixels (SSIM, PSNR, and the share of pixels that differ by
//! more than 8/255, overall and outside the text boxes). Writes a Markdown summary and a
//! self-contained HTML report with the three pictures of every frame, and exits with 1 when a
//! threshold is broken.
//!
//!   cargo run -p board-draw --example compare --locked -- --corpus DIR --frames DIR
//!       [--report FILE.md] [--html FILE.html] [--replay-commit SHA]
//!       [--max-layout PX] [--min-ssim X] [--max-off-percent P]

mod support;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use board_draw::{BOARD_FONT_FILES, decode_rgba, encode_png};
use serde_json::Value;

/// The pixel difference that counts, out of 255.
const PIXEL_TOLERANCE: i32 = 8;
/// Room around a text element's box that is ignored in the pixel count (its anti-aliased edge).
const TEXT_PAD: f64 = 3.0;

struct Thresholds {
    layout: f64,
    ssim: f64,
    off_percent: f64,
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
    layout_max: f64,
    mismatches: Vec<Mismatch>,
    ssim: f64,
    psnr: f64,
    off_outside: f64,
    off_all: f64,
    images: [Vec<u8>; 3],
}

impl Row {
    fn layout_ok(&self, t: &Thresholds) -> bool {
        self.layout_max <= t.layout && self.mismatches.iter().all(|m| m.diff.is_finite())
    }
    fn ssim_ok(&self, t: &Thresholds) -> bool {
        self.ssim >= t.ssim
    }
    fn pixels_ok(&self, t: &Thresholds) -> bool {
        self.off_outside <= t.off_percent
    }
    fn ok(&self, t: &Thresholds) -> bool {
        self.layout_ok(t) && self.ssim_ok(t) && self.pixels_ok(t)
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

/// Every number of the two layout dumps, side by side; `max` is the largest difference.
fn layout_diff(rust: &Value, ts: &Value, limit: f64) -> (f64, Vec<Mismatch>) {
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

fn frame_row(name: String, frames: &Path, limit: f64) -> Row {
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
    Row {
        name,
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
        images: [tpng, rpng, encode_png(rw, rh, &diff).unwrap()],
    }
}

fn markdown(rows: &[Row], t: &Thresholds) -> String {
    let mut report = String::from(
        "| frame | layout max diff (px) | SSIM | PSNR (dB) | pixels off >8/255 outside text | pixels off >8/255 all |\n|---|---|---|---|---|---|\n",
    );
    for r in rows {
        writeln!(
            report,
            "| {}{} | {:.3} | {:.4} | {:.1} | {:.3} % | {:.3} % |",
            r.name,
            if r.ok(t) { "" } else { " **FAIL**" },
            r.layout_max,
            r.ssim,
            r.psnr,
            r.off_outside,
            r.off_all
        )
        .unwrap();
    }
    writeln!(
        report,
        "\nThresholds: layout <= {} px, SSIM >= {}, off outside text <= {} %.",
        t.layout, t.ssim, t.off_percent
    )
    .unwrap();
    report
}

const CSS: &str = r#":root{--bg:#f6f7f9;--card:#fff;--text:#1b1f24;--muted:#5b6571;--line:#d9dee5;--ok:#1a7f37;--ok-bg:#dcf5e3;--bad:#c4281c;--bad-bg:#fde4e1;--accent:#1971c2}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--bg:#14171b;--card:#1d2127;--text:#e6e9ee;--muted:#9aa4b0;--line:#323941;--ok:#56d364;--ok-bg:#16301f;--bad:#ff7b72;--bad-bg:#3a1b19;--accent:#4dabf7}}
:root[data-theme="dark"]{--bg:#14171b;--card:#1d2127;--text:#e6e9ee;--muted:#9aa4b0;--line:#323941;--ok:#56d364;--ok-bg:#16301f;--bad:#ff7b72;--bad-bg:#3a1b19;--accent:#4dabf7}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--text);font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1200px;margin:0 auto;padding:16px}
h1{font-size:1.5rem;margin:.5rem 0}h2{font-size:1.15rem;margin:0 0 .5rem}
p,li{overflow-wrap:anywhere}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:16px;margin:16px 0}
.card.fail{border-color:var(--bad)}
.meta{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,16rem),1fr));gap:8px 24px;margin:0}
.meta dt{color:var(--muted);font-size:.85rem}.meta dd{margin:0 0 .5rem;overflow-wrap:anywhere}
.checks{display:flex;flex-wrap:wrap;gap:8px;margin:.5rem 0}
.chip{border-radius:999px;padding:2px 10px;font-size:.85rem;border:1px solid var(--line)}
.ok{color:var(--ok);background:var(--ok-bg);border-color:var(--ok)}.bad{color:var(--bad);background:var(--bad-bg);border-color:var(--bad)}
.shots{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,17rem),1fr));gap:12px}
figure{margin:0;min-width:0}figcaption{font-size:.85rem;color:var(--muted);margin-bottom:4px}
.pic{overflow:auto;border:1px solid var(--line);border-radius:6px;background:#000;max-height:80vh}
.pic img{display:block;width:100%;height:auto;cursor:zoom-in}
.pic.zoom img{width:auto;max-width:none;cursor:zoom-out}
.nums{display:flex;flex-wrap:wrap;gap:4px 20px;margin:.75rem 0;font-variant-numeric:tabular-nums}
.nums span{white-space:nowrap}.nums b.bad,.nums b.ok{background:none;border:0}
ul.mm{margin:.25rem 0 0;padding-left:1.2rem}
.hint{color:var(--muted);font-size:.85rem}"#;

const JS: &str = r#"document.addEventListener('click',function(e){var p=e.target.closest('.pic');if(p&&e.target.tagName==='IMG')p.classList.toggle('zoom')});"#;

fn chip(ok: bool, label: &str) -> String {
    format!(
        "<span class=\"chip {}\">{} {}</span>",
        if ok { "ok" } else { "bad" },
        if ok { "pass" } else { "FAIL" },
        label
    )
}

fn html(rows: &[Row], t: &Thresholds, frames: &Path, replay_commit: &str, scenes: usize) -> String {
    let count = |f: &dyn Fn(&Row) -> bool| rows.iter().filter(|r| f(r)).count();
    let failing_scenes = {
        let mut names: Vec<&str> = rows
            .iter()
            .filter(|r| !r.ok(t))
            .map(|r| r.name.rsplit_once('-').unwrap().0)
            .collect();
        names.dedup();
        names.len()
    };
    let browser =
        std::fs::read_to_string(frames.join("browser.txt")).unwrap_or_else(|_| "unknown".into());
    let mut s = String::new();
    write!(
        s,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Render parity report</title><style>{CSS}</style></head><body><main><h1>Render parity report</h1>\
<p class=\"hint\">The web player's board drawing in Chromium (reference) against the Rust renderer, on a synthetic corpus of board logs.</p>"
    )
    .unwrap();
    let all_ok = rows.iter().all(|r| r.ok(t));
    write!(
        s,
        "<section class=\"card {}\"><h2>Summary</h2><div class=\"checks\">{}</div><p>{} scenes ({} with a failing frame), {} frames.</p>\
<div class=\"checks\">{}{}{}</div>\
<dl class=\"meta\"><div><dt>Thresholds</dt><dd>layout: at most {} px; SSIM: at least {}; pixels off by more than 8/255 outside text: at most {} %</dd></div>\
<div><dt>Platform</dt><dd>{} {}</dd></div><div><dt>Reference browser</dt><dd>{}</dd></div>\
<div><dt>lesson-replay commit</dt><dd>{}</dd></div><div><dt>Font files (stack order)</dt><dd>{}</dd></div></dl></section>",
        if all_ok { "" } else { "fail" },
        chip(all_ok, "overall"),
        scenes,
        failing_scenes,
        rows.len(),
        chip(count(&|r| r.layout_ok(t)) == rows.len(), &format!("layout {}/{}", count(&|r| r.layout_ok(t)), rows.len())),
        chip(count(&|r| r.ssim_ok(t)) == rows.len(), &format!("SSIM {}/{}", count(&|r| r.ssim_ok(t)), rows.len())),
        chip(count(&|r| r.pixels_ok(t)) == rows.len(), &format!("pixels {}/{}", count(&|r| r.pixels_ok(t)), rows.len())),
        t.layout,
        t.ssim,
        t.off_percent,
        std::env::consts::OS,
        std::env::consts::ARCH,
        escape(browser.trim()),
        escape(replay_commit),
        BOARD_FONT_FILES.iter().map(|f| escape(f)).collect::<Vec<_>>().join(", "),
    )
    .unwrap();
    for r in rows {
        let [ts, rust, diff] = &r.images;
        let img = |label: &str, bytes: &[u8]| {
            format!(
                "<figure><figcaption>{label}</figcaption><div class=\"pic\"><img alt=\"{label}, {}\" src=\"data:image/png;base64,{}\"></div></figure>",
                escape(&r.name),
                base64(bytes)
            )
        };
        write!(
            s,
            "<section class=\"card {}\"><h2>{}</h2><div class=\"shots\">{}{}{}</div><p class=\"hint\">Tap a picture to see it at full size. The difference is |TS - Rust| x 4 on black.</p>\
<div class=\"nums\"><span>SSIM {}</span><span>PSNR {} dB</span><span>off &gt;8/255, all: {:.3} %</span><span>off &gt;8/255, outside text: {:.3} %</span><span>layout max error {:.3} px</span></div>\
<div class=\"checks\">{}{}{}</div>",
            if r.ok(t) { "" } else { "fail" },
            escape(&r.name),
            img("TS (Chromium)", ts),
            img("Rust", rust),
            img("Difference", diff),
            format_args!("{:.4}", r.ssim),
            if r.psnr.is_finite() { format!("{:.1}", r.psnr) } else { "infinite".into() },
            r.off_all,
            r.off_outside,
            r.layout_max,
            chip(r.layout_ok(t), "layout"),
            chip(r.ssim_ok(t), "SSIM"),
            chip(r.pixels_ok(t), "pixels"),
        )
        .unwrap();
        if r.mismatches.is_empty() {
            write!(
                s,
                "<p class=\"hint\">Layout: every number within {} px.</p>",
                t.layout
            )
            .unwrap();
        } else {
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
    };
    let index: Value = serde_json::from_slice(&read(&corpus.join("index.json"))).unwrap();
    let mut rows = Vec::new();
    let scenes = index["scenes"].as_array().unwrap();
    for scene in scenes {
        for time in scene["times"].as_array().unwrap() {
            rows.push(frame_row(
                format!("{}-{}", scene["name"].as_str().unwrap(), time),
                &frames,
                t.layout,
            ));
        }
    }
    let report = markdown(&rows, &t);
    print!("{report}");
    if let Some(path) = support::opt("--report") {
        std::fs::write(path, &report).unwrap();
    }
    if let Some(path) = support::opt("--html") {
        let commit = support::opt("--replay-commit").unwrap_or_else(|| "unknown".into());
        std::fs::write(path, html(&rows, &t, &frames, &commit, scenes.len())).unwrap();
    }
    ExitCode::from(u8::from(!rows.iter().all(|r| r.ok(&t))))
}
