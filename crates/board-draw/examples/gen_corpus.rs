//! Writes the parity corpus: small synthetic board logs (gzipped NDJSON, like the recorder's) that
//! between them use every core element, Vietnamese and maths text, sheets with a deck picture,
//! clears, pointer previews and a camera change, with the times each is sampled at, and the deck
//! picture they use. The output is committed (`tests/corpus/`); run this to change it:
//!
//!   cargo run -p board-draw --example gen_corpus --locked -- [--out DIR] [--fonts DIR]

mod support;

use std::io::Write;
use std::path::PathBuf;

use board_draw::{Canvas, Rgba, SkiaCanvas};
use flate2::{Compression, write::GzEncoder};
use serde_json::{Value, json};

/// The deck the sheets show: any 26 lowercase letters or digits.
const DECK: &str = "deckfixture00000000000001a";

const COLORS: [&str; 6] = ["ink", "red", "orange", "green", "blue", "violet"];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

fn env(id: &str, version: u32, z: u32, mut data: Value) -> Value {
    data["z"] = json!(z);
    json!({"id": id, "version": version, "version_nonce": 1000 + z, "is_deleted": false, "data": data})
}

fn tombstone(id: &str, version: u32) -> Value {
    json!({"id": id, "version": version, "version_nonce": 1, "is_deleted": true, "data": {}})
}

fn line(color: &str, size: u32, shape: &str, p: [f64; 4]) -> Value {
    json!({"type": shape, "color": color, "size": size, "x1": p[0], "y1": p[1], "x2": p[2], "y2": p[3]})
}

fn text(color: &str, size: u32, x: f64, y: f64, t: &str) -> Value {
    json!({"type": "text", "color": color, "size": size, "x": x, "y": y, "text": t})
}

/// A hand-drawn looking curve; `pressure` None gives the constant 0.5 a mouse sends.
fn stroke(
    rng: &mut Rng,
    color: &str,
    size: u32,
    highlighter: bool,
    at: (f64, f64),
    n: usize,
    pressure: bool,
) -> Value {
    let (mut x, mut y) = at;
    let mut heading = rng.range(0.0, std::f64::consts::TAU);
    let turn = rng.range(-0.15, 0.15);
    let mut points = Vec::new();
    for i in 0..n {
        let p = if pressure {
            0.5 + 0.35 * ((i as f64 / n as f64) * 3.1).sin()
        } else {
            0.5
        };
        points.extend([
            (x * 10.0).round() / 10.0,
            (y * 10.0).round() / 10.0,
            (p * 100.0).round() / 100.0,
        ]);
        heading += turn + rng.range(-0.05, 0.05);
        x += heading.cos() * 7.0;
        y += heading.sin() * 7.0;
    }
    json!({"type": "stroke", "color": color, "size": size, "highlighter": highlighter, "points": points})
}

fn page(x: f64, deck_page: Option<u32>) -> Value {
    let mut d =
        json!({"type": "page", "color": "ink", "size": 2, "x": x, "y": 0, "w": 1280, "h": 720});
    if let Some(n) = deck_page {
        d["deck"] = json!(DECK);
        d["n"] = json!(n);
    }
    d
}

fn rec(t: u32, kind: &str, extra: Value) -> Value {
    let mut r = json!({"t": t, "k": kind});
    r.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    r
}

fn elements() -> Vec<Value> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut els = Vec::new();
    let mut z = 1;
    let mut add = |id: String, data: Value| {
        els.push(env(&id, 1, z, data));
        z += 1;
    };
    for (i, c) in COLORS.iter().enumerate() {
        let x = 60.0 + 190.0 * i as f64;
        add(
            format!("pen{i}"),
            stroke(&mut rng, c, 4, false, (x, 80.0), 40, true),
        );
        add(
            format!("mouse{i}"),
            stroke(&mut rng, c, 8, false, (x, 200.0), 30, false),
        );
        add(
            format!("line{i}"),
            line(
                c,
                2 + 2 * (i as u32 % 3),
                "line",
                [x, 300.0, x + 120.0, 340.0 + 10.0 * i as f64],
            ),
        );
        add(
            format!("arrow{i}"),
            line(
                c,
                4,
                "arrow",
                [x, 420.0, x + 140.0, 380.0 - 15.0 * i as f64],
            ),
        );
    }
    add(
        "rect1".into(),
        line("ink", 4, "rect", [60.0, 460.0, 320.0, 580.0]),
    );
    add(
        "rect2".into(),
        line("blue", 2, "rect", [360.0, 560.0, 420.0, 470.0]),
    );
    add(
        "rect3".into(),
        line("red", 8, "rect", [470.0, 480.0, 480.0, 560.0]),
    );
    add(
        "ell1".into(),
        line("green", 4, "ellipse", [560.0, 460.0, 800.0, 580.0]),
    );
    add(
        "ell2".into(),
        line("violet", 2, "ellipse", [900.0, 560.0, 840.0, 470.0]),
    );
    add(
        "hl1".into(),
        stroke(&mut rng, "orange", 8, true, (100.0, 120.0), 25, false),
    );
    add(
        "hl2".into(),
        stroke(&mut rng, "green", 4, true, (500.0, 140.0), 25, true),
    );
    add(
        "dot".into(),
        json!({"type": "stroke", "color": "ink", "size": 8, "highlighter": false, "points": [1000.0, 520.0, 0.5]}),
    );
    add("hello".into(), text("ink", 28, 900.0, 80.0, "Hello, board"));
    add(
        "multi".into(),
        text(
            "blue",
            20,
            900.0,
            150.0,
            "line one\nline two is longer\nthree",
        ),
    );
    els
}

fn scene_elements() -> Vec<Value> {
    let mut rng = Rng(7);
    let mut els = elements();
    let more = [
        (
            "late1",
            stroke(&mut rng, "red", 4, false, (700.0, 250.0), 30, true),
        ),
        (
            "late2",
            line("ink", 4, "arrow", [700.0, 620.0, 1100.0, 600.0]),
        ),
    ];
    let mut out = vec![rec(
        1000,
        "snapshot",
        json!({"locked": false, "elements": els}),
    )];
    els = more
        .into_iter()
        .enumerate()
        .map(|(i, (id, d))| env(id, 1, 100 + i as u32, d))
        .collect();
    out.push(rec(
        2000,
        "elements",
        json!({"u": "teacher", "elements": els}),
    ));
    out
}

fn scene_text() -> Vec<Value> {
    let texts = [
        ("t1", "ink", 40, 40.0, 30.0, "Phương trình bậc hai"),
        (
            "t2",
            "blue",
            28,
            40.0,
            100.0,
            "Nếu b² − 4ac > 0 thì có hai nghiệm phân biệt",
        ),
        (
            "t3",
            "red",
            20,
            40.0,
            150.0,
            "ạ ả ã à á â ầ ấ ẩ ẫ ậ ă ằ ắ ẳ ẵ ặ đ Đ ê ề ế ể ễ ệ ơ ờ ớ ở ỡ ợ ư ừ ứ ử ữ ự",
        ),
        (
            "t4",
            "green",
            28,
            40.0,
            200.0,
            "½ × 3² = 4,5   ÷ ± ° µ ³ ¼ — “quoted” € ™",
        ),
        (
            "t5",
            "violet",
            20,
            40.0,
            260.0,
            "Dòng một\nDòng hai dài hơn nhiều\n\nSau dòng trống",
        ),
        ("t6", "orange", 40, 700.0, 320.0, "Ag fy.\nTo AV"),
        (
            "t7",
            "ink",
            20,
            40.0,
            420.0,
            "The quick brown fox jumps over the lazy dog 0123456789",
        ),
        (
            "t8",
            "ink",
            28,
            40.0,
            470.0,
            "Ngày 21 tháng 11 năm 2026: Học vui!",
        ),
        (
            "t9",
            "blue",
            28,
            40.0,
            520.0,
            "x₁ + x₂ = −b/a,  α β γ Δ π,  ∑ ∫ √ ≤ ≥ ≠ ≈ ∞ → ⇒",
        ),
        ("t10", "red", 20, 700.0, 30.0, "Ký hiệu ᴬ ᴮ ᴰ, ★ ✓ ✗ ♥ ▲ ◆"),
    ];
    let els: Vec<Value> = texts
        .iter()
        .enumerate()
        .map(|(i, (id, c, s, x, y, t))| env(id, 1, i as u32 + 1, text(c, *s, *x, *y, t)))
        .collect();
    vec![rec(1000, "snapshot", json!({"elements": els}))]
}

fn scene_sheets() -> Vec<Value> {
    let mut rng = Rng(42);
    let els = vec![
        env("s1", 1, 0, page(0.0, None)),
        env("s2", 1, 0, page(1600.0, Some(1))),
        env("s3", 1, 0, page(3200.0, Some(2))),
        env(
            "i1",
            1,
            10,
            stroke(&mut rng, "red", 4, false, (200.0, 200.0), 50, true),
        ),
        env("i2", 1, 11, text("ink", 28, 100.0, 80.0, "Trang một")),
        env(
            "i3",
            1,
            12,
            line("blue", 4, "ellipse", [1800.0, 400.0, 2300.0, 650.0]),
        ),
        env("i4", 1, 13, text("violet", 40, 1750.0, 60.0, "Slide 1")),
        env(
            "i5",
            1,
            14,
            line("green", 4, "arrow", [3300.0, 100.0, 3900.0, 500.0]),
        ),
    ];
    vec![
        rec(1000, "snapshot", json!({"elements": els})),
        rec(
            3000,
            "elements",
            json!({"u": "teacher", "elements": [env("view", 1, 0, json!({"type": "view", "color": "ink", "size": 1, "page": "s2"}))]}),
        ),
        rec(
            4000,
            "elements",
            json!({"u": "teacher", "elements": [env("view", 2, 0, json!({"type": "view", "color": "ink", "size": 1, "page": "s3"}))]}),
        ),
    ]
}

fn scene_dynamic() -> Vec<Value> {
    let mut rng = Rng(99);
    let base = vec![
        env(
            "a",
            1,
            1,
            line("ink", 4, "line", [100.0, 100.0, 500.0, 180.0]),
        ),
        env("b", 1, 2, text("blue", 28, 120.0, 240.0, "Trước khi xoá")),
        env(
            "c",
            1,
            3,
            line("red", 4, "ellipse", [600.0, 100.0, 900.0, 300.0]),
        ),
    ];
    let preview = |sid: u32, from: u32, pts: &[f64]| json!({"cursor": [pts[pts.len() - 3], pts[pts.len() - 2]], "preview": {"type": "stroke", "color": "green", "size": 4, "z": 50, "sid": sid, "from": from, "points": pts}});
    let whole =
        match stroke(&mut rng, "green", 4, false, (300.0, 350.0), 12, true)["points"].clone() {
            Value::Array(a) => a.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<f64>>(),
            _ => unreachable!(),
        };
    let finished = json!({"type": "stroke", "color": "green", "size": 4, "highlighter": false, "points": whole});
    vec![
        rec(1000, "snapshot", json!({"locked": false, "elements": base})),
        rec(
            2000,
            "presence",
            json!({"u": "u1", "p": preview(1, 0, &whole[..18])}),
        ),
        rec(
            2200,
            "presence",
            json!({"u": "u1", "p": preview(1, 6, &whole[18..])}),
        ),
        rec(
            2500,
            "presence",
            json!({"u": "u2", "p": {"cursor": [700, 400], "preview": {"type": "rect", "color": "violet", "size": 4, "z": 60, "x1": 650, "y1": 350, "x2": 800, "y2": 450}}}),
        ),
        rec(
            3000,
            "elements",
            json!({"u": "u1", "elements": [env("g1", 1, 50, finished)]}),
        ),
        rec(
            4000,
            "elements",
            json!({"u": "teacher", "elements": [env("far", 1, 70, text("ink", 40, 2600.0, 500.0, "Xa tít"))]}),
        ),
        rec(
            5000,
            "elements",
            json!({"u": "teacher", "elements": [tombstone("b", 2)]}),
        ),
        rec(6000, "clear", json!({"u": "teacher"})),
        rec(
            6200,
            "elements",
            json!({"u": "teacher", "elements": [env("n1", 1, 1, text("orange", 40, 200.0, 200.0, "Bảng mới")), env("n2", 1, 2, line("ink", 8, "line", [200.0, 300.0, 700.0, 300.0]))]}),
        ),
        rec(7000, "lock", json!({"locked": true})),
    ]
}

fn write_log(dir: &std::path::Path, name: &str, records: &[Value]) {
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    for r in records {
        writeln!(gz, "{r}").unwrap();
    }
    std::fs::write(dir.join(format!("{name}.ndjson.gz")), gz.finish().unwrap()).unwrap();
}

/// The picture of the deck's first page: flat blocks and a diagonal, no text, so it looks the same
/// whatever the fonts are.
fn deck_png(like: &SkiaCanvas) -> Vec<u8> {
    let mut c = like.offscreen(640, 360);
    c.set_fill_style(Rgba::hex(0xfff7e6));
    c.fill_rect(0.0, 0.0, 640.0, 360.0);
    for (i, color) in [0x2f9e44u32, 0x1971c2, 0xe8590c, 0x7048e8]
        .iter()
        .enumerate()
    {
        c.set_fill_style(Rgba::hex(*color));
        c.fill_rect(
            40.0 + 140.0 * i as f64,
            60.0 + 20.0 * i as f64,
            110.0,
            140.0 - 10.0 * i as f64,
        );
    }
    let mut p = board_draw::Path::new();
    p.move_to(20.0, 340.0);
    p.line_to(620.0, 240.0);
    c.set_stroke_style(Rgba::hex(0x1e1e1e));
    c.set_line_width(6.0);
    c.stroke(&p);
    // Same type as the board's canvas, so `png` is that of the offscreen one.
    c.png()
}

fn main() {
    let dir = PathBuf::from(
        support::opt("--out")
            .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus").to_owned()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let like = SkiaCanvas::new(8, 8, support::fonts()).unwrap();
    std::fs::write(dir.join("deck.png"), deck_png(&like)).unwrap();
    let scenes: [(&str, Vec<Value>, Vec<u32>); 4] = [
        ("elements", scene_elements(), vec![1500, 2500]),
        ("text", scene_text(), vec![1500]),
        ("sheets", scene_sheets(), vec![1500, 3500, 4500]),
        (
            "dynamic",
            scene_dynamic(),
            vec![1500, 2100, 2300, 2800, 3500, 4500, 5500, 6100, 6800, 7500],
        ),
    ];
    let mut index = Vec::new();
    for (name, records, times) in &scenes {
        write_log(&dir, name, records);
        index.push(json!({"name": name, "times": times}));
    }
    let index = json!({"deck": DECK, "width": 1280, "height": 624, "scenes": index});
    std::fs::write(
        dir.join("index.json"),
        serde_json::to_string_pretty(&index).unwrap() + "\n",
    )
    .unwrap();
}
