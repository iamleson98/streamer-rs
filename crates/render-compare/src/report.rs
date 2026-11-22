//! The report: a self-contained HTML page and a short Markdown summary of everything `analyze`
//! measured, grouped by recording, with the gates of the native engine as pass or fail chips.

use std::fmt::Write as _;

use report_page::{State, chip, escape, page, picture, slug};

use crate::analyze::{Analysis, Pair};
use crate::timeline::{Second, Stage};

/// The mean VMAF the native video must reach against the lossless native reference.
pub const MIN_NATIVE_VMAF: f64 = 90.0;
/// How far the sound may be from the picture: one frame.
pub const MAX_AV_SKEW_MS: f64 = 1000.0 / 30.0 + 1.0;
/// The runner wants a keyframe at least every two seconds.
pub const MAX_KEYFRAME_GAP_S: f64 = 2.0 + 0.01;
/// What any 2 seconds of video may carry (kbit/s): the 2.5 Mbit/s cap, plus what its 5 Mbit
/// buffer lets through in a window that short.
pub const MAX_PEAK_2S_KBPS: f64 = 2500.0 + 5000.0 / 2.0;

/// Everything measured for one recording.
#[derive(Debug)]
pub struct ScenarioReport {
    pub name: String,
    pub media_s: f64,
    pub preroll_ms: u64,
    pub native: Result<Analysis, String>,
    /// `Err` says why there is none ("not run" or what failed).
    pub chromium: Result<Analysis, String>,
    pub expected: Vec<Second>,
    pub native_vs_reference: Option<Pair>,
    pub chromium_vs_reference: Option<Pair>,
    pub native_vs_chromium: Option<Pair>,
}

/// One pass or fail the native engine is held to.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

impl ScenarioReport {
    pub fn gates(&self) -> Vec<Gate> {
        let Ok(n) = &self.native else {
            return vec![Gate {
                name: "native render",
                ok: false,
                detail: self.native.as_ref().err().cloned().unwrap_or_default(),
            }];
        };
        let mut gates = vec![
            Gate {
                name: "runner probe",
                ok: n.probe.is_ok(),
                detail: n
                    .probe
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "accepted".into()),
            },
            Gate {
                name: "keyframes every 2 s",
                ok: n.max_keyframe_gap_s <= MAX_KEYFRAME_GAP_S,
                detail: format!("largest gap {:.2} s", n.max_keyframe_gap_s),
            },
            Gate {
                name: "every frame once, on the 30 fps grid",
                ok: n.frames_on_grid,
                detail: format!("{} of {} frames", n.frames, n.frames_expected),
            },
            Gate {
                name: "bitrate within the cap, joins included",
                ok: n.peak_kbps_2s <= MAX_PEAK_2S_KBPS,
                detail: format!(
                    "peak {:.0} kbit/s in 1 s, {:.0} in 2 s",
                    n.peak_kbps_1s, n.peak_kbps_2s
                ),
            },
            Gate {
                name: "length",
                ok: (n.duration_s - (self.media_s + self.preroll_ms as f64 / 1000.0)).abs() < 0.3,
                detail: format!("{:.2} s", n.duration_s),
            },
            Gate {
                name: "timeline matches the manifest",
                ok: n.timeline_mismatches.is_empty(),
                detail: if n.timeline_mismatches.is_empty() {
                    "every second".into()
                } else {
                    format!("seconds {:?} differ", n.timeline_mismatches)
                },
            },
        ];
        if let Some(skew) = n.av.skew_ms() {
            gates.push(Gate {
                name: "A/V offset within one frame",
                ok: skew.abs() <= MAX_AV_SKEW_MS,
                detail: format!("{skew:+.0} ms"),
            });
        }
        if let Some(pair) = &self.native_vs_reference {
            gates.push(Gate {
                name: "VMAF against the lossless reference",
                ok: pair.vmaf.mean >= MIN_NATIVE_VMAF,
                detail: format!(
                    "mean {:.1}, worst second {:.1}",
                    pair.vmaf.mean, pair.vmaf.min
                ),
            });
        }
        gates
    }

    pub fn ok(&self) -> bool {
        self.gates().iter().all(|g| g.ok)
    }
}

fn cell(v: Option<String>) -> String {
    v.unwrap_or_else(|| "–".into())
}

fn row(label: &str, native: Option<String>, chromium: Option<String>) -> String {
    format!(
        "<tr><th>{}</th><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
        escape(label),
        cell(native),
        cell(chromium)
    )
}

fn timeline_row(label: &str, seconds: &[Second], expected: Option<&[Second]>) -> String {
    let mut out = format!("<div><b>{}</b></div><div class=\"tl\">", escape(label));
    for (i, s) in seconds.iter().enumerate() {
        let class = match s.stage {
            Stage::Card => "card",
            Stage::Board => "board",
            Stage::Screen => "screen",
        };
        let off = expected.is_some_and(|e| e.get(i) != Some(s));
        let _ = write!(
            out,
            "<i class=\"{class}{}{}\" title=\"{i} s\"></i>",
            if s.camera { " cam" } else { "" },
            if off { " off" } else { "" }
        );
    }
    out.push_str("</div>");
    out
}

fn sparkline(values: &[f64], low: f64, high: f64) -> String {
    let (w, h) = (300.0, 40.0);
    let n = values.len().max(2) as f64 - 1.0;
    let points: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            format!(
                "{:.1},{:.1}",
                i as f64 / n * w,
                h - ((v - low) / (high - low)).clamp(0.0, 1.0) * h
            )
        })
        .collect();
    format!(
        "<svg viewBox=\"0 0 {w} {h}\" width=\"100%\" height=\"{h}\" preserveAspectRatio=\"none\" role=\"img\"><polyline fill=\"none\" stroke=\"var(--accent)\" stroke-width=\"1.5\" points=\"{}\"/></svg>",
        points.join(" ")
    )
}

fn pair_html(pair: &Pair) -> String {
    let mut out = format!(
        "<div class=\"card\"><h3>{}</h3><div class=\"nums\"><span>VMAF mean <b>{:.1}</b></span><span>worst second <b>{:.1}</b></span><span>SSIM mean <b>{:.4}</b></span><span>worst second <b>{:.4}</b></span></div>",
        escape(&pair.title),
        pair.vmaf.mean,
        pair.vmaf.min,
        pair.ssim.mean,
        pair.ssim.min
    );
    let _ = write!(
        out,
        "<div class=\"hint\">VMAF per second</div>{}",
        sparkline(&pair.vmaf.per_second, 40.0, 100.0)
    );
    if !pair.worst.is_empty() {
        let _ = write!(
            out,
            "<details><summary>The {} worst seconds as pictures</summary><div class=\"shots\">",
            pair.worst.len()
        );
        for shot in &pair.worst {
            let caption =
                |what: &str| format!("{} s: {what} (VMAF {:.1})", shot.second, shot.value);
            out.push_str(&picture("image/jpeg", &shot.a, &caption(&pair.dist_label)));
            out.push_str(&picture(
                "image/jpeg",
                &shot.b,
                &caption(&pair.reference_label),
            ));
            out.push_str(&picture(
                "image/jpeg",
                &shot.diff,
                &caption("difference, brightened"),
            ));
        }
        out.push_str("</div></details>");
    }
    out.push_str("</div>");
    out
}

fn scenario_html(r: &ScenarioReport) -> String {
    let mut out = format!(
        "<section class=\"group\" id=\"{}\"><h2>{}</h2>",
        slug(&r.name),
        escape(&r.name)
    );
    out.push_str("<div class=\"checks\">");
    for g in r.gates() {
        out.push_str(&chip(
            if g.ok { State::Pass } else { State::Fail },
            &format!("{}: {}", g.name, g.detail),
        ));
    }
    out.push_str("</div>");

    let (n, c) = (r.native.as_ref().ok(), r.chromium.as_ref().ok());
    if let Err(why) = &r.chromium {
        let _ = write!(out, "<div class=\"banner\">Chromium: {}</div>", escape(why));
    }
    let f = |v: f64, d: usize| format!("{v:.d$}");
    out.push_str("<div class=\"card tablewrap\"><table><thead><tr><th></th><th class=\"n\">Native</th><th class=\"n\">Chromium</th></tr></thead><tbody>");
    for (label, get) in [
        (
            "Length (s)",
            Box::new(|a: &Analysis| f(a.duration_s, 2)) as Box<dyn Fn(&Analysis) -> String>,
        ),
        (
            "File size (KiB)",
            Box::new(|a| f(a.size_bytes as f64 / 1024.0, 0)),
        ),
        (
            "Bitrate (kbit/s)",
            Box::new(|a| {
                f(
                    a.size_bytes as f64 * 8.0 / 1000.0 / a.duration_s.max(0.001),
                    0,
                )
            }),
        ),
        (
            "Largest keyframe gap (s)",
            Box::new(|a| f(a.max_keyframe_gap_s, 2)),
        ),
        (
            "Runner probe",
            Box::new(|a| {
                if a.probe.is_ok() {
                    "accepted".into()
                } else {
                    format!(
                        "refused: {}",
                        a.probe.as_ref().err().cloned().unwrap_or_default()
                    )
                }
            }),
        ),
        (
            "Loudness (LUFS)",
            Box::new(|a| a.loudness.map_or("–".into(), |l| f(l.integrated_lufs, 1))),
        ),
        (
            "True peak (dBFS)",
            Box::new(|a| a.loudness.map_or("–".into(), |l| f(l.true_peak_dbtp, 1))),
        ),
        (
            "Sound offset (ms)",
            Box::new(|a| a.av.audio_ms.map_or("–".into(), |v| format!("{v:+.0}"))),
        ),
        (
            "Picture offset (ms)",
            Box::new(|a| a.av.video_ms.map_or("–".into(), |v| format!("{v:+.0}"))),
        ),
        (
            "Sound later than picture (ms)",
            Box::new(|a| a.av.skew_ms().map_or("–".into(), |v| format!("{v:+.0}"))),
        ),
        (
            "Seconds that differ from the manifest",
            Box::new(|a| a.timeline_mismatches.len().to_string()),
        ),
        ("Wall time (s)", Box::new(|a| f(a.cost.wall_s, 1))),
        (
            "Faster than real time",
            Box::new(|a| format!("{:.1}x", a.duration_s / a.cost.wall_s.max(0.001))),
        ),
        ("CPU seconds", Box::new(|a| f(a.cost.cpu_s, 1))),
        (
            "Core-seconds per media minute",
            Box::new(|a| f(a.cost.core_seconds_per_media_minute(a.duration_s), 1)),
        ),
        (
            "Peak memory (MiB, tree)",
            Box::new(|a| f(a.cost.peak_rss_mib, 0)),
        ),
    ] {
        out.push_str(&row(label, n.map(&get), c.map(&get)));
    }
    out.push_str("</tbody></table></div>");

    out.push_str("<div class=\"card\"><h3>Timeline</h3><div class=\"legend\"><span><i style=\"background:#495057\"></i>card</span><span><i style=\"background:#e9ecef;border:1px solid var(--line)\"></i>board</span><span><i style=\"background:#4dabf7\"></i>screen</span><span><i style=\"box-shadow:inset 0 -4px 0 #2f9e44;background:var(--line)\"></i>camera lit</span><span><i style=\"outline:2px solid var(--bad)\"></i>differs from the manifest</span></div>");
    out.push_str(&timeline_row("Manifest", &r.expected, None));
    if let Some(n) = n {
        out.push_str(&timeline_row("Native", &n.timeline, Some(&r.expected)));
    }
    if let Some(c) = c {
        out.push_str(&timeline_row("Chromium", &c.timeline, Some(&r.expected)));
    }
    out.push_str("</div>");
    for pair in [
        &r.native_vs_reference,
        &r.chromium_vs_reference,
        &r.native_vs_chromium,
    ]
    .into_iter()
    .flatten()
    {
        out.push_str(&pair_html(pair));
    }
    out.push_str("</section>");
    out
}

const HOW_TO_READ: &str = "<div class=\"card\"><h2>How to read this</h2><ul>\
<li><b>Reference</b> is the native pipeline's own frames encoded losslessly: the picture both videos are meant to show. VMAF and SSIM compare a video with it frame by frame on the same 30 fps grid; <i>native vs Chromium</i> compares the two videos directly and is reported, not gated.</li>\
<li><b>Timeline</b> is read from the pictures of each video (card, board, screen, camera tile), so it shows what was drawn, not what an engine says it did. A red outline is a second that differs from what the manifest says.</li>\
<li><b>Sound offset</b> is found by correlating the microphone's own file, placed where the manifest puts it, with what the video's sound holds; <b>picture offset</b> by the camera's flashes in the tile. Their difference is the A/V offset the viewer sees.</li>\
<li><b>Cost</b> is measured in this process and the children it started; memory is the sum of the resident sizes of the whole tree (a shared page counts once per process, so it is an upper bound).</li>\
<li><b>Gates</b> (native): the runner's probe accepts the file, a keyframe at least every 2 s, the length, the timeline, the A/V offset within one frame, and a mean VMAF of at least 90 against the reference.</li></ul></div>";

/// The page.
pub fn html(reports: &[ScenarioReport], banner: &str) -> String {
    let mut body = String::from("<h1>Render comparison: Chromium and native</h1>");
    if !banner.is_empty() {
        let _ = write!(body, "<div class=\"banner\">{}</div>", escape(banner));
    }
    body.push_str("<div class=\"card tablewrap\"><table><thead><tr><th>Recording</th><th class=\"n\">Gates</th><th class=\"n\">Native VMAF vs ref</th><th class=\"n\">Chromium VMAF vs ref</th><th class=\"n\">Native vs Chromium</th><th class=\"n\">Native wall s</th><th class=\"n\">Chromium wall s</th><th class=\"n\">Native core-s/min</th><th class=\"n\">Chromium core-s/min</th></tr></thead><tbody>");
    for r in reports {
        let gates = r.gates();
        let passed = gates.iter().filter(|g| g.ok).count();
        let v = |p: &Option<Pair>| {
            p.as_ref()
                .map_or("–".to_owned(), |p| format!("{:.1}", p.vmaf.mean))
        };
        let wall = |a: &Result<Analysis, String>| {
            a.as_ref()
                .map_or("–".to_owned(), |a| format!("{:.1}", a.cost.wall_s))
        };
        let core = |a: &Result<Analysis, String>| {
            a.as_ref().map_or("–".to_owned(), |a| {
                format!("{:.1}", a.cost.core_seconds_per_media_minute(a.duration_s))
            })
        };
        let _ = write!(
            body,
            "<tr><td><a href=\"#{}\">{}</a></td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
            slug(&r.name),
            escape(&r.name),
            chip(
                if passed == gates.len() {
                    State::Pass
                } else {
                    State::Fail
                },
                &format!("{passed}/{}", gates.len())
            ),
            v(&r.native_vs_reference),
            v(&r.chromium_vs_reference),
            v(&r.native_vs_chromium),
            wall(&r.native),
            wall(&r.chromium),
            core(&r.native),
            core(&r.chromium)
        );
    }
    body.push_str("</tbody></table></div>");
    body.push_str(HOW_TO_READ);
    for r in reports {
        body.push_str(&scenario_html(r));
    }
    page("Render comparison", &body)
}

/// A short Markdown summary of the same table.
pub fn markdown(reports: &[ScenarioReport]) -> String {
    let mut out = String::from(
        "# Render comparison\n\n| recording | gates | native VMAF | Chromium VMAF | native vs Chromium | native wall s | Chromium wall s |\n|---|---|---|---|---|---|---|\n",
    );
    for r in reports {
        let gates = r.gates();
        let v = |p: &Option<Pair>| {
            p.as_ref()
                .map_or("-".to_owned(), |p| format!("{:.1}", p.vmaf.mean))
        };
        let wall = |a: &Result<Analysis, String>| {
            a.as_ref()
                .map_or("-".to_owned(), |a| format!("{:.1}", a.cost.wall_s))
        };
        let _ = writeln!(
            out,
            "| {} | {}/{} | {} | {} | {} | {} | {} |",
            r.name,
            gates.iter().filter(|g| g.ok).count(),
            gates.len(),
            v(&r.native_vs_reference),
            v(&r.chromium_vs_reference),
            v(&r.native_vs_chromium),
            wall(&r.native),
            wall(&r.chromium)
        );
    }
    for r in reports {
        for g in r.gates().iter().filter(|g| !g.ok) {
            let _ = writeln!(out, "\nFAIL {}: {}: {}", r.name, g.name, g.detail);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed_run() -> ScenarioReport {
        ScenarioReport {
            name: "board only".into(),
            media_s: 10.0,
            preroll_ms: 0,
            native: Err("no output".into()),
            chromium: Err("not run".into()),
            expected: vec![],
            native_vs_reference: None,
            chromium_vs_reference: None,
            native_vs_chromium: None,
        }
    }

    #[test]
    fn a_render_that_failed_fails_its_gate_and_the_page_says_why_chromium_is_missing() {
        let r = failed_run();
        assert!(!r.ok());
        let html = html(&[r], "banner <b>");
        assert!(html.contains("FAIL native render: no output"));
        assert!(html.contains("Chromium: not run"));
        assert!(html.contains("banner &lt;b&gt;"));
        assert!(html.contains("width=device-width") && !html.contains("https://"));
    }

    #[test]
    fn the_markdown_lists_the_failing_gates() {
        let md = markdown(&[failed_run()]);
        assert!(md.contains("| board only | 0/1 |"));
        assert!(md.contains("FAIL board only: native render: no output"));
    }

    #[test]
    fn a_timeline_marks_the_seconds_that_differ_from_the_manifest() {
        let s = |stage| Second {
            stage,
            camera: false,
        };
        let html = timeline_row(
            "Native",
            &[s(Stage::Board), s(Stage::Screen)],
            Some(&[s(Stage::Board), s(Stage::Board)]),
        );
        assert_eq!(html.matches("off").count(), 1);
        assert!(html.contains("class=\"screen off\""));
    }
}
