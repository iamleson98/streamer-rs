//! Writes the parity corpus: small synthetic board logs (gzipped NDJSON, like the recorder's), one
//! scene for each thing the board can show, named for what it tests so that a failing frame says
//! what broke. The scenes are in groups (strokes, shapes, text, sheets, viewport, scene rules,
//! presence, stage sizes, dense); `index.json` says how each is sampled. The output is committed
//! (`tests/corpus/`); run this to change it:
//!
//!   cargo run -p board-draw --example gen_corpus --locked -- [--out DIR] [--fonts DIR]

mod support;

use std::f64::consts::TAU;
use std::io::Write;
use std::path::PathBuf;

use board_draw::{Canvas, Path, Rgba, SkiaCanvas};
use flate2::{Compression, write::GzEncoder};
use serde_json::{Value, json};

/// The deck the sheets show: any 26 lowercase letters or digits.
const DECK: &str = "deckfixture00000000000001a";

const COLORS: [&str; 6] = ["ink", "red", "orange", "green", "blue", "violet"];
const SIZES: [u32; 3] = [2, 4, 8];
const TEXT_SIZES: [u32; 3] = [20, 28, 40];

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

    fn pick<'a, T>(&mut self, of: &'a [T]) -> &'a T {
        &of[(self.next() * of.len() as f64) as usize % of.len()]
    }
}

/// One scene of the corpus: a board log and the times it is drawn at.
struct Scene {
    name: String,
    group: &'static str,
    about: String,
    records: Vec<Value>,
    times: Vec<u32>,
    width: u32,
    height: u32,
    /// The camera instead of the one the board asks for: `[x, y, zoom]`.
    view: Option<[f64; 3]>,
    /// Sheets' pictures never arrive (the board draws their ring).
    loading: bool,
    /// The web side draws every frame from a fresh replay; the Rust side keeps one (seeking back).
    fresh: bool,
    /// Draws another scene's log.
    log: Option<String>,
}

fn scene(
    name: &str,
    group: &'static str,
    about: &str,
    records: Vec<Value>,
    times: &[u32],
) -> Scene {
    Scene {
        name: name.into(),
        group,
        about: about.into(),
        records,
        times: times.to_vec(),
        width: 1280,
        height: 624,
        view: None,
        loading: false,
        fresh: false,
        log: None,
    }
}

/// Elements in stacking order, written as one snapshot.
struct Board {
    els: Vec<Value>,
    z: u32,
}

impl Board {
    fn new() -> Self {
        Self {
            els: Vec::new(),
            z: 0,
        }
    }

    fn add(&mut self, data: Value) -> &mut Self {
        let id = format!("e{}", self.els.len());
        self.add_as(&id, data)
    }

    fn add_as(&mut self, id: &str, data: Value) -> &mut Self {
        self.z += 1;
        self.els.push(env(id, 1, self.z, data));
        self
    }

    fn snapshot(&self) -> Vec<Value> {
        vec![rec(1000, "snapshot", json!({"elements": self.els}))]
    }

    fn still(self, name: &str, group: &'static str, about: &str) -> Scene {
        scene(name, group, about, self.snapshot(), &[1500])
    }
}

fn env(id: &str, version: u32, z: u32, data: Value) -> Value {
    env_nonce(id, version, 1000 + z, z, data)
}

fn env_nonce(id: &str, version: u32, nonce: u32, z: u32, mut data: Value) -> Value {
    data["z"] = json!(z);
    json!({"id": id, "version": version, "version_nonce": nonce, "is_deleted": false, "data": data})
}

fn tombstone(id: &str, version: u32) -> Value {
    json!({"id": id, "version": version, "version_nonce": 1, "is_deleted": true, "data": {}})
}

fn shape(kind: &str, color: &str, size: impl Into<f64>, p: [f64; 4]) -> Value {
    json!({"type": kind, "color": color, "size": size.into(), "x1": p[0], "y1": p[1], "x2": p[2], "y2": p[3]})
}

fn text(color: &str, size: u32, x: f64, y: f64, t: &str) -> Value {
    json!({"type": "text", "color": color, "size": size, "x": x, "y": y, "text": t})
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Flat `[x, y, pressure, ...]` as the client stores it: coordinates to a tenth, pressure to a
/// hundredth.
fn flat(points: impl IntoIterator<Item = (f64, f64, f64)>) -> Vec<f64> {
    points
        .into_iter()
        .flat_map(|(x, y, p)| [round1(x), round1(y), (p * 100.0).round() / 100.0])
        .collect()
}

fn ink_stroke(color: &str, size: u32, highlighter: bool, points: Vec<f64>) -> Value {
    json!({"type": "stroke", "color": color, "size": size, "highlighter": highlighter, "points": points})
}

#[derive(Clone, Copy)]
enum Pressure {
    /// A mouse sends 0.5 every time.
    Mouse,
    /// A pen's rise and fall.
    Pen,
    Fixed(f64),
}

impl Pressure {
    fn at(self, i: usize, n: usize) -> f64 {
        match self {
            Pressure::Mouse => 0.5,
            Pressure::Pen => 0.5 + 0.35 * ((i as f64 / n as f64) * 3.1).sin(),
            Pressure::Fixed(p) => p,
        }
    }
}

/// A hand-drawn looking curve of `n` points `step` apart.
fn walk(rng: &mut Rng, at: (f64, f64), n: usize, step: f64, pressure: Pressure) -> Vec<f64> {
    let (mut x, mut y) = at;
    let mut heading = rng.range(0.0, TAU);
    let turn = rng.range(-0.15, 0.15);
    let mut points = Vec::new();
    for i in 0..n {
        points.push((x, y, pressure.at(i, n)));
        heading += turn + rng.range(-0.05, 0.05);
        x += heading.cos() * step;
        y += heading.sin() * step;
    }
    flat(points)
}

fn stroke(
    rng: &mut Rng,
    color: &str,
    size: u32,
    highlighter: bool,
    at: (f64, f64),
    n: usize,
    pressure: bool,
) -> Value {
    let p = if pressure {
        Pressure::Pen
    } else {
        Pressure::Mouse
    };
    ink_stroke(color, size, highlighter, walk(rng, at, n, 7.0, p))
}

/// A sine wave across `width` pixels sampled `n` times.
fn wave(at: (f64, f64), width: f64, amp: f64, cycles: f64, n: usize, p: Pressure) -> Vec<f64> {
    flat((0..n).map(|i| {
        let f = i as f64 / (n - 1) as f64;
        (
            at.0 + width * f,
            at.1 + amp * (f * cycles * TAU).sin(),
            p.at(i, n),
        )
    }))
}

fn rec(t: u32, kind: &str, extra: Value) -> Value {
    let mut r = json!({"t": t, "k": kind});
    r.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    r
}

fn elements_rec(t: u32, user: &str, els: Vec<Value>) -> Value {
    rec(t, "elements", json!({"u": user, "elements": els}))
}

fn presence_rec(t: u32, user: &str, p: Value) -> Value {
    rec(t, "presence", json!({"u": user, "p": p}))
}

fn page(x: f64, y: f64, w: f64, h: f64, deck_page: Option<u32>) -> Value {
    let mut d = json!({"type": "page", "color": "ink", "size": 2, "x": x, "y": y, "w": w, "h": h});
    if let Some(n) = deck_page {
        d["deck"] = json!(DECK);
        d["n"] = json!(n);
    }
    d
}

fn view_rec(t: u32, version: u32, sheet: &str) -> Value {
    elements_rec(
        t,
        "teacher",
        vec![env(
            "view",
            version,
            0,
            json!({"type": "view", "color": "ink", "size": 1, "page": sheet}),
        )],
    )
}

// ---------------------------------------------------------------------------------------------
// Strokes

fn strokes() -> Vec<Scene> {
    const G: &str = "Strokes";
    let mut out = Vec::new();

    let mut rng = Rng(11);
    let mut b = Board::new();
    for (i, c) in COLORS.iter().enumerate() {
        let x = 60.0 + 200.0 * i as f64;
        b.add(stroke(&mut rng, c, 4, false, (x, 100.0), 40, true));
        b.add(stroke(&mut rng, c, 4, false, (x, 300.0), 40, false));
        b.add(stroke(&mut rng, c, 8, true, (x + 40.0, 480.0), 25, false));
    }
    out.push(b.still(
        "stroke-every-colour",
        G,
        "A pen stroke, a mouse stroke and a highlighter stroke in each of the six colours.",
    ));

    for size in SIZES {
        let mut rng = Rng(20 + u64::from(size));
        let mut b = Board::new();
        for (i, c) in ["ink", "red", "blue"].iter().enumerate() {
            let y = 120.0 + 170.0 * i as f64;
            b.add(ink_stroke(
                c,
                size,
                false,
                walk(&mut rng, (100.0, y), 60, 12.0, Pressure::Pen),
            ));
            b.add(ink_stroke(
                c,
                size,
                false,
                walk(&mut rng, (700.0, y), 60, 12.0, Pressure::Mouse),
            ));
        }
        out.push(b.still(
            &format!("stroke-size-{size}"),
            G,
            &format!("Pen (left) and mouse (right) strokes at size {size}."),
        ));
    }

    let mut b = Board::new();
    b.add(ink_stroke(
        "ink",
        4,
        false,
        wave((60.0, 100.0), 1100.0, 30.0, 3.0, 80, Pressure::Pen),
    ));
    for (i, size) in SIZES.iter().enumerate() {
        let y = 100.0 + 150.0 * i as f64;
        b.add(ink_stroke(
            "orange",
            *size,
            true,
            wave((60.0, y + 70.0), 1100.0, 40.0, 2.0, 70, Pressure::Mouse),
        ));
        b.add(ink_stroke(
            "green",
            *size,
            false,
            wave((60.0, y + 90.0), 1100.0, 40.0, 2.0, 70, Pressure::Mouse),
        ));
    }
    b.add(ink_stroke(
        "blue",
        8,
        true,
        wave((300.0, 520.0), 700.0, 20.0, 1.0, 40, Pressure::Pen),
    ));
    out.push(b.still(
        "stroke-highlighter-on-and-off",
        G,
        "Highlighters (translucent, no pressure) of sizes 2, 4 and 8 beside pens, crossing a pen line.",
    ));

    let mut b = Board::new();
    for (i, p) in [0.1, 0.3, 0.5, 0.8, 1.0].iter().enumerate() {
        b.add(ink_stroke(
            "ink",
            4,
            false,
            wave(
                (80.0, 60.0 + 40.0 * i as f64),
                500.0,
                10.0,
                2.0,
                40,
                Pressure::Fixed(*p),
            ),
        ));
        b.add(ink_stroke(
            "red",
            8,
            false,
            wave(
                (700.0, 60.0 + 40.0 * i as f64),
                500.0,
                10.0,
                2.0,
                40,
                Pressure::Fixed(*p),
            ),
        ));
    }
    b.add(ink_stroke(
        "blue",
        4,
        false,
        wave((80.0, 380.0), 500.0, 40.0, 2.0, 60, Pressure::Mouse),
    ));
    b.add(ink_stroke(
        "blue",
        8,
        false,
        wave((700.0, 380.0), 500.0, 40.0, 2.0, 60, Pressure::Mouse),
    ));
    out.push(b.still(
        "stroke-pressure-constant",
        G,
        "Constant pressure: 0.5 (a mouse, which simulates pressure) and other values (taken as sent).",
    ));

    let mut b = Board::new();
    for (i, size) in SIZES.iter().enumerate() {
        b.add(ink_stroke(
            "violet",
            *size,
            false,
            wave(
                (80.0, 100.0 + 150.0 * i as f64),
                1100.0,
                50.0,
                2.5,
                90,
                Pressure::Pen,
            ),
        ));
    }
    let n = 60;
    b.add(ink_stroke(
        "ink",
        8,
        false,
        flat((0..n).map(|i| {
            let f = i as f64 / (n - 1) as f64;
            (100.0 + 1000.0 * f, 540.0, 0.05 + 0.95 * f)
        })),
    ));
    out.push(b.still(
        "stroke-pressure-varying",
        G,
        "Pen strokes whose pressure rises and falls, and one that ramps from 0.05 to 1.",
    ));

    let mut b = Board::new();
    for (i, size) in SIZES.iter().enumerate() {
        b.add(ink_stroke(
            COLORS[i],
            *size,
            false,
            vec![200.0 + 150.0 * i as f64, 200.0, 0.5],
        ));
        b.add(ink_stroke(
            COLORS[i + 3],
            *size,
            true,
            vec![200.0 + 150.0 * i as f64, 340.0, 0.5],
        ));
        b.add(ink_stroke(
            COLORS[i],
            *size,
            false,
            vec![700.0 + 150.0 * i as f64, 200.0, 0.9],
        ));
    }
    b.add(ink_stroke(
        "ink",
        4,
        false,
        vec![200.0, 480.0, 0.5, 200.0, 480.0, 0.5],
    ));
    out.push(b.still(
        "stroke-one-point-dot",
        G,
        "A stroke of one point (a dot) at each size, as a highlighter, with pressure 0.9, and a stroke of two identical points.",
    ));

    let mut b = Board::new();
    for (i, size) in SIZES.iter().enumerate() {
        b.add(ink_stroke(
            "ink",
            *size,
            false,
            wave(
                (100.0, 150.0 + 150.0 * i as f64),
                1100.0,
                35.0,
                4.0,
                2,
                Pressure::Pen,
            ),
        ));
    }
    out.push(b.still(
        "stroke-two-points",
        G,
        "Strokes of two points, which are straight segments with round ends.",
    ));

    let mut b = Board::new();
    let n = 1200;
    let mut rng = Rng(5);
    let mut pts = Vec::new();
    for i in 0..n {
        let f = i as f64 / (n - 1) as f64;
        pts.push((
            60.0 + 1150.0 * f + rng.range(-1.5, 1.5),
            320.0
                + 200.0 * (f * 19.0 * TAU / 3.0).sin() * (1.0 - f * 0.5)
                + 60.0 * (f * 53.0).sin()
                + rng.range(-1.5, 1.5),
            0.4 + 0.3 * (f * 40.0).sin(),
        ));
    }
    b.add(ink_stroke("blue", 4, false, flat(pts)));
    out.push(b.still(
        "stroke-1200-points",
        G,
        "A single stroke of 1,200 points, the most the client keeps.",
    ));

    let mut b = Board::new();
    let lissajous = |cx: f64, cy: f64, a: f64, bb: f64, n: usize, p: Pressure| {
        flat((0..n).map(|i| {
            let t = TAU * i as f64 / (n - 1) as f64;
            (
                cx + a * (3.0 * t).sin(),
                cy + bb * (4.0 * t + 1.0).sin(),
                p.at(i, n),
            )
        }))
    };
    b.add(ink_stroke(
        "red",
        4,
        false,
        lissajous(330.0, 310.0, 250.0, 230.0, 300, Pressure::Pen),
    ));
    b.add(ink_stroke(
        "ink",
        8,
        true,
        lissajous(900.0, 310.0, 250.0, 230.0, 300, Pressure::Mouse),
    ));
    let spiral = flat((0..200).map(|i| {
        let t = i as f64 * 0.25;
        (900.0 + 4.0 * t * t.cos(), 310.0 + 4.0 * t * t.sin(), 0.5)
    }));
    b.add(ink_stroke("green", 2, false, spiral));
    out.push(b.still(
        "stroke-self-crossing-scribble",
        G,
        "Strokes that cross themselves: a figure of loops with a pen, a translucent highlighter over it, and a spiral.",
    ));

    let mut b = Board::new();
    let fast: Vec<f64> = flat((0..18).map(|i| {
        (
            60.0 + 66.0 * i as f64,
            150.0 + 90.0 * (i as f64 * 0.9).sin(),
            0.5,
        )
    }));
    b.add(ink_stroke("ink", 4, false, fast.clone()));
    let mut rng = Rng(3);
    b.add(ink_stroke(
        "red",
        4,
        false,
        walk(&mut rng, (100.0, 400.0), 14, 95.0, Pressure::Pen),
    ));
    b.add(ink_stroke(
        "blue",
        8,
        true,
        fast.iter()
            .enumerate()
            .map(|(i, v)| if i % 3 == 1 { v + 220.0 } else { *v })
            .collect(),
    ));
    out.push(b.still(
        "stroke-very-fast-sampling",
        G,
        "Few points far apart (60 to 95 px), as a fast flick of the pen is sampled.",
    ));

    let mut b = Board::new();
    b.add(ink_stroke(
        "ink",
        4,
        false,
        flat((0..500).map(|i| {
            let f = i as f64 / 499.0;
            (100.0 + 120.0 * f, 150.0 + 40.0 * (f * TAU).sin(), 0.5)
        })),
    ));
    b.add(ink_stroke(
        "red",
        4,
        false,
        flat((0..400).map(|i| {
            let f = i as f64 / 399.0;
            (
                400.0 + 100.0 * f,
                150.0 + 40.0 * (f * TAU).sin(),
                0.5 + 0.3 * (f * 9.0).sin(),
            )
        })),
    ));
    b.add(ink_stroke(
        "blue",
        8,
        false,
        flat((0..300).map(|i| {
            let f = i as f64 / 299.0;
            (700.0 + 80.0 * f, 150.0 + 20.0 * (f * TAU).sin(), 0.5)
        })),
    ));
    b.add(ink_stroke(
        "green",
        8,
        true,
        flat((0..300).map(|i| {
            let f = i as f64 / 299.0;
            (900.0 + 200.0 * f, 150.0 + 30.0 * (f * TAU).sin(), 0.5)
        })),
    ));
    out.push(b.still(
        "stroke-very-slow-sampling",
        G,
        "Hundreds of points a fraction of a pixel apart (a slow hand on a fine pen), many of them equal after rounding.",
    ));

    let mut b = Board::new();
    for (i, p) in [0.0, -0.3, 1.5, 7.0, 0.01].iter().enumerate() {
        b.add(ink_stroke(
            COLORS[i],
            8,
            false,
            wave(
                (100.0, 80.0 + 100.0 * i as f64),
                500.0,
                20.0,
                2.0,
                30,
                Pressure::Fixed(*p),
            ),
        ));
        b.add(ink_stroke(
            COLORS[i],
            4,
            false,
            flat((0..30).map(|k| {
                let f = f64::from(k) / 29.0;
                (
                    700.0 + 500.0 * f,
                    80.0 + 100.0 * i as f64 + 20.0 * (f * TAU * 2.0).sin(),
                    if k % 2 == 0 { *p } else { 0.5 },
                )
            })),
        ));
    }
    out.push(b.still(
        "stroke-pressure-out-of-range",
        G,
        "Pressure that a device should not send (0, negative, above 1) is taken as sent, constant (left) or alternating with 0.5 (right).",
    ));

    let mut b = Board::new();
    let line = wave((100.0, 150.0), 600.0, 30.0, 2.0, 40, Pressure::Pen);
    for (i, flag) in [
        json!(true),
        json!("true"),
        json!(1),
        json!(false),
        Value::Null,
    ]
    .iter()
    .enumerate()
    {
        let mut d = ink_stroke(
            "orange",
            8,
            false,
            line.iter()
                .enumerate()
                .map(|(k, v)| if k % 3 == 1 { v + 90.0 * i as f64 } else { *v })
                .collect(),
        );
        d["highlighter"] = flag.clone();
        b.add(d);
    }
    out.push(b.still(
        "stroke-highlighter-flag-must-be-true",
        G,
        "Only `true` makes a highlighter: the string \"true\", 1, false and null draw pens (the rows are, from the top, true, \"true\", 1, false, null).",
    ));
    out
}

// ---------------------------------------------------------------------------------------------
// Shapes

/// A shape drawn in each size (columns) and each direction (rows).
fn shape_grid(kind: &str) -> Board {
    let mut b = Board::new();
    for (col, size) in SIZES.iter().enumerate() {
        for (row, (dx, dy)) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)]
            .iter()
            .enumerate()
        {
            let (cx, cy) = (210.0 + 430.0 * col as f64, 90.0 + 145.0 * row as f64);
            let c = COLORS[(col + row) % 6];
            b.add(shape(
                kind,
                c,
                *size,
                [cx, cy, cx + dx * 150.0, cy + dy * 55.0],
            ));
        }
    }
    b
}

fn shapes() -> Vec<Scene> {
    const G: &str = "Shapes";
    let mut out = Vec::new();
    for kind in ["line", "arrow", "rect", "ellipse"] {
        out.push(shape_grid(kind).still(
            &format!("shape-{kind}-every-size-and-direction"),
            G,
            &format!(
                "A {kind} at sizes 2, 4 and 8 drawn right/down, left/down, right/up and left/up (x2 < x1, y2 < y1)."
            ),
        ));
    }

    let mut b = Board::new();
    for (i, kind) in ["line", "arrow", "rect", "ellipse"].iter().enumerate() {
        for (j, size) in SIZES.iter().enumerate() {
            let (x, y) = (120.0 + 280.0 * i as f64, 120.0 + 150.0 * j as f64);
            b.add(shape(kind, COLORS[i + j], *size, [x, y, x, y]));
        }
    }
    out.push(b.still(
        "shape-zero-size",
        G,
        "Each shape with both ends at one point: a dot where round caps give one, nothing where they do not.",
    ));

    let mut b = Board::new();
    for (i, kind) in ["line", "arrow", "rect", "ellipse"].iter().enumerate() {
        for (j, size) in SIZES.iter().enumerate() {
            let x = 80.0 + 300.0 * i as f64;
            let y = 80.0 + 170.0 * j as f64;
            b.add(shape(kind, COLORS[i], *size, [x, y, x + 1.0, y + 1.0]));
            b.add(shape(
                kind,
                COLORS[i + 1],
                *size,
                [x + 60.0, y, x + 61.0, y + 90.0],
            ));
            b.add(shape(
                kind,
                COLORS[i + 2],
                *size,
                [x + 100.0, y, x + 190.0, y + 1.0],
            ));
        }
    }
    out.push(b.still(
        "shape-one-pixel",
        G,
        "Each shape one pixel wide and high, one pixel wide only, and one pixel high only.",
    ));

    let mut b = Board::new();
    let mut x = 60.0;
    for (i, w) in [2.0, 4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0, 64.0]
        .iter()
        .enumerate()
    {
        let size = SIZES[i % 3];
        b.add(shape(
            "rect",
            COLORS[i % 6],
            size,
            [x, 100.0, x + w, 100.0 + w],
        ));
        b.add(shape(
            "rect",
            COLORS[(i + 1) % 6],
            size,
            [x, 220.0, x + w, 220.0 + 3.0 * w],
        ));
        b.add(shape(
            "rect",
            COLORS[(i + 2) % 6],
            size,
            [x, 470.0, x + 3.0 * w, 470.0 + w],
        ));
        x += w + 60.0;
    }
    out.push(b.still(
        "shape-rect-tiny",
        G,
        "Small rectangles (2 to 64 px) where the corner radius is a quarter of the shorter side, not 8.",
    ));

    let mut b = Board::new();
    b.add(shape("rect", "ink", 4, [40.0, 40.0, 1240.0, 584.0]));
    b.add(shape("rect", "red", 8, [80.0, 80.0, 1200.0, 140.0]));
    b.add(shape("rect", "blue", 2, [80.0, 200.0, 140.0, 540.0]));
    b.add(shape("rect", "green", 4, [200.0, 200.0, 232.0, 232.0]));
    b.add(shape("rect", "violet", 4, [300.0, 200.0, 332.0, 264.0]));
    b.add(shape("rect", "orange", 4, [400.0, 200.0, 440.0, 400.0]));
    b.add(shape("rect", "ink", 4, [500.0, 200.0, 1100.0, 540.0]));
    b.add(shape("ellipse", "orange", 4, [520.0, 220.0, 1080.0, 520.0]));
    out.push(b.still(
        "shape-rect-huge",
        G,
        "Large rectangles, where the corner radius is the 8 px cap, and one where one side is short.",
    ));

    let mut b = Board::new();
    for (j, size) in SIZES.iter().enumerate() {
        for (i, len) in [0.0, 1.0, 4.0, 8.0, 12.0, 20.0, 30.0, 60.0, 120.0]
            .iter()
            .enumerate()
        {
            let (x, y) = (60.0 + 130.0 * i as f64, 90.0 + 170.0 * j as f64);
            b.add(shape(
                "arrow",
                COLORS[i % 6],
                *size,
                [x, y, x + len, y + len * 0.6],
            ));
            b.add(shape(
                "arrow",
                COLORS[(i + 3) % 6],
                *size,
                [x, y + 80.0, x, y + 80.0 - len],
            ));
        }
    }
    out.push(b.still(
        "shape-arrow-short",
        G,
        "Arrows from 0 to 120 px long: the head is at least 12 px (3.5 times the size when that is more), however short the shaft.",
    ));
    out
}

// ---------------------------------------------------------------------------------------------
// Text

const GREEK: &str = "α β γ δ ε ζ η θ ι κ λ μ ν ξ π ρ σ τ υ φ χ ψ ω Γ Δ Θ Λ Ξ Π Σ Φ Ψ Ω";
const OPERATORS: &str = "± ∓ × ÷ · ≠ ≈ ≡ ∼ ≤ ≥ ≪ ≫ ∝ ∞ √ ∑ ∏ ∫ ∮ ∂ ∇ ° ∠ ⊥ ∥ ∴ ∵ ħ";
const SETS: &str = "∈ ∉ ⊂ ⊃ ⊆ ⊇ ∪ ∩ ∅ ∀ ∃ ¬ ∧ ∨ ℕ ℤ ℚ ℝ ℂ";
const ARROWS: &str = "→ ← ↑ ↓ ↔ ⇒ ⇐ ⇔ ⇌ ⟶";
const SUPERS: &str = "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻ";
const SUPER_CAPITALS: &str = "ᴬᴮᴰᴱᴳᴴᴵᴶᴷᴸᴹᴺᴼᴾᴿᵀᵁᵂ";
const SUBS: &str = "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓ";

fn text_lines(lines: &[(&str, u32, &str)]) -> Board {
    let mut b = Board::new();
    let mut y = 30.0;
    for (color, size, t) in lines {
        b.add(text(color, *size, 40.0, y, t));
        y += f64::from(*size) * 1.25 * t.split('\n').count() as f64 + 14.0;
    }
    b
}

fn texts() -> Vec<Scene> {
    const G: &str = "Text";
    let mut out = Vec::new();

    let mut b = Board::new();
    let mut y = 30.0;
    for size in TEXT_SIZES {
        b.add(text(
            "ink",
            size,
            40.0,
            y,
            &format!("Size {size}: The quick brown fox, Ag fy. To AV"),
        ));
        y += f64::from(size) * 1.25 + 24.0;
    }
    for size in TEXT_SIZES {
        b.add(text("blue", size, 40.0, y, "Dòng một\nDòng hai\nDòng ba"));
        y += f64::from(size) * 3.75 + 24.0;
    }
    out.push(b.still(
        "text-sizes-20-28-40",
        G,
        "One line and three lines of text at each of the three sizes.",
    ));

    out.push(
        text_lines(&[
            ("ink", 28, "first\n\nthird after one empty line"),
            ("red", 20, "\nstarts with an empty line"),
            ("green", 28, "ends with an empty line\n"),
            ("violet", 20, "a\n\n\n\nfive lines, three of them empty"),
            (
                "blue",
                28,
                "   leading and trailing spaces   \n\ttab\tseparated\r\u{c}end",
            ),
        ])
        .still(
            "text-multiline-empty-lines",
            G,
            "Lines of text with empty lines at the start, the middle and the end, spaces, tabs, a carriage return and a form feed (canvas text takes the last three for spaces).",
        ),
    );

    let long: String = (0..2000)
        .map(|i| {
            if i % 50 == 49 {
                ' '
            } else {
                char::from(b'a' + (i * 7 % 26) as u8)
            }
        })
        .collect();
    let mut b = Board::new();
    b.add(text("ink", 20, 40.0, 40.0, &long));
    b.add(text("red", 28, 40.0, 100.0, &long[..1990]));
    let over: String = long
        .chars()
        .chain("OVER".chars().cycle().take(100))
        .collect();
    b.add(text("blue", 20, 40.0, 160.0, &over));
    out.push(b.still(
        "text-2000-character-line",
        G,
        "Lines of 2,000 characters (and one of 2,100, cut to the 2,000 the board keeps): a very wide box, so the camera is at its smallest zoom.",
    ));

    let mut b = Board::new();
    for (i, c) in COLORS.iter().enumerate() {
        b.add(text(
            c,
            [20, 28, 40][i % 3],
            40.0 + 600.0 * (i % 2) as f64,
            40.0 + 190.0 * (i / 2) as f64,
            &format!("Màu {c}\nColour {c}"),
        ));
    }
    out.push(b.still("text-every-colour", G, "Text in each of the six colours."));

    let tones = |marks: &str| {
        marks
            .split(' ')
            .map(|g| g.to_owned())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let lower = "à á ả ã ạ ă ằ ắ ẳ ẵ ặ â ầ ấ ẩ ẫ ậ\nè é ẻ ẽ ẹ ê ề ế ể ễ ệ\nì í ỉ ĩ ị ò ó ỏ õ ọ\nô ồ ố ổ ỗ ộ ơ ờ ớ ở ỡ ợ\nù ú ủ ũ ụ ư ừ ứ ử ữ ự\nỳ ý ỷ ỹ ỵ đ";
    let upper = lower.to_uppercase();
    out.push(
        text_lines(&[
            ("ink", 28, &tones(lower)),
            ("blue", 28, &tones(&upper)),
            (
                "red",
                28,
                "Trường học vui, giảng viên tận tâm; nghiêng ngả nhưng không ngại khó.",
            ),
        ])
        .still(
            "text-vietnamese-all-tone-marks",
            G,
            "Every Vietnamese vowel with every tone mark, in lower and upper case, and a sentence.",
        ),
    );

    out.push(
        text_lines(&[
            ("ink", 28, "a\u{0300} a\u{0301} a\u{0309} a\u{0303} a\u{0323} e\u{0300} o\u{0309} u\u{0323} y\u{0303} i\u{0301}"),
            ("blue", 28, "Việt Nam, Hà Nội, Sài Gòn (precomposed)"),
            ("red", 28, "Vie\u{0323}t Nam, Ha\u{0300} No\u{0323}i (combining marks)"),
        ])
        .still(
            "text-vietnamese-combining-marks",
            G,
            "Vietnamese typed as base letters plus a combining tone mark the fonts have (U+0300, 0301, 0303, 0309, 0323), next to the same words precomposed.",
        ),
    );

    out.push(
        text_lines(&[
            ("ink", 28, "H_2O → H₂O   SO₄²⁻   xⁿ⁺¹   E = mc²   a₁ + a₂ = −b/a"),
            ("blue", 28, SUPERS),
            ("red", 28, SUBS),
            ("green", 20, &format!("{SUPERS}\n{SUBS}")),
        ])
        .still(
            "text-typeset-superscripts-subscripts",
            G,
            "Every superscript and subscript character `typeset.ts` produces, and formulas that use them.",
        ),
    );
    out.push(
        text_lines(&[
            ("ink", 28, GREEK),
            ("violet", 40, "α β γ Δ Σ Ω π"),
            ("blue", 20, GREEK),
        ])
        .still(
            "text-typeset-greek",
            G,
            "Every Greek letter `typeset.ts` produces, at three sizes.",
        ),
    );
    out.push(
        text_lines(&[
            ("ink", 28, OPERATORS),
            ("blue", 28, SETS),
            ("red", 20, &format!("{OPERATORS}\n{SETS}")),
        ])
        .still(
            "text-typeset-operators-sets-logic",
            G,
            "Every operator, relation, set and logic symbol `typeset.ts` produces.",
        ),
    );
    out.push(
        text_lines(&[
            ("ink", 28, ARROWS),
            ("green", 40, "A → B ⇒ C ⇌ D ⟶ E"),
            ("red", 20, ARROWS),
        ])
        .still(
            "text-typeset-arrows",
            G,
            "Every arrow `typeset.ts` produces.",
        ),
    );
    out.push(
        text_lines(&[
            ("ink", 28, SUPER_CAPITALS),
            ("blue", 40, "ᴬᴮᴰ ᴱᴳᴴ ᴵᴶᴷ ᴸᴹᴺ ᴼᴾᴿ ᵀᵁᵂ"),
            ("red", 20, "x^A + y^B = z^D → xᴬ + yᴮ = zᴰ"),
        ])
        .still(
            "text-typeset-superscript-capitals",
            G,
            "The superscript capitals (ᴬᴮᴰ...) that only Noto Sans has, beside Inter text.",
        ),
    );
    out.push(
        text_lines(&[
            ("ink", 28, "★ ☆ ✓ ✔ ✗ ✘ ♥ ♠ ♣ ♦ ▲ ▼ ◆ ● ■ □"),
            ("red", 28, "☀ ☁ ☂ ☎ ☑ ☒ ✌ ❄ ☘ ⚡ ✂ ✈ ✉ ✏ ✿ ❤"),
            ("blue", 40, "★ ✓ ♥ ▲ ◆"),
            ("green", 20, "➔ ➜ ➤ ⬆ ⬇ ⬛ ⬜ ⭐"),
        ])
        .still(
            "text-symbols-noto-sans-symbols-2",
            G,
            "Geometric shapes, dingbats and miscellaneous symbols that only Noto Sans Symbols 2 has.",
        ),
    );
    out.push(
        text_lines(&[
            ("ink", 28, "Inter: x → y; Noto Sans: xᴬ; Math: ∑ ∫ ≤; Symbols 2: ★ ✓ ♥"),
            ("blue", 28, "a ᴬ ∑ ★ b ᴮ ∫ ✓ c ᴰ ≤ ♥ ạ α β ₂ ⇒ ▲"),
            ("red", 20, "Công thức: S = ∑ᵢ aᵢ·xⁿ ≤ ᴬ★ → Đáp số ✓"),
            ("green", 40, "Ag ∑★ᴬ α"),
        ])
        .still(
            "text-lines-mixing-all-four-fonts",
            G,
            "Lines that use Inter, Noto Sans, Noto Sans Math and Noto Sans Symbols 2 in one line, at three sizes.",
        ),
    );

    out.push(
        text_lines(&[
            (
                "ink",
                28,
                "Fractions ½ ⅓ ¼ and ℓ µ ° ± × ÷ ‰ ‹ › « » € £ ¥ ™ © ® … – — “ ” ‘ ’",
            ),
            (
                "blue",
                28,
                "Ngày 21 tháng 11 năm 2026: Học vui! 0123456789 @#&*()[]{}<>",
            ),
            (
                "red",
                20,
                "Ký tự đặc biệt: \\ / | ~ ` ^ _ % $ ! ? ; : , . ' \"",
            ),
        ])
        .still(
            "text-punctuation-and-currency",
            G,
            "Latin punctuation, quotes, dashes, currency and digits.",
        ),
    );

    let mut b = Board::new();
    let mut y = 20.0;
    for size in [8.0, 13.0, 17.5, 24.5, 33.0, 64.0, 100.0] {
        let mut d = text("ink", 20, 40.0, y, &format!("Cỡ {size}: Hg ½"));
        d["size"] = json!(size);
        b.add(d);
        y += size * 1.25 + 8.0;
    }
    let mut d = text("red", 20, 700.0, 20.0, "300 -> 200");
    d["size"] = json!(300);
    b.add(d);
    out.push(b.still(
        "text-odd-sizes",
        G,
        "Sizes the tools never send but the format allows: fractional, small, large, and one over 200 (drawn as 200).",
    ));
    out
}

// ---------------------------------------------------------------------------------------------
// Sheets and decks

/// The scripts and emoji the board fonts lack: drawn from the fallback fonts (the web side gets
/// the same files appended to its font stack).
fn other_scripts() -> Vec<Scene> {
    const G: &str = "Other scripts";
    let mut out = Vec::new();
    let mut scripts = |name: &str, about: &str, lines: &[(&str, u32, &str)]| {
        out.push(text_lines(lines).still(name, G, about));
    };

    scripts(
        "scripts-chinese",
        "Simplified and traditional Chinese: the same sentence in both, and Han characters whose shapes differ between the two.",
        &[
            ("ink", 28, "你好，世界！欢迎来到学习课堂。"),
            ("blue", 28, "你好，世界！歡迎來到學習課堂。"),
            ("red", 28, "数学 物理 化学 / 數學 物理 化學"),
            ("green", 20, "汉字 漢字 国 國 学 學 书 書 骨 直 海 语 語"),
            ("violet", 40, "中文 Chinese 中文"),
        ],
    );
    scripts(
        "scripts-japanese",
        "Japanese: hiragana, katakana, kanji and the long vowel mark, with Latin and digits between.",
        &[
            ("ink", 28, "こんにちは、世界。ひらがなとカタカナ"),
            ("blue", 28, "日本語を勉強しています。漢字と仮名"),
            ("red", 28, "ラーメン コーヒー スペース 2026年11月21日"),
            ("green", 20, "あいうえお かきくけこ アイウエオ カキクケコ"),
            ("violet", 40, "日本語 Japanese"),
        ],
    );
    scripts(
        "scripts-japanese-punctuation",
        "Full-width punctuation next to each other (Chromium's text-spacing-trim halves a closing mark before another mark and an opening mark after one): brackets, commas, full stops, the middle dot, and a line that starts with an opening bracket (not trimmed).",
        &[
            ("ink", 28, "「こんにちは」、『日本語』（括弧）"),
            ("blue", 28, "「あ」「い」（う）。、」「 ・「」・"),
            ("red", 28, "【重要】（注）：「雨」。『本』、〈図〉《注》"),
            ("green", 28, "ラーメン、コーヒー。バナナ・リンゴ・スーパー"),
            ("violet", 20, "「まず」、「次に」、「最後に」。（以上）"),
            ("ink", 40, "「日本」、『語』。"),
        ],
    );
    scripts(
        "scripts-chinese-punctuation",
        "Full-width punctuation in Chinese: the same trimming, book marks and brackets, colon, quotes of the Latin font (never trimmed) and the full-width semicolon (see the report: next to a semicolon or a Latin quote Chromium trims in a way the Rust side does not match).",
        &[
            ("ink", 28, "《汉字》、〈词〉；（注）：「引」。"),
            ("blue", 28, "他说：“你好。”（完）；（续）"),
            ("red", 28, "第一，第二；第三。【重点】；；“好”；"),
            ("violet", 20, "《红楼梦》（上）、《水浒传》（下）。"),
        ],
    );
    scripts(
        "scripts-korean",
        "Korean: Hangul syllables with Latin and digits between.",
        &[
            ("ink", 28, "안녕하세요, 세계! 한국어를 배웁니다."),
            ("blue", 28, "수학 물리 화학 영어 2026년 11월"),
            ("red", 20, "가나다라마바사 아자차카타파하 Korean 한글"),
            ("violet", 40, "한국어 Korean"),
        ],
    );
    scripts(
        "scripts-thai",
        "Thai: vowels and tone marks above and below the consonants, no spaces between words.",
        &[
            ("ink", 28, "สวัสดีครับ ยินดีต้อนรับสู่ชั้นเรียน"),
            ("blue", 28, "ภาษาไทยมีวรรณยุกต์ ห้าเสียง ก๊ ก้ ก่ ก๋"),
            ("red", 20, "กรุงเทพมหานคร อัครราชทูต ผู้เชี่ยวชาญ Thai"),
            ("violet", 40, "ไทย Thai"),
        ],
    );
    scripts(
        "scripts-lao",
        "Lao: vowels and tone marks above and below the consonants.",
        &[
            ("ink", 28, "ສະບາຍດີ ຍິນດີຕ້ອນຮັບສູ່ຫ້ອງຮຽນ"),
            ("blue", 28, "ພາສາລາວ ວິທະຍາສາດ ຄະນິດສາດ"),
            ("red", 20, "ກ ຂ ຄ ງ ຈ ສ ຊ ຍ ດ ຕ ຖ ທ ນ ບ ປ Lao"),
            ("violet", 40, "ລາວ Lao"),
        ],
    );
    scripts(
        "scripts-khmer",
        "Khmer: stacked subscript consonants and vowels around the base.",
        &[
            ("ink", 28, "សួស្តី សូមស្វាគមន៍មកកាន់ថ្នាក់រៀន"),
            ("blue", 28, "ភាសាខ្មែរ គណិតវិទ្យា វិទ្យាសាស្ត្រ"),
            ("red", 20, "កខគឃង ចឆជឈញ ដឋឌឍណ Khmer"),
            ("violet", 40, "ខ្មែរ Khmer"),
        ],
    );
    scripts(
        "scripts-arabic",
        "Arabic on its own: joined letters, marks above and below, digits, and a right-to-left line next to a left-to-right one.",
        &[
            ("ink", 28, "مرحبا بكم في الفصل الدراسي"),
            ("blue", 28, "اَلسَّلَامُ عَلَيْكُمْ وَرَحْمَةُ اللَّهِ"),
            ("red", 28, "اللغة العربية جميلة ١٢٣٤٥ 12345"),
            (
                "green",
                20,
                "ا ب ت ث ج ح خ د ذ ر ز س ش ص ض ط ظ ع غ ف ق ك ل م ن ه و ي",
            ),
            ("violet", 40, "عربي Arabic"),
        ],
    );
    scripts(
        "scripts-hebrew",
        "Hebrew on its own: letters, points above and below, and Latin between.",
        &[
            ("ink", 28, "שלום עולם, ברוכים הבאים לכיתה"),
            ("blue", 28, "שָׁלוֹם עוֹלָם בְּרֵאשִׁית"),
            ("red", 20, "א ב ג ד ה ו ז ח ט י כ ל מ נ ס ע פ צ ק ר ש ת"),
            ("violet", 40, "עברית Hebrew"),
        ],
    );
    scripts(
        "scripts-rtl-mixed-with-vietnamese",
        "Right-to-left runs inside left-to-right lines (and the other way round): Arabic and Hebrew with Vietnamese, Latin, digits and punctuation.",
        &[
            ("ink", 28, "Xin chào مرحبا Việt Nam שלום 123 end"),
            ("blue", 28, "Bài 5: مرحبا (hello) والعالم!"),
            ("red", 28, "العام 2026 ميلادي, Hà Nội, שנת 2026"),
            ("green", 28, "Tiếng Ả Rập: العربية - Tiếng Do Thái: עברית."),
            ("violet", 20, "a مرحبا b שלום c العربية d"),
        ],
    );
    scripts(
        "scripts-hindi",
        "Hindi (Devanagari): conjuncts, vowel signs before and after the consonant, the headline.",
        &[
            ("ink", 28, "नमस्ते दुनिया, कक्षा में आपका स्वागत है।"),
            ("blue", 28, "हिन्दी भाषा क्षत्रिय ज्ञान श्रीमान् द्वारा"),
            ("red", 20, "क ख ग घ ङ च छ ज झ ञ ट ठ ड ढ ण त थ द ध न Hindi"),
            ("violet", 40, "हिन्दी Hindi"),
        ],
    );
    scripts(
        "scripts-emoji-simple",
        "Single emoji, alone and between words.",
        &[
            ("ink", 28, "😀 😂 🎉 📚 🚀 🌍 🍎 👍 💡 🔥"),
            ("blue", 28, "Hello 👋 world 🌏 học 📖 vui 😊!"),
            ("red", 20, "😀😃😄😁😆😅🤣😂🙂🙃😉😊😇🥰😍🤩😘"),
            ("violet", 40, "👍 OK 👌"),
        ],
    );
    scripts(
        "scripts-emoji-zwj",
        "Emoji joined with zero-width joiners (families, professions, the rainbow flag): each stays one picture.",
        &[
            ("ink", 28, "👨‍👩‍👧‍👦 👩‍💻 🧑‍🏫 🏳️‍🌈 👩‍❤️‍👨"),
            ("blue", 28, "👩🏽‍🔬 👨🏿‍🏫 🧑🏻‍🎓 🐻‍❄️ 🧑‍🤝‍🧑"),
            ("red", 20, "Cả nhà 👨‍👩‍👧‍👦 học 👩‍🏫 vui"),
            ("violet", 40, "👨‍👩‍👧‍👦"),
        ],
    );
    scripts(
        "scripts-emoji-skin-tones",
        "A hand and a person in each of the five skin tones, and without.",
        &[
            ("ink", 28, "👍 👍🏻 👍🏼 👍🏽 👍🏾 👍🏿"),
            ("blue", 28, "👋 👋🏻 👋🏼 👋🏽 👋🏾 👋🏿"),
            ("red", 28, "🧑 🧑🏻 🧑🏼 🧑🏽 🧑🏾 🧑🏿"),
            ("violet", 40, "🙋🏽 🙌🏾"),
        ],
    );
    scripts(
        "scripts-emoji-flags",
        "Flags from pairs of regional indicators, and a flag from tag characters.",
        &[
            ("ink", 28, "🇻🇳 🇺🇸 🇯🇵 🇫🇷 🇦🇺 🇬🇧 🇰🇷 🇹🇭"),
            ("blue", 28, "🏴󠁧󠁢󠁥󠁮󠁧󠁿 🏴󠁧󠁢󠁳󠁣󠁴󠁿 🏳️ 🏴 🏁 🚩"),
            ("red", 20, "🇻🇳🇺🇸🇯🇵 (three flags with no space) 🇫🇷🇦🇺"),
            ("violet", 40, "🇻🇳 Việt Nam"),
        ],
    );
    scripts(
        "scripts-emoji-keycaps",
        "Keycap sequences (a digit, VS16, the combining keycap), next to the plain digits.",
        &[
            ("ink", 28, "0️⃣ 1️⃣ 2️⃣ 3️⃣ 4️⃣ 5️⃣ 6️⃣ 7️⃣ 8️⃣ 9️⃣"),
            ("blue", 28, "#️⃣ *️⃣ 🔟 and 0123456789 #*"),
            ("violet", 40, "1️⃣2️⃣3️⃣"),
        ],
    );
    scripts(
        "scripts-emoji-text-vs-emoji-presentation",
        "The same characters as text (the board's symbol fonts), with VS15 (text) and with VS16 (emoji): VS16 picks the colour picture.",
        &[
            ("ink", 28, "❤ ❤︎ ❤️ ☺ ☺︎ ☺️ ✔ ✔︎ ✔️"),
            ("blue", 28, "☎ ☎︎ ☎️ ✈ ✈︎ ✈️ ☀ ☀︎ ☀️"),
            ("red", 28, "★ ⭐ ✅ ❌ ✓ ✗ ♥ ♥️"),
            ("violet", 40, "❤ ❤️"),
        ],
    );
    scripts(
        "scripts-one-line-of-everything",
        "One line mixing Latin, Vietnamese, Cyrillic, Greek, Chinese, Japanese, Korean, Thai, Lao, Khmer, Arabic, Hebrew, Hindi and emoji, at 20, 28 and 40.",
        &[
            (
                "ink",
                20,
                "Hello Việt мир Ελλάς 你好 こんにちは 한국어 สวัสดี ສະບາຍດີ សួស្តី مرحبا שלום नमस्ते 😀🇻🇳",
            ),
            (
                "blue",
                28,
                "Hello Việt мир Ελλάς 你好 こんにちは 한국어 สวัสดี ສະບາຍດີ សួស្តី مرحبا שלום नमस्ते 😀🇻🇳",
            ),
            (
                "red",
                40,
                "Hello Việt мир 你好 こんにちは 한국어 สวัสดี مرحبا שלום 😀🇻🇳",
            ),
        ],
    );
    out
}

fn sheets() -> Vec<Scene> {
    const G: &str = "Sheets";
    let mut out = Vec::new();
    let mut rng = Rng(42);

    let mut b = Board::new();
    b.add_as("s1", page(0.0, 0.0, 1280.0, 720.0, None));
    b.add(stroke(&mut rng, "red", 4, false, (200.0, 200.0), 50, true));
    b.add(text("ink", 28, 100.0, 80.0, "Trang trắng"));
    b.add(shape("ellipse", "blue", 4, [700.0, 300.0, 1100.0, 560.0]));
    out.push(b.still(
        "sheet-blank",
        G,
        "A blank sheet with ink on it: white paper, a pale edge, the board's dot grid around it.",
    ));

    for (name, n, w, h, about) in [
        (
            "sheet-deck-landscape",
            1,
            1280.0,
            720.0,
            "A landscape picture (16:9) on a 16:9 sheet.",
        ),
        (
            "sheet-deck-portrait",
            2,
            1280.0,
            720.0,
            "A portrait picture centred on a landscape sheet, with white at the sides.",
        ),
        (
            "sheet-deck-square",
            3,
            1000.0,
            600.0,
            "A square picture on a wide sheet, fitted to its height.",
        ),
    ] {
        let mut b = Board::new();
        b.add_as("s1", page(0.0, 0.0, w, h, Some(n)));
        b.add(shape("arrow", "red", 4, [100.0, 100.0, 400.0, 300.0]));
        b.add(text("blue", 28, 500.0, 60.0, "Trên hình"));
        out.push(b.still(name, G, about));
    }

    let mut b = Board::new();
    b.add_as("s1", page(0.0, 0.0, 1280.0, 720.0, Some(1)));
    b.add_as("s2", page(1400.0, 0.0, 800.0, 450.0, Some(2)));
    b.add(text("ink", 28, 100.0, 80.0, "Hình đang tải"));
    let mut loading = b.still(
        "sheet-picture-loading",
        G,
        "The ring a sheet shows while its picture is still on its way (the frame is taken before it arrives).",
    );
    loading.loading = true;
    out.push(loading);

    let mut b = Board::new();
    b.add_as("s1", page(0.0, 0.0, 1280.0, 720.0, Some(9)));
    b.add_as("s2", page(1400.0, 0.0, 600.0, 400.0, Some(12)));
    out.push(b.still(
        "sheet-picture-failed",
        G,
        "The crossed frame a sheet shows when its picture cannot be had (the deck has no such page).",
    ));

    let els = vec![
        env("s1", 1, 0, page(0.0, 0.0, 1280.0, 720.0, None)),
        env("s2", 1, 0, page(1600.0, 0.0, 1280.0, 720.0, Some(1))),
        env("s3", 1, 0, page(3200.0, 0.0, 720.0, 1280.0, Some(2))),
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
            shape("ellipse", "blue", 4, [1800.0, 400.0, 2300.0, 650.0]),
        ),
        env("i4", 1, 13, text("violet", 40, 1750.0, 60.0, "Slide 1")),
        env(
            "i5",
            1,
            14,
            shape("arrow", "green", 4, [3300.0, 100.0, 3900.0, 500.0]),
        ),
    ];
    out.push(scene(
        "sheets-view-switching",
        G,
        "Three sheets: the camera shows all of them until the teacher's view names one, then follows the view from sheet to sheet, and falls back when it names a sheet that is not there.",
        vec![
            rec(1000, "snapshot", json!({"elements": els})),
            view_rec(2000, 1, "s2"),
            view_rec(3000, 2, "s3"),
            view_rec(4000, 3, "s1"),
            view_rec(5000, 4, "nosuchsheet"),
        ],
        &[1500, 2500, 3500, 4500, 5500],
    ));
    out
}

// ---------------------------------------------------------------------------------------------
// Viewport

/// What the viewport scenes look at: ink of every kind, text and a picture on a sheet, around
/// (650, 330).
fn viewport_content() -> Vec<Value> {
    let mut rng = Rng(77);
    let mut b = Board::new();
    b.add_as("s1", page(40.0, 40.0, 1180.0, 590.0, Some(1)));
    b.add(stroke(&mut rng, "red", 4, false, (100.0, 120.0), 50, true));
    b.add(stroke(&mut rng, "ink", 8, true, (700.0, 120.0), 30, false));
    b.add(shape("rect", "blue", 4, [120.0, 300.0, 420.0, 480.0]));
    b.add(shape("ellipse", "green", 4, [480.0, 300.0, 780.0, 480.0]));
    b.add(shape("arrow", "violet", 4, [820.0, 300.0, 1150.0, 480.0]));
    b.add(shape("line", "orange", 2, [100.0, 540.0, 1150.0, 540.0]));
    b.add(text("ink", 40, 120.0, 220.0, "Học vui: x² + y² = r²"));
    b.add(text("red", 20, 700.0, 230.0, "Một dòng nhỏ\nhai dòng"));
    b.snapshot()
}

fn centred(zoom: f64, cx: f64, cy: f64, w: u32, h: u32) -> [f64; 3] {
    [
        cx - f64::from(w) / 2.0 / zoom,
        cy - f64::from(h) / 2.0 / zoom,
        zoom,
    ]
}

fn viewports() -> Vec<Scene> {
    const G: &str = "Viewport";
    let mut out = Vec::new();
    for (name, zoom, about) in [
        (
            "viewport-zoom-0-2",
            0.2,
            "The smallest zoom: the grid is hidden (steps under 12 px), a sheet's edge is 7.5 px wide.",
        ),
        (
            "viewport-zoom-0-5",
            0.5,
            "Half size: the grid is back, the edge is 3 px.",
        ),
        ("viewport-zoom-1", 1.0, "Scale 1."),
        (
            "viewport-zoom-1-37",
            1.37,
            "A fractional zoom, with the dot size taken from it.",
        ),
        (
            "viewport-zoom-2",
            2.0,
            "Twice the size: the board's strokes and text are scaled by the camera, not redrawn at another size.",
        ),
        (
            "viewport-zoom-5",
            5.0,
            "The largest zoom, close on the heading.",
        ),
    ] {
        let mut s = scene(name, G, about, Vec::new(), &[1500]);
        s.log = Some("viewport-content".into());
        let (cx, cy) = if zoom == 5.0 {
            (260.0, 235.0)
        } else {
            (650.0, 330.0)
        };
        s.view = Some(centred(zoom, cx, cy, 1280, 624));
        out.push(s);
    }
    for (name, view, about) in [
        (
            "viewport-fractional-pan",
            [123.37, -45.61, 1.0],
            "A camera at fractional board coordinates, zoom 1: the dot grid and every element sit on fractions of a pixel.",
        ),
        (
            "viewport-fractional-pan-and-zoom",
            [-77.31, 18.73, 1.37],
            "A fractional pan and zoom together.",
        ),
        (
            "viewport-negative-pan",
            [-900.0, -400.0, 0.5],
            "A camera left of and above the board, so the board is at the bottom right.",
        ),
        (
            "viewport-grid-hidden-under-12-px",
            [0.0, 0.0, 0.36],
            "Zoom 0.36: the grid step is 11.52 px, under 12, so no dots.",
        ),
        (
            "viewport-grid-shown-at-12-px",
            [0.0, 0.0, 0.375],
            "Zoom 0.375: the grid step is exactly 12 px, so the dots are drawn (with the 0.8 px minimum dot).",
        ),
        (
            "viewport-grid-offset-wraps",
            [1000.5, 700.25, 1.0],
            "A camera far from the origin, where the dot grid's offset wraps around the step.",
        ),
    ] {
        let mut s = scene(name, G, about, Vec::new(), &[1500]);
        s.log = Some("viewport-content".into());
        s.view = Some(view);
        out.push(s);
    }
    {
        let mut rng = Rng(64);
        let mut b = Board::new();
        b.add(shape("arrow", "red", 8, [1300.0, 100.0, 1200.0, 160.0]));
        b.add(text(
            "ink",
            40,
            -260.0,
            220.0,
            "Chữ bắt đầu ngoài khung nhìn",
        ));
        b.add(stroke(&mut rng, "blue", 8, true, (-30.0, 320.0), 20, false));
        b.add(shape("rect", "green", 8, [1275.0, 400.0, 1400.0, 500.0]));
        b.add(shape("ellipse", "violet", 8, [-100.0, 450.0, 3.0, 600.0]));
        b.add(shape("line", "orange", 8, [600.0, 640.0, 700.0, 660.0]));
        b.add(shape("line", "ink", 8, [600.0, -30.0, 700.0, -4.0]));
        let mut s = scene(
            "viewport-culling-elements-crossing-the-edge",
            G,
            "Elements that start just outside the stage and reach into it (and ones that only touch it): none may be dropped as off screen while a pixel of it shows.",
            b.snapshot(),
            &[1500],
        );
        s.view = Some([0.0, 0.0, 1.0]);
        out.push(s);
    }
    // The camera the board asks for, with no override: it fits whatever is drawn.
    let mut rng = Rng(8);
    let mut b = Board::new();
    for i in 0..12 {
        b.add(stroke(
            &mut rng,
            COLORS[i % 6],
            4,
            false,
            (
                200.0 + 300.0 * (i % 6) as f64,
                100.0 + 1500.0 * (i / 6) as f64,
            ),
            40,
            true,
        ));
    }
    b.add(text("ink", 40, 3000.0, 1600.0, "Góc xa"));
    out.push(b.still(
        "viewport-fit-wide-content",
        G,
        "No camera given: content larger than the stage is fitted with its margin, zoom under 1.",
    ));
    let mut b = Board::new();
    b.add(text("ink", 28, 600.0, 300.0, "Nhỏ"));
    out.push(b.still(
        "viewport-fit-small-content",
        G,
        "No camera given: small content is centred at zoom 1 (never zoomed in).",
    ));
    let mut b = Board::new();
    b.add(shape("line", "ink", 4, [300.0, 300.0, 300.0, 300.0]));
    out.push(b.still(
        "viewport-fit-single-point",
        G,
        "No camera given: content of no width or height (one unit counts).",
    ));
    out.push(scene(
        "viewport-fit-empty-board",
        G,
        "No camera given and nothing drawn: scale 1 at the origin, only the grid.",
        vec![rec(1000, "snapshot", json!({"elements": []}))],
        &[1500],
    ));
    out
}

// ---------------------------------------------------------------------------------------------
// Scene rules

fn rules_base() -> Vec<Value> {
    vec![
        env(
            "a",
            1,
            1,
            shape("line", "ink", 4, [100.0, 100.0, 500.0, 180.0]),
        ),
        env("b", 1, 2, text("blue", 28, 120.0, 240.0, "Trước khi xoá")),
        env(
            "c",
            1,
            3,
            shape("ellipse", "red", 4, [600.0, 100.0, 900.0, 300.0]),
        ),
    ]
}

fn rules() -> Vec<Scene> {
    const G: &str = "Scene rules";
    let mut out = Vec::new();

    out.push(scene(
        "rules-deleted-element",
        G,
        "A deleted element (a tombstone of a higher version) is gone.",
        vec![
            rec(1000, "snapshot", json!({"elements": rules_base()})),
            elements_rec(2000, "teacher", vec![tombstone("b", 2)]),
        ],
        &[1500, 2500],
    ));

    out.push(scene(
        "rules-tombstone-blocks-late-older-write",
        G,
        "A write older than the deletion that arrives later does not bring the element back.",
        vec![
            rec(1000, "snapshot", json!({"elements": rules_base()})),
            elements_rec(2000, "teacher", vec![tombstone("b", 3)]),
            elements_rec(
                3000,
                "student",
                vec![env("b", 2, 2, text("red", 28, 120.0, 240.0, "Muộn"))],
            ),
        ],
        &[2500, 3500],
    ));

    out.push(scene(
        "rules-higher-version-wins",
        G,
        "A later write of a higher version replaces the element; an older version arriving after it is ignored.",
        vec![
            rec(1000, "snapshot", json!({"elements": rules_base()})),
            elements_rec(
                2000,
                "teacher",
                vec![env("c", 3, 3, shape("ellipse", "blue", 8, [300.0, 300.0, 700.0, 500.0]))],
            ),
            elements_rec(
                3000,
                "teacher",
                vec![env("c", 2, 3, shape("ellipse", "green", 4, [100.0, 400.0, 300.0, 560.0]))],
            ),
        ],
        &[1500, 2500, 3500],
    ));

    let version = |t, nonce, color: &str, x: f64| {
        elements_rec(
            t,
            "teacher",
            vec![env_nonce(
                "a",
                2,
                nonce,
                1,
                shape("rect", color, 8, [x, 100.0, x + 300.0, 300.0]),
            )],
        )
    };
    out.push(scene(
        "rules-same-version-lower-nonce-wins",
        G,
        "Two writes of one version: the one with the lower nonce wins, whatever order they arrive in.",
        vec![
            rec(
                1000,
                "snapshot",
                json!({"elements": [env_nonce("a", 1, 7, 1, shape("rect", "ink", 8, [100.0, 100.0, 400.0, 300.0]))]}),
            ),
            version(2000, 50, "ink", 100.0),
            version(3000, 20, "red", 500.0),
            version(4000, 90, "green", 900.0),
        ],
        &[1500, 2500, 3500, 4500],
    ));

    out.push(scene(
        "rules-clear-then-draw-again",
        G,
        "The board is cleared, then drawn on again; a clear drops writes that were pending in the same step.",
        vec![
            rec(1000, "snapshot", json!({"elements": rules_base()})),
            rec(2000, "clear", json!({"u": "teacher"})),
            elements_rec(
                2500,
                "teacher",
                vec![
                    env("n1", 1, 1, text("orange", 40, 200.0, 200.0, "Bảng mới")),
                    env("n2", 1, 2, shape("line", "ink", 8, [200.0, 300.0, 700.0, 300.0])),
                ],
            ),
            elements_rec(3000, "teacher", vec![env("n3", 1, 3, shape("rect", "red", 4, [300.0, 350.0, 600.0, 450.0]))]),
            rec(3000, "clear", json!({"u": "teacher"})),
            elements_rec(3500, "teacher", vec![env("n4", 1, 1, text("blue", 28, 200.0, 200.0, "Sau lần xoá thứ hai"))]),
        ],
        &[1500, 2200, 2700, 3200, 3700],
    ));

    out.push(scene(
        "rules-lock-changes-nothing-drawn",
        G,
        "The lock only changes what students may do: the picture is the same before and after.",
        vec![
            rec(
                1000,
                "snapshot",
                json!({"locked": false, "elements": rules_base()}),
            ),
            rec(2000, "lock", json!({"locked": true})),
            rec(3000, "lock", json!({"locked": false})),
        ],
        &[1500, 2500, 3500],
    ));

    out.push(scene(
        "rules-snapshot-mid-log",
        G,
        "A snapshot part-way through the log merges into the board (it does not replace it); one older than what is there changes nothing.",
        vec![
            rec(1000, "snapshot", json!({"elements": rules_base()})),
            elements_rec(2000, "teacher", vec![env("a", 2, 1, shape("line", "red", 4, [100.0, 100.0, 500.0, 400.0]))]),
            rec(
                4000,
                "snapshot",
                json!({"locked": true, "elements": [
                    env("a", 1, 1, shape("line", "green", 8, [100.0, 500.0, 900.0, 500.0])),
                    env("d", 1, 4, text("violet", 40, 600.0, 400.0, "Ảnh chụp giữa chừng")),
                ]}),
            ),
        ],
        &[1500, 3500, 4500],
    ));

    let overlap = |id: &str, z: u32, color: &str, dx: f64| {
        env(
            id,
            1,
            z,
            shape(
                "rect",
                color,
                8,
                [200.0 + dx, 150.0 + dx, 500.0 + dx, 400.0 + dx],
            ),
        )
    };
    out.push(scene(
        "rules-stacking-order-by-z-then-id",
        G,
        "Overlapping elements are drawn by z, and elements of one z by id; an element written later with a lower z lies below.",
        vec![
            rec(
                1000,
                "snapshot",
                json!({"elements": [
                    overlap("zb", 5, "red", 0.0),
                    overlap("za", 5, "green", 60.0),
                    overlap("zc", 5, "blue", 120.0),
                    overlap("high", 9, "violet", 180.0),
                ]}),
            ),
            elements_rec(2000, "teacher", vec![overlap("low", 2, "orange", 90.0)]),
        ],
        &[1500, 2500],
    ));

    let mut bad_text = text("ink", 28, 400.0, 500.0, "");
    bad_text["text"] = json!("");
    out.push(scene(
        "rules-invalid-elements-are-dropped",
        G,
        "Elements the board refuses: an unknown colour, a size of 0, a stroke with a stray coordinate, empty text, a sheet that is too small, a half-named deck page. Only the good ones are drawn.",
        vec![rec(
            1000,
            "snapshot",
            json!({"elements": [
                env("good1", 1, 1, shape("rect", "blue", 4, [100.0, 100.0, 300.0, 200.0])),
                env("bad1", 1, 2, shape("rect", "pink", 4, [400.0, 100.0, 600.0, 200.0])),
                env("bad2", 1, 3, shape("rect", "red", 0, [700.0, 100.0, 900.0, 200.0])),
                env("bad3", 1, 4, json!({"type": "stroke", "color": "red", "size": 4, "highlighter": false, "points": [100.0, 300.0, 0.5, 400.0, 350.0]})),
                env("bad4", 1, 5, bad_text),
                env("bad5", 1, 6, page(1000.0, 100.0, 50.0, 50.0, None)),
                env("bad6", 1, 7, json!({"type": "page", "color": "ink", "size": 2, "x": 1000, "y": 300, "w": 200, "h": 200, "deck": DECK})),
                env("bad7", 1, 8, json!({"type": "hexagon", "color": "ink", "size": 2, "x1": 0, "y1": 0, "x2": 50, "y2": 50})),
                env("good2", 1, 9, text("green", 28, 100.0, 450.0, "Chỉ phần đúng được vẽ")),
            ]}),
        )],
        &[1500],
    ));

    let mut b = Board::new();
    b.add(shape("line", "red", 500, [200.0, 150.0, 1000.0, 150.0]));
    b.add(shape("rect", "blue", 200, [300.0, 300.0, 800.0, 560.0]));
    b.add(shape("line", "green", 199.5, [900.0, 300.0, 1100.0, 500.0]));
    out.push(b.still(
        "rules-size-capped-at-200",
        G,
        "A size over 200 is drawn as 200.",
    ));

    let mut b = Board::new();
    b.add(shape("line", "ink", 4, [100.0, 100.0, 400.0, 200.0]));
    let mut s = b.snapshot();
    s.push(rec(2000, "quiz", json!({"q": [{"t": "ignored"}]})));
    s.push(rec(
        2500,
        "snapshot",
        json!({"elements": [env("late", 1, 9, text("red", 28, 100.0, 300.0, "Sau bản ghi quiz"))]}),
    ));
    out.push(scene(
        "rules-quiz-records-are-skipped",
        G,
        "Records of the lesson's quiz game are in the log and the board ignores them.",
        s,
        &[1500, 2200, 2700],
    ));

    // Rendered in this order on one renderer; the web side draws each from a fresh replay.
    let mut rng = Rng(123);
    let mut recs = Vec::new();
    for i in 0..30u32 {
        let t = 1000 + i * 2000;
        let mut els = vec![env(
            &format!("s{i}"),
            1,
            i + 1,
            stroke(
                &mut rng,
                COLORS[i as usize % 6],
                4,
                false,
                (80.0 + 36.0 * f64::from(i), 120.0 + 12.0 * f64::from(i)),
                25,
                true,
            ),
        )];
        if i % 7 == 3 {
            els.push(tombstone(&format!("s{}", i - 2), 2));
        }
        if i % 9 == 4 {
            els.push(env(
                &format!("t{i}"),
                1,
                100 + i,
                text(
                    "ink",
                    28,
                    80.0 + 30.0 * f64::from(i),
                    450.0,
                    &format!("Lúc {i}"),
                ),
            ));
        }
        recs.push(elements_rec(t, "teacher", els));
        if i == 14 {
            recs.push(rec(t + 500, "clear", json!({"u": "teacher"})));
        }
        if i % 4 == 0 {
            recs.push(presence_rec(
                t + 800,
                "u1",
                json!({"cursor": [300, 300], "preview": {"type": "rect", "color": "red", "size": 4, "z": 900, "x1": 300, "y1": 300, "x2": 400, "y2": 380}}),
            ));
        }
    }
    let mut s = scene(
        "rules-seeking-backwards",
        G,
        "One renderer and replay draw 60 s, then 10 s, then 40 s, then 5 s and 60.5 s: each frame must equal one drawn from a fresh replay (here, the web player's).",
        recs,
        &[60_000, 10_000, 40_000, 5_000, 60_500, 20_500],
    );
    s.fresh = true;
    out.push(s);
    out
}

// ---------------------------------------------------------------------------------------------
// Presence

fn preview_stroke(points: &[f64], from: usize, sid: u32) -> Value {
    json!({"cursor": [points[points.len() - 3], points[points.len() - 2]], "preview": {"type": "stroke", "color": "green", "size": 4, "z": 50, "sid": sid, "from": from, "points": points}})
}

fn presence() -> Vec<Scene> {
    const G: &str = "Presence";
    let mut out = Vec::new();
    let base = || {
        vec![rec(
            1000,
            "snapshot",
            json!({"elements": [
                env("a", 1, 1, shape("rect", "ink", 4, [100.0, 100.0, 400.0, 250.0])),
                env("b", 1, 2, text("blue", 28, 120.0, 300.0, "Trên bảng")),
            ]}),
        )]
    };
    let mut rng = Rng(99);
    let whole = walk(&mut rng, (300.0, 350.0), 40, 9.0, Pressure::Pen);
    let finished = ink_stroke("green", 4, false, whole.clone());

    let mut recs = base();
    recs.push(presence_rec(2000, "u1", preview_stroke(&whole[..30], 0, 1)));
    recs.push(presence_rec(
        2200,
        "u1",
        preview_stroke(&whole[30..60], 10, 1),
    ));
    recs.push(presence_rec(
        2400,
        "u1",
        preview_stroke(&whole[60..], 20, 1),
    ));
    recs.push(elements_rec(3000, "u1", vec![env("g1", 1, 50, finished)]));
    out.push(scene(
        "presence-stroke-growing-then-finished",
        G,
        "A stroke someone is drawing, growing as updates add points; then their finished element replaces the preview.",
        recs,
        &[1500, 2100, 2300, 2500, 3500],
    ));

    let mut recs = base();
    recs.push(presence_rec(2000, "u1", preview_stroke(&whole[..60], 0, 1)));
    recs.push(presence_rec(
        2200,
        "u1",
        preview_stroke(&whole[90..], 30, 1),
    ));
    recs.push(presence_rec(
        2400,
        "u1",
        json!({"cursor": [500, 500], "preview": {"type": "stroke", "points": whole[90..].to_vec(), "from": 30, "sid": 2}}),
    ));
    out.push(scene(
        "presence-stroke-update-resumed-mid-way",
        G,
        "Updates that do not follow on: points from the middle of a stroke continue what is shown, a new stroke number starts again.",
        recs,
        &[2100, 2300, 2500],
    ));

    let mut recs = base();
    recs.push(presence_rec(
        2000,
        "u1",
        json!({"cursor": [700, 400], "preview": {"type": "rect", "color": "violet", "size": 4, "z": 60, "x1": 650, "y1": 350, "x2": 800, "y2": 450}}),
    ));
    recs.push(presence_rec(
        2100,
        "u2",
        json!({"cursor": [900, 400], "preview": {"type": "ellipse", "color": "orange", "size": 8, "z": 61, "x1": 1000, "y1": 450, "x2": 850, "y2": 350}}),
    ));
    recs.push(presence_rec(
        2200,
        "u3",
        json!({"cursor": [100, 500], "preview": {"type": "arrow", "color": "red", "size": 4, "z": 62, "x1": 100, "y1": 500, "x2": 400, "y2": 450}}),
    ));
    recs.push(presence_rec(
        2300,
        "u4",
        json!({"cursor": [800, 550], "preview": {"type": "text", "color": "green", "size": 28, "z": 63, "x": 700.0, "y": 520.0, "text": "đang gõ"}}),
    ));
    out.push(scene(
        "presence-shape-and-text-previews",
        G,
        "Previews of a rectangle, an ellipse (drawn backwards), an arrow and some text, each by a different person.",
        recs,
        &[2500],
    ));

    let mut recs = base();
    recs.push(presence_rec(2000, "u1", preview_stroke(&whole, 0, 1)));
    recs.push(presence_rec(
        2100,
        "u2",
        json!({"cursor": [700, 400], "preview": {"type": "rect", "color": "violet", "size": 4, "z": 60, "x1": 650, "y1": 350, "x2": 800, "y2": 450}}),
    ));
    recs.push(presence_rec(
        2200,
        "u1",
        json!({"cursor": [500, 500], "preview": {"type": "stroke", "points": [100.0, 450.0, 0.5, 300.0, 520.0, 0.5, 500.0, 450.0, 0.5], "from": 0, "sid": 2, "color": "red", "size": 8, "z": 70, "highlighter": true}}),
    ));
    out.push(scene(
        "presence-two-people-at-once",
        G,
        "Two people drawing at the same time (a stroke and a rectangle); the first then starts a highlighter stroke, which replaces her first preview but keeps her place in the order.",
        recs,
        &[2050, 2150, 2250],
    ));

    let mut recs = base();
    recs.push(presence_rec(2000, "u1", preview_stroke(&whole, 0, 1)));
    recs.push(presence_rec(
        2100,
        "u2",
        json!({"cursor": [700, 400], "preview": {"type": "rect", "color": "violet", "size": 4, "z": 60, "x1": 650, "y1": 350, "x2": 800, "y2": 450}}),
    ));
    recs.push(elements_rec(
        3000,
        "u2",
        vec![env(
            "w",
            1,
            70,
            shape("rect", "violet", 4, [650.0, 350.0, 800.0, 450.0]),
        )],
    ));
    recs.push(elements_rec(
        3500,
        "teacher",
        vec![env(
            "t",
            1,
            71,
            text("ink", 20, 900.0, 100.0, "Giáo viên viết"),
        )],
    ));
    out.push(scene(
        "presence-preview-cleared-by-that-users-elements-record",
        G,
        "A person's elements record clears their own preview only: the rectangle's owner is done, the stroke's owner is still drawing, and the teacher's write clears neither.",
        recs,
        &[2500, 3200, 3700],
    ));

    let mut recs = base();
    recs.push(presence_rec(2000, "u1", preview_stroke(&whole, 0, 1)));
    recs.push(presence_rec(
        6000,
        "u2",
        json!({"cursor": [700, 400], "preview": {"type": "rect", "color": "violet", "size": 4, "z": 60, "x1": 650, "y1": 350, "x2": 800, "y2": 450}}),
    ));
    out.push(scene(
        "presence-expires-after-the-6-second-ttl",
        G,
        "A preview is shown for 6 s after the person's last update, then dropped: the stroke at 7.9 s and gone at 8.1 s; the rectangle (sent at 6 s) outlives it.",
        recs,
        &[7900, 8100, 11900, 12100],
    ));

    let mut recs = base();
    recs.push(presence_rec(2000, "u1", json!({"cursor": [500, 400]})));
    recs.push(presence_rec(
        2100,
        "u2",
        json!({"name": "Lan", "color": "#ff0000", "cursor": [700, 450], "preview": {"type": "line", "color": "red", "size": 4, "z": 60, "x1": 600, "y1": 500, "x2": 900, "y2": 520}}),
    ));
    recs.push(presence_rec(
        2200,
        "u3",
        json!({"cursor": [100, 100], "label": "Minh"}),
    ));
    out.push(scene(
        "presence-cursors-and-names-are-not-drawn",
        G,
        "Pointers and the names that come with them are not part of the video: only the line preview is drawn.",
        recs,
        &[2500],
    ));

    let mut recs = base();
    recs.push(presence_rec(
        2000,
        "u1",
        json!({"cursor": [500, 400], "preview": {"type": "page", "color": "ink", "size": 2, "z": 5, "x": 600.0, "y": 300.0, "w": 400.0, "h": 300.0}}),
    ));
    recs.push(presence_rec(
        2100,
        "u2",
        json!({"cursor": [500, 400], "preview": {"type": "view", "color": "ink", "size": 1, "z": 5, "page": "a"}}),
    ));
    recs.push(presence_rec(
        2200,
        "u3",
        json!({"cursor": [500, 400], "preview": {"type": "stroke", "color": "red", "size": 4, "z": 5, "points": [1, 2]}}),
    ));
    out.push(scene(
        "presence-sheet-view-and-malformed-previews-are-ignored",
        G,
        "A preview that is a sheet, the view or a malformed stroke draws nothing.",
        recs,
        &[2500],
    ));

    let mut recs = base();
    recs.push(presence_rec(
        2000,
        "u1",
        json!({"cursor": [500, 400], "preview": {"ref": "a", "set": {"x2": 700.0, "y2": 300.0}}}),
    ));
    recs.push(presence_rec(
        2100,
        "u2",
        json!({"cursor": [500, 400], "preview": {"ref": "nosuchelement", "set": {"x2": 700.0}}}),
    ));
    out.push(scene(
        "presence-ref-preview-moving-an-existing-element",
        G,
        "A preview that edits an element by reference. The board patches only graphs, solids and symbols (pack elements, which neither renderer draws yet): for a rectangle or an unknown id nothing is drawn and the element stays where it is.",
        recs,
        &[1500, 2500],
    ));
    out
}

// ---------------------------------------------------------------------------------------------
// Stage sizes

fn stages() -> Vec<Scene> {
    const G: &str = "Stage sizes";
    let mut out = Vec::new();
    for (log, label) in [
        ("viewport-content", "a sheet with a picture and ink"),
        ("stage-free-board", "an unbounded board of ink and text"),
    ] {
        for (h, what) in [(624, "with the camera strip"), (664, "without it")] {
            let mut s = scene(
                &format!("stage-{log}-{h}"),
                G,
                &format!("1280x{h}, the stage {what}: {label}."),
                Vec::new(),
                &[1500],
            );
            s.height = h;
            s.log = Some(log.into());
            out.push(s);
        }
    }
    out
}

fn stage_free_board() -> Vec<Value> {
    let mut rng = Rng(31);
    let mut b = Board::new();
    for i in 0..10 {
        b.add(stroke(
            &mut rng,
            COLORS[i % 6],
            [2, 4, 8][i % 3],
            i % 4 == 3,
            (
                100.0 + 230.0 * (i % 5) as f64,
                120.0 + 700.0 * (i / 5) as f64,
            ),
            30,
            i % 2 == 0,
        ));
    }
    b.add(text("ink", 40, 200.0, 300.0, "Bảng không giới hạn"));
    b.add(shape("rect", "blue", 4, [100.0, 500.0, 1100.0, 640.0]));
    b.snapshot()
}

// ---------------------------------------------------------------------------------------------
// Dense

fn dense() -> Vec<Scene> {
    let mut rng = Rng(2026);
    let phrases = [
        "Phương trình bậc hai",
        "x² + y² = r²",
        "Học vui!",
        "α β γ → ∑ ≤ ∞",
        "H₂O, CO₂, SO₄²⁻",
        "Đáp số: 42",
        "★ ✓ ♥ ▲",
        "Ghi chú\nnhiều dòng",
        "The quick brown fox",
        "Ngày 21/11",
    ];
    let mut els = Vec::new();
    let mut z = 0;
    let mut add = |els: &mut Vec<Value>, data: Value| {
        z += 1;
        els.push(env(&format!("d{z}"), 1, z, data));
    };
    add(&mut els, page(0.0, 0.0, 1400.0, 780.0, Some(1)));
    add(&mut els, page(1500.0, 0.0, 1300.0, 780.0, None));
    add(&mut els, page(0.0, 850.0, 780.0, 450.0, Some(2)));
    for i in 0..800 {
        let at = (rng.range(40.0, 2760.0), rng.range(40.0, 1240.0));
        let color = *rng.pick(&COLORS);
        let size = *rng.pick(&SIZES);
        let data = match i % 10 {
            0..=3 => {
                let n = 8 + (rng.next() * 40.0) as usize;
                let p = if i % 2 == 0 {
                    Pressure::Pen
                } else {
                    Pressure::Mouse
                };
                {
                    let step = rng.range(3.0, 12.0);
                    ink_stroke(color, size, i % 11 == 0, walk(&mut rng, at, n, step, p))
                }
            }
            4..=6 => {
                let kind = *rng.pick(&["line", "arrow", "rect", "ellipse"]);
                shape(
                    kind,
                    color,
                    size,
                    [
                        at.0,
                        at.1,
                        at.0 + rng.range(-200.0, 200.0),
                        at.1 + rng.range(-120.0, 120.0),
                    ],
                )
            }
            _ => text(
                color,
                *rng.pick(&TEXT_SIZES),
                at.0,
                at.1,
                rng.pick(&phrases),
            ),
        };
        add(&mut els, data);
    }
    let mut recs = vec![rec(1000, "snapshot", json!({"elements": els}))];
    let mut later = Vec::new();
    for i in 0..40 {
        let at = (rng.range(40.0, 2760.0), rng.range(40.0, 1240.0));
        later.push(env(
            &format!("l{i}"),
            1,
            2000 + i,
            ink_stroke("red", 4, false, walk(&mut rng, at, 20, 8.0, Pressure::Pen)),
        ));
    }
    for i in 0..30 {
        later.push(tombstone(&format!("d{}", 4 + i * 20), 2));
    }
    recs.push(elements_rec(2000, "teacher", later));
    recs.push(presence_rec(
        2500,
        "u1",
        json!({"cursor": [400, 400], "preview": {"type": "ellipse", "color": "blue", "size": 8, "z": 9000, "x1": 300, "y1": 300, "x2": 900, "y2": 700}}),
    ));
    vec![scene(
        "dense-800-elements-of-every-kind",
        "Dense",
        "About 800 strokes, shapes and text lines (Vietnamese, maths, symbols) over three sheets, then 40 more strokes, 30 deletions and a preview.",
        recs,
        &[1500, 2200, 2700],
    )]
}

// ---------------------------------------------------------------------------------------------

fn write_log(dir: &std::path::Path, name: &str, records: &[Value]) {
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    for r in records {
        writeln!(gz, "{r}").unwrap();
    }
    std::fs::write(dir.join(format!("{name}.ndjson.gz")), gz.finish().unwrap()).unwrap();
}

/// The pictures of the deck: flat blocks and a diagonal, no text, so they look the same whatever
/// the fonts are. A mark in the top right corner shows which way up each is.
fn deck_png(like: &SkiaCanvas, w: u32, h: u32, background: u32) -> Vec<u8> {
    let mut c = like.offscreen(w, h);
    let (wf, hf) = (f64::from(w), f64::from(h));
    c.set_fill_style(Rgba::hex(background));
    c.fill_rect(0.0, 0.0, wf, hf);
    for (i, color) in [0x2f9e44u32, 0x1971c2, 0xe8590c, 0x7048e8]
        .iter()
        .enumerate()
    {
        c.set_fill_style(Rgba::hex(*color));
        c.fill_rect(
            wf * (0.06 + 0.22 * i as f64),
            hf * (0.2 + 0.06 * i as f64),
            wf * 0.17,
            hf * (0.4 - 0.03 * i as f64),
        );
    }
    let mut p = Path::new();
    p.move_to(wf * 0.03, hf * 0.94);
    p.line_to(wf * 0.97, hf * 0.67);
    c.set_stroke_style(Rgba::hex(0x1e1e1e));
    c.set_line_width(6.0);
    c.stroke(&p);
    let mut mark = Path::new();
    mark.move_to(wf, 0.0);
    mark.line_to(wf * 0.88, 0.0);
    mark.line_to(wf, wf * 0.12);
    mark.close_path();
    c.set_fill_style(Rgba::hex(0xe03131));
    c.fill(&mark);
    c.png()
}

fn main() {
    let dir = PathBuf::from(
        support::opt("--out")
            .unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus").to_owned()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "gz" || e == "png") {
            std::fs::remove_file(path).unwrap();
        }
    }
    let like = SkiaCanvas::new(8, 8, support::fonts()).unwrap();
    let decks = [
        ("deck.png", 1, 640, 360, 0xfff7e6),
        ("deck-portrait.png", 2, 360, 640, 0xe7f5ff),
        ("deck-square.png", 3, 480, 480, 0xebfbee),
    ];
    let mut pictures = serde_json::Map::new();
    for (file, n, w, h, bg) in decks {
        std::fs::write(dir.join(file), deck_png(&like, w, h, bg)).unwrap();
        pictures.insert(format!("{DECK}/{n}"), json!(file));
    }

    let mut scenes: Vec<Scene> = Vec::new();
    scenes.extend(strokes());
    scenes.extend(shapes());
    scenes.extend(texts());
    scenes.extend(other_scripts());
    scenes.extend(sheets());
    scenes.extend(viewports());
    scenes.extend(rules());
    scenes.extend(presence());
    scenes.extend(stages());
    scenes.extend(dense());

    // The logs several scenes draw.
    let shared = [
        ("viewport-content", viewport_content()),
        ("stage-free-board", stage_free_board()),
    ];
    for (name, records) in &shared {
        write_log(&dir, name, records);
    }
    let mut index = Vec::new();
    for s in &scenes {
        if s.log.is_none() {
            write_log(&dir, &s.name, &s.records);
        }
        let mut entry = json!({
            "name": s.name,
            "group": s.group,
            "about": s.about,
            "times": s.times,
            "width": s.width,
            "height": s.height,
            "log": s.log.clone().unwrap_or_else(|| s.name.clone()),
        });
        if let Some(v) = s.view {
            entry["view"] = json!(v);
        }
        if s.loading {
            entry["loading"] = json!(true);
        }
        if s.fresh {
            entry["fresh"] = json!(true);
        }
        index.push(entry);
    }
    let groups = [
        "Strokes",
        "Shapes",
        "Text",
        "Other scripts",
        "Sheets",
        "Viewport",
        "Scene rules",
        "Presence",
        "Stage sizes",
        "Dense",
    ];
    let index = json!({"deck": DECK, "pictures": pictures, "groups": groups, "scenes": index});
    std::fs::write(
        dir.join("index.json"),
        serde_json::to_string_pretty(&index).unwrap() + "\n",
    )
    .unwrap();
}
