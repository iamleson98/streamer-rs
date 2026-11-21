//! Renders synthetic recordings with the native engine (production and lossless reference) and,
//! when a browser is given, with the Chromium engine through the real render page, measures every
//! video and writes `report.html` and `report.md`.
//!
//!   render-compare --out DIR --fonts DIR [--ffmpeg P] [--ffprobe P] [--vmaf-ffmpeg P]
//!       [--browser P --web-dir DIR [--no-sandbox]] [--scenarios a,b|all] [--secs 20]
//!       [--preroll-ms 60000] [--max-mb 14]
//!
//! Exit code 1: a gate of the native engine failed, or the report is larger than `--max-mb`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use fake_site::Scenario;
use render_compare::analyze::{self, Analysis, compare, expected_timeline};
use render_compare::cost::Cost;
use render_compare::metrics::{Tools, has_vmaf};
use render_compare::report::{ScenarioReport, html, markdown};
use render_compare::rig::{Rig, Run};
use stream_render::chrome::Chrome;
use stream_render::chromium::ChromiumEngine;
use stream_render::config::Settings;
use stream_render::native::NativeEngine;
use stream_render::native::encode::Profile;

/// `name` as an absolute path: the worker runs its tools with an empty environment, so a bare name
/// would not be found.
fn resolve(name: &str) -> PathBuf {
    let path = PathBuf::from(name);
    if path.components().count() > 1 {
        return path;
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(name))
                .find(|p| p.is_file())
        })
        .unwrap_or(path)
}

struct Args(Vec<String>);

impl Args {
    fn get(&self, name: &str) -> Option<String> {
        self.0
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.0.get(i + 1))
            .cloned()
    }

    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
}

fn keep(run: &Run, to: &Path) -> Result<PathBuf, String> {
    let from = run.result.as_ref().map_err(Clone::clone)?;
    std::fs::copy(from, to).map_err(|e| e.to_string())?;
    Ok(to.to_owned())
}

#[tokio::main]
async fn main() -> ExitCode {
    stream_common::redact::init_logging();
    let args = Args(std::env::args().skip(1).collect());
    let out = PathBuf::from(
        args.get("--out")
            .unwrap_or_else(|| "render-compare-out".into()),
    );
    let Some(fonts) = args.get("--fonts") else {
        eprintln!("missing --fonts DIR (the board's and the fallback fonts together)");
        return ExitCode::from(2);
    };
    let ffmpeg = resolve(&args.get("--ffmpeg").unwrap_or_else(|| "ffmpeg".into()));
    let tools = Tools {
        ffprobe: resolve(&args.get("--ffprobe").unwrap_or_else(|| "ffprobe".into())),
        vmaf_ffmpeg: PathBuf::from(
            args.get("--vmaf-ffmpeg")
                .unwrap_or_else(|| ffmpeg.display().to_string()),
        ),
        ffmpeg: ffmpeg.clone(),
    };
    let (render_only, analyze_only) = (args.has("--render-only"), args.has("--analyze-only"));
    if !render_only && !has_vmaf(&tools.vmaf_ffmpeg) {
        eprintln!("the ffmpeg for VMAF has no libvmaf (--vmaf-ffmpeg)");
        return ExitCode::from(2);
    }
    let secs: u64 = args
        .get("--secs")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let preroll_ms: u64 = args
        .get("--preroll-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60_000);
    let max_mb: f64 = args
        .get("--max-mb")
        .and_then(|s| s.parse().ok())
        .unwrap_or(14.0);
    let scenarios: Vec<Scenario> = match args.get("--scenarios").as_deref() {
        None | Some("all") => Scenario::ALL.to_vec(),
        Some(list) => list.split(',').filter_map(Scenario::parse).collect(),
    };
    let native = match NativeEngine::new(Path::new(&fonts)) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("the fonts: {error}");
            return ExitCode::from(2);
        }
    };
    let reference_engine = NativeEngine::new(Path::new(&fonts))
        .expect("fonts that loaded once")
        .with_profile(Profile::Lossless);
    let chromium = args.get("--browser").map(|browser| {
        ChromiumEngine::new(Chrome::new(
            browser.into(),
            args.has("--no-sandbox"),
            Duration::from_secs(60),
        ))
    });

    let mut settings = Settings::new("compare");
    settings.ffmpeg = ffmpeg.clone();
    settings.ffprobe = tools.ffprobe.clone();
    settings.retry_delay = Duration::from_millis(50);
    if let Some(dir) = args.get("--web-dir") {
        settings.web_dir = PathBuf::from(dir);
    }
    settings.browser_no_sandbox = args.has("--no-sandbox");

    let mut reports = Vec::new();
    for (i, scenario) in scenarios.iter().enumerate() {
        let name = scenario.name();
        eprintln!("== {name}");
        let dir = out.join(name);
        let _ = std::fs::create_dir_all(&dir);
        // The first recording opens with the countdown card.
        let preroll = if i == 0 { preroll_ms } else { 0 };
        let rig = match Rig::start(*scenario, &ffmpeg, &dir.join("recording"), secs, preroll).await
        {
            Ok(rig) => rig,
            Err(error) => {
                eprintln!("cannot build {name}: {error}");
                return ExitCode::from(2);
            }
        };
        let media_s = secs as f64;
        let measure = |label: &str, run: &Run, to: &Path| -> Result<Analysis, String> {
            let file = keep(run, to)?;
            analyze::analyze(&tools, label, &file, &rig.recording, preroll, run.cost)
                .map_err(|e| e.to_string())
        };

        if analyze_only {
            eprintln!("   (analysing the videos already rendered)");
        }
        let costs = dir.join("costs.json");
        let saved: serde_json::Value = std::fs::read(&costs)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let load = |key: &str, file: &str| -> Option<Run> {
            let c = saved.get(key)?;
            let n = |f: &str| c.get(f).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
            Some(Run {
                result: Ok(dir.join(file)),
                cost: Cost {
                    wall_s: n("wall_s"),
                    cpu_s: n("cpu_s"),
                    peak_rss_mib: n("peak_rss_mib"),
                },
            })
        };
        eprintln!("   native");
        let native_run = match analyze_only.then(|| load("native", "native.mp4")).flatten() {
            Some(run) => run,
            None => rig.run(&native, &settings, &dir.join("work-native")).await,
        };
        let native_kept = keep(&native_run, &dir.join("native.mp4"));
        eprintln!("   reference");
        let reference_run = match analyze_only
            .then(|| load("reference", "reference.mp4"))
            .flatten()
        {
            Some(run) => run,
            None => {
                rig.run(&reference_engine, &settings, &dir.join("work-reference"))
                    .await
            }
        };
        let reference_kept = keep(&reference_run, &dir.join("reference.mp4"));
        let chromium_run = match &chromium {
            _ if analyze_only => load("chromium", "chromium.mp4"),
            Some(engine) => {
                eprintln!("   chromium");
                Some(rig.run(engine, &settings, &dir.join("work-chromium")).await)
            }
            None => None,
        };
        let chromium_kept = chromium_run
            .as_ref()
            .map(|r| keep(r, &dir.join("chromium.mp4")));

        if !analyze_only {
            let cost = |r: &Run| serde_json::json!({"wall_s": r.cost.wall_s, "cpu_s": r.cost.cpu_s, "peak_rss_mib": r.cost.peak_rss_mib});
            let saved = serde_json::json!({
                "native": cost(&native_run),
                "reference": cost(&reference_run),
                "chromium": chromium_run.as_ref().map(cost),
            });
            let _ = std::fs::write(&costs, saved.to_string());
        }
        for run_dir in ["work-native", "work-reference", "work-chromium"] {
            let _ = std::fs::remove_dir_all(dir.join(run_dir));
        }
        if render_only {
            continue;
        }
        let native_a = measure("Native", &native_run, &dir.join("native.mp4"));
        let chromium_a = match &chromium_run {
            Some(run) => measure("Chromium", run, &dir.join("chromium.mp4")),
            None => Err("not run (no --browser)".to_owned()),
        };
        let pair = |title: &str,
                    a: (&str, &Result<PathBuf, String>),
                    b: (&str, &Result<PathBuf, String>),
                    key: &str| {
            let (Ok(pa), Ok(pb)) = (a.1, b.1) else {
                return None;
            };
            match compare(
                &tools,
                title,
                (a.0, pa),
                (b.0, pb),
                &dir.join(format!("metrics-{key}")),
                3,
            ) {
                Ok(pair) => Some(pair),
                Err(error) => {
                    eprintln!("   {title}: {error}");
                    None
                }
            }
        };
        let chromium_path = chromium_kept.unwrap_or_else(|| Err("not run".into()));
        let report = ScenarioReport {
            name: name.to_owned(),
            media_s,
            preroll_ms: preroll,
            expected: expected_timeline(
                &rig.recording,
                preroll,
                (media_s + preroll as f64 / 1000.0).ceil() as usize,
            ),
            native_vs_reference: pair(
                "Native against the lossless reference",
                ("native", &native_kept),
                ("reference", &reference_kept),
                "native-ref",
            ),
            chromium_vs_reference: pair(
                "Chromium against the lossless reference",
                ("Chromium", &chromium_path),
                ("reference", &reference_kept),
                "chromium-ref",
            ),
            native_vs_chromium: pair(
                "Native against Chromium",
                ("native", &native_kept),
                ("Chromium", &chromium_path),
                "native-chromium",
            ),
            native: native_a,
            chromium: chromium_a,
        };
        for run_dir in ["work-native", "work-reference", "work-chromium"] {
            let _ = std::fs::remove_dir_all(dir.join(run_dir));
        }
        reports.push(report);
    }

    if render_only {
        return ExitCode::SUCCESS;
    }
    let banner = format!(
        "Chromium here is {}; the numbers of record are the Linux CI run. {}",
        if cfg!(target_os = "macos") {
            "headless Chromium on macOS, which encodes H.264 through VideoToolbox, not the OpenH264 of the production image"
        } else {
            "headless Chromium on this machine"
        },
        if cfg!(target_os = "macos") {
            "Native cost on this machine is not the production host's either."
        } else {
            ""
        },
    );
    let page = html(&reports, &banner);
    let _ = std::fs::write(out.join("report.html"), &page);
    let _ = std::fs::write(out.join("report.md"), markdown(&reports));
    let mb = page.len() as f64 / 1e6;
    eprintln!("report.html {mb:.1} MB");
    let gates_ok = reports.iter().all(ScenarioReport::ok);
    if !gates_ok {
        eprint!("{}", markdown(&reports));
    }
    if mb > max_mb {
        eprintln!("the report is larger than --max-mb {max_mb}");
        return ExitCode::from(1);
    }
    if gates_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
