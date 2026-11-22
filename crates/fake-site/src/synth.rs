//! Synthetic recordings: a board log made here and video and sound made by ffmpeg's test sources.
//! The camera flashes white for 100 ms every second and the microphone beeps for the same 100 ms,
//! so a report can measure how far the two sit apart in a rendered video. The screens are ffmpeg's
//! `testsrc2`, which carries its own frame counter.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::recording::{DECK_ID, RecFile, Recording, TEACHER_ID};

const STUDENT_ID: &str = "studentfixture000000001";

/// What a test recording is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Strokes and text on a white board, one microphone.
    BoardOnly,
    /// A shared screen for most of the lesson, then the board again.
    ScreenShare,
    /// The teacher's camera in the strip; a student's camera and voice that must not show.
    Camera,
    /// Pages of an imported deck on the board.
    Decks,
    /// Vietnamese with every tone mark, and maths text.
    VietnameseMaths,
    /// A camera in two files of different shapes with a gap between, a screen in the middle and a
    /// microphone that reconnected.
    GapAndRotation,
    /// A race: lobby, a question with its reveal, the podium; the board before and after.
    Quiz,
    /// A Quest: the lobby, the live board with its time running down, the podium.
    Quest,
}

impl Scenario {
    pub const ALL: [Scenario; 8] = [
        Self::BoardOnly,
        Self::ScreenShare,
        Self::Camera,
        Self::Decks,
        Self::VietnameseMaths,
        Self::GapAndRotation,
        Self::Quiz,
        Self::Quest,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::BoardOnly => "board-only",
            Self::ScreenShare => "screen-share",
            Self::Camera => "camera",
            Self::Decks => "decks",
            Self::VietnameseMaths => "vietnamese-maths",
            Self::GapAndRotation => "gap-and-rotation",
            Self::Quiz => "quiz",
            Self::Quest => "quest",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }
}

fn run(ffmpeg: &Path, args: &[&str]) -> io::Result<()> {
    let output = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-y"])
        .args(args)
        .stdout(Stdio::null())
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
}

/// A VP8 video of `secs` seconds: ffmpeg's `testsrc2`, or (with `flash`) a gradient that goes
/// white for 100 ms every second.
fn video(ffmpeg: &Path, out: &Path, (w, h): (u32, u32), secs: f64, flash: bool) -> io::Result<()> {
    let source = if flash {
        format!(
            "color=c=black:size={w}x{h}:rate=30:duration={secs},\
             geq=lum='if(lt(mod(T\\,1)\\,0.1)\\,235\\,40+X*160/W)':cb=128:cr=128"
        )
    } else {
        format!("testsrc2=size={w}x{h}:rate=30:duration={secs}")
    };
    run(
        ffmpeg,
        &[
            "-f",
            "lavfi",
            "-i",
            &source,
            "-c:v",
            "libvpx",
            "-b:v",
            "1200k",
            "-g",
            "60",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-pix_fmt",
            "yuv420p",
            "-an",
            &out.to_string_lossy(),
        ],
    )
}

/// An Opus microphone of `secs` seconds that beeps 100 ms at the start of every second.
fn voice(ffmpeg: &Path, out: &Path, secs: f64) -> io::Result<()> {
    let source = format!("aevalsrc='sin(2*PI*1000*t)*lt(mod(t\\,1)\\,0.1)*0.5':s=48000:d={secs}");
    run(
        ffmpeg,
        &[
            "-f",
            "lavfi",
            "-i",
            &source,
            "-c:a",
            "libopus",
            "-b:a",
            "48k",
            &out.to_string_lossy(),
        ],
    )
}

/// A board record.
fn record(kind: &str, t: u64, elements: Vec<Value>) -> Value {
    json!({"k": kind, "t": t, "u": TEACHER_ID, "elements": elements})
}

fn element(id: &str, z: u32, data: Value) -> Value {
    let mut data = data;
    data["z"] = z.into();
    json!({"id": id, "version": 1, "version_nonce": 1000 + z, "is_deleted": false, "data": data})
}

fn text(id: &str, z: u32, x: f64, y: f64, size: u32, color: &str, s: &str) -> Value {
    element(
        id,
        z,
        json!({"type": "text", "color": color, "size": size, "text": s, "x": x, "y": y}),
    )
}

/// A pen stroke along a sine wave starting at (x, y).
fn stroke(id: &str, z: u32, x: f64, y: f64, color: &str) -> Value {
    let mut points = Vec::new();
    for k in 0..40 {
        let t = f64::from(k);
        points.extend([x + t * 8.0, y + (t / 4.0).sin() * 24.0, 0.5 + t / 100.0]);
    }
    element(
        id,
        z,
        json!({"type": "stroke", "color": color, "size": 4, "highlighter": false, "points": points}),
    )
}

fn page(id: &str, z: u32, deck: Option<(u32, f64, f64)>) -> Value {
    let (w, h) = deck.map_or((1280.0, 720.0), |(_, w, h)| (w, h));
    let mut data =
        json!({"type": "page", "color": "ink", "size": 2, "x": 0.0, "y": 0.0, "w": w, "h": h});
    if let Some((n, _, _)) = deck {
        data["deck"] = DECK_ID.into();
        data["n"] = n.into();
    }
    element(id, z, data)
}

/// A quiz frame of the board log (`k: "quiz"`).
fn quiz_frame(t_ms: u64, frame: Value) -> Value {
    json!({"t": t_ms, "k": "quiz", "q": frame})
}

/// The game of the quiz and Quest scenarios: it starts at 10 % of the lesson, runs over most of it
/// and ends before the last tenth, with the board drawn before and after.
fn quiz_records(scenario: Scenario, secs: u64) -> Vec<Value> {
    let ms = secs * 1000;
    let (from, end) = (ms / 10, ms * 9 / 10);
    let at = |frac: u64| from + (end - from) * frac / 100;
    let ranking = |bonus: u64| {
        json!([
            ["Anna", 1900 + bonus],
            ["Bình", 1750 + bonus],
            ["Chi", 1600 + bonus],
            ["", 1450 + bonus],
            ["Đức", 1300 + bonus]
        ])
    };
    match scenario {
        Scenario::Quiz => vec![
            quiz_frame(from, json!([1, 5_000_000, "Geography", 5, 12])),
            quiz_frame(
                at(10),
                json!([
                    2,
                    5_002_000,
                    1,
                    5,
                    5_002_000 + (at(60) - at(10)),
                    0,
                    "What is the capital of Vietnam?",
                    ["Hà Nội", "Huế", "Đà Nẵng", "Cần Thơ"]
                ]),
            ),
            quiz_frame(at(30), json!([4, 8, 12, [6, 1, 1, 0], ranking(0)])),
            quiz_frame(at(60), json!([5, 1, 1, [9, 1, 1, 1], null, ranking(0)])),
            quiz_frame(
                at(75),
                json!([
                    6,
                    [["Anna", 2600], ["Bình", 2400], ["Chi", 2200]],
                    ranking(700)
                ]),
            ),
            quiz_frame(end, json!([7, 0])),
        ],
        Scenario::Quest => {
            let board = |coins: u64| {
                json!([
                    29,
                    [
                        ["Anna", 0, 1000 + coins],
                        ["Bình", 5, 900 + coins],
                        ["Chi", 10, 800 + coins],
                        ["", 3, 700 + coins],
                        ["Đức", 8, 600 + coins]
                    ],
                    9,
                    40,
                    31,
                    290_000
                ])
            };
            let mut frames = vec![
                quiz_frame(from, json!([20, 5_000_000, "Quest", 5, 3, 300, 0])),
                quiz_frame(at(5), json!([21, [["Anna", 1], ["Bình", 4]]])),
                quiz_frame(at(10), json!([22, 5_010_000, 5_310_000])),
            ];
            for k in 0..8u64 {
                frames.push(quiz_frame(at(15 + k * 8), board(k * 150)));
            }
            frames.push(quiz_frame(
                at(85),
                json!([
                    6,
                    [["Anna", 2600], ["Bình", 2400], ["Chi", 2200]],
                    ranking(700)
                ]),
            ));
            frames.push(quiz_frame(end, json!([7, 0])));
            frames
        }
        _ => Vec::new(),
    }
}

/// The board log of a scenario: one new thing every second.
fn board_log(scenario: Scenario, secs: u64) -> Vec<u8> {
    let lines: &[&str] = match scenario {
        Scenario::VietnameseMaths => &[
            "Phương trình bậc hai: ax² + bx + c = 0",
            "Δ = b² − 4ac ≥ 0 thì có nghiệm",
            "x₁,₂ = (−b ± √Δ) / 2a",
            "Tổng các số hạng: ∑ aₙ, n ∈ ℕ",
            "Ưu điểm: Việt Nam, Đà Nẵng, Huế",
            "α + β = π/2 ⇒ sin α = cos β",
            "Hằng số π ≈ 3,14159 và e ≈ 2,71828",
        ],
        _ => &[
            "Hôm nay chúng ta học bài mới",
            "Ví dụ 1: tính 12 × 7",
            "Chú ý: đọc kỹ đề bài",
            "Bài tập về nhà",
        ],
    };
    let mut records = vec![record(
        "snapshot",
        0,
        vec![page(
            "p1",
            1,
            (scenario == Scenario::Decks).then_some((1, 1280.0, 720.0)),
        )],
    )];
    let colors = ["ink", "red", "blue", "green"];
    for s in 1..secs {
        let (i, z) = (s as usize, 10 + s as u32);
        let el = if scenario == Scenario::Decks && s % 6 == 0 {
            // A new page of the deck: the next sheet.
            let n = (s / 6) as u32 + 1;
            let mut e = page(
                &format!("p{n}"),
                z,
                Some((n.min(3), if n == 2 { 720.0 } else { 1280.0 }, 720.0)),
            );
            e["data"]["y"] = (f64::from(n - 1) * 800.0).into();
            e
        } else if s % 2 == 0 {
            stroke(
                &format!("k{s}"),
                z,
                60.0 + (i % 5) as f64 * 40.0,
                120.0 + (i % 6) as f64 * 70.0,
                colors[i % 4],
            )
        } else {
            text(
                &format!("t{s}"),
                z,
                80.0,
                80.0 + (i % 7) as f64 * 70.0,
                28,
                colors[i % 4],
                lines[i % lines.len()],
            )
        };
        records.push(record("elements", s * 1000, vec![el]));
    }
    records.extend(quiz_records(scenario, secs));
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    for r in records {
        writeln!(gz, "{r}").expect("writing to memory");
    }
    gz.finish().expect("gzip to memory")
}

/// Builds the recording of `scenario`, `secs` seconds long, in `dir`.
pub fn build(scenario: Scenario, ffmpeg: &Path, dir: &Path, secs: u64) -> io::Result<Recording> {
    std::fs::create_dir_all(dir)?;
    let ms = secs * 1000;
    let mut files: Vec<RecFile> = Vec::new();
    let mut add = |kind: &'static str,
                   user: &'static str,
                   session: &'static str,
                   (a, b): (u64, u64),
                   size: Option<(u32, u32)>,
                   path: PathBuf|
     -> io::Result<()> {
        let bytes = std::fs::metadata(&path)?.len();
        files.push(RecFile {
            kind,
            user,
            session,
            start_ms: a,
            end_ms: b,
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            path,
            size: bytes,
        });
        Ok(())
    };
    let secs_of = |(a, b): (u64, u64)| (b - a) as f64 / 1000.0;

    let board = dir.join("board.ndjson.gz");
    std::fs::write(&board, board_log(scenario, secs))?;
    add("board", "", "", (0, ms), None, board)?;

    let teacher = "s-teacher";
    let mut mic = |span: (u64, u64), name: &str| -> io::Result<()> {
        let path = dir.join(name);
        voice(ffmpeg, &path, secs_of(span))?;
        add("voice", TEACHER_ID, teacher, span, None, path)
    };
    match scenario {
        Scenario::GapAndRotation => {
            mic((0, ms * 9 / 20), "voice-a.webm")?;
            mic((ms / 2, ms), "voice-b.webm")?;
        }
        _ => mic((0, ms), "voice.webm")?,
    }
    match scenario {
        Scenario::ScreenShare => {
            let span = (ms / 5, ms * 4 / 5);
            let path = dir.join("screen.webm");
            video(ffmpeg, &path, (1920, 1080), secs_of(span), false)?;
            add(
                "screen",
                TEACHER_ID,
                teacher,
                span,
                Some((1920, 1080)),
                path,
            )?;
        }
        Scenario::Camera => {
            let path = dir.join("camera.webm");
            video(ffmpeg, &path, (640, 360), secs as f64, true)?;
            add(
                "camera",
                TEACHER_ID,
                teacher,
                (0, ms),
                Some((640, 360)),
                path,
            )?;
            let other = dir.join("student.webm");
            video(ffmpeg, &other, (320, 240), secs as f64, false)?;
            add(
                "camera",
                STUDENT_ID,
                "s-student",
                (0, ms),
                Some((320, 240)),
                other,
            )?;
        }
        Scenario::GapAndRotation => {
            let landscape = (0, ms * 2 / 5);
            let portrait = (ms * 3 / 5, ms);
            let a = dir.join("camera-a.webm");
            video(ffmpeg, &a, (640, 360), secs_of(landscape), true)?;
            add(
                "camera",
                TEACHER_ID,
                teacher,
                landscape,
                Some((640, 360)),
                a,
            )?;
            let b = dir.join("camera-b.webm");
            video(ffmpeg, &b, (360, 640), secs_of(portrait), true)?;
            add("camera", TEACHER_ID, teacher, portrait, Some((360, 640)), b)?;
            let span = (ms / 4, ms * 3 / 4);
            let screen = dir.join("screen.webm");
            video(ffmpeg, &screen, (1280, 720), secs_of(span), false)?;
            add(
                "screen",
                TEACHER_ID,
                teacher,
                span,
                Some((1280, 720)),
                screen,
            )?;
        }
        Scenario::BoardOnly
        | Scenario::Decks
        | Scenario::VietnameseMaths
        | Scenario::Quiz
        | Scenario::Quest => {}
    }

    let mut pictures: HashMap<(String, u32), &'static [u8]> = HashMap::new();
    if scenario == Scenario::Decks {
        pictures.insert((DECK_ID.into(), 1), include_bytes!("../assets/deck.png"));
        pictures.insert(
            (DECK_ID.into(), 2),
            include_bytes!("../assets/deck-portrait.png"),
        );
        pictures.insert(
            (DECK_ID.into(), 3),
            include_bytes!("../assets/deck-square.png"),
        );
    }
    Ok(Recording {
        name: scenario.name().to_owned(),
        duration_ms: ms,
        files,
        pictures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_log_is_gzipped_records_one_per_second() {
        use std::io::Read;
        let gz = board_log(Scenario::VietnameseMaths, 10);
        let mut text = String::new();
        flate2::read::GzDecoder::new(&gz[..])
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text.lines().count(), 10);
        assert!(text.contains("Δ = b²"));
        assert!(text.lines().next().unwrap().contains("\"k\":\"snapshot\""));
    }

    #[test]
    fn a_scenario_is_found_by_its_name() {
        for s in Scenario::ALL {
            assert_eq!(Scenario::parse(s.name()), Some(s));
        }
        assert_eq!(Scenario::parse("nope"), None);
    }
}
