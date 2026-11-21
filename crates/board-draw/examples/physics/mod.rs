//! The physics pack's scenes: every symbol in the four orientations, at its default size and
//! stretched, in every colour, with each kind's one number across its range, with labels, at every
//! zoom and under a remote user's previews.

use serde_json::{Value, json};

use super::{Board, Scene, centred, presence_rec, scene};

const G: &str = "Physics";

/// Every kind and its default size (`DEFAULT_SIZES`).
const KINDS: [(&str, f64, f64); 24] = [
    ("block", 90.0, 60.0),
    ("spring", 160.0, 50.0),
    ("pulley", 120.0, 120.0),
    ("pendulum", 150.0, 200.0),
    ("incline", 260.0, 180.0),
    ("battery", 100.0, 50.0),
    ("resistor", 120.0, 40.0),
    ("rheostat", 140.0, 50.0),
    ("capacitor", 100.0, 50.0),
    ("inductor", 140.0, 40.0),
    ("switch", 120.0, 50.0),
    ("bulb", 100.0, 60.0),
    ("ammeter", 100.0, 60.0),
    ("voltmeter", 100.0, 60.0),
    ("acSource", 100.0, 60.0),
    ("ground", 50.0, 60.0),
    ("convexLens", 300.0, 160.0),
    ("concaveLens", 300.0, 160.0),
    ("planeMirror", 100.0, 180.0),
    ("concaveMirror", 200.0, 180.0),
    ("convexMirror", 200.0, 180.0),
    ("positiveCharge", 60.0, 60.0),
    ("negativeCharge", 60.0, 60.0),
    ("magnet", 160.0, 50.0),
];

const COLORS: [&str; 6] = ["ink", "red", "orange", "green", "blue", "violet"];

fn size_of(kind: &str) -> (f64, f64) {
    let k = KINDS.iter().find(|k| k.0 == kind).unwrap();
    (k.1, k.2)
}

/// A symbol with its top left corner at `at` and `scale` times its default size.
fn symbol(kind: &str, color: &str, size: f64, at: (f64, f64), scale: f64, extra: Value) -> Value {
    let (w, h) = size_of(kind);
    let mut el = json!({"type": "symbol", "kind": kind, "color": color, "size": size,
        "x1": at.0, "y1": at.1, "x2": at.0 + w * scale, "y2": at.1 + h * scale});
    el.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    el
}

/// The four turns of a kind side by side, a label under each.
fn turns_row(kind: &str, about: &str) -> Scene {
    let (w, h) = size_of(kind);
    let mut b = Board::new();
    let gap = 60.0;
    let cell = w.max(h) + gap;
    for turns in 0..4 {
        b.add(symbol(
            kind,
            COLORS[turns % COLORS.len()],
            3.0,
            (60.0 + cell * turns as f64, 120.0),
            1.0,
            json!({"turns": turns, "label": format!("{} {}", kind, turns)}),
        ));
    }
    let mut s = b.still(
        &format!("physics-{}-four-turns", kind.to_lowercase()),
        G,
        about,
    );
    s.view = Some(centred(0.8, 60.0 + cell * 2.0, 220.0, 1280, 624));
    s
}

fn kinds() -> Vec<Scene> {
    KINDS
        .iter()
        .map(|(kind, _, _)| {
            turns_row(
                kind,
                "At its default size in the four quarter turns, each in another colour, with a label (a block's sits inside it).",
            )
        })
        .collect()
}

/// Sizes, strokes and colours.
fn looks() -> Vec<Scene> {
    let mut out = Vec::new();
    let mut b = Board::new();
    for (i, (kind, _, _)) in KINDS.iter().enumerate() {
        let (col, row) = ((i % 6) as f64, (i / 6) as f64);
        b.add(symbol(
            kind,
            COLORS[i % COLORS.len()],
            1.0 + (i % 5) as f64 * 2.0,
            (30.0 + col * 205.0, 40.0 + row * 150.0),
            0.6,
            json!({"turns": i % 4}),
        ));
    }
    let mut s = b.still(
        "physics-every-kind-small-and-mixed",
        G,
        "All 24 kinds at 0.6 of their default size, stroke widths from 1 to 9, the six colours and every turn.",
        );
    s.view = Some([0.0, 0.0, 1.0]);
    out.push(s);

    let mut b = Board::new();
    for (i, (kind, _, _)) in KINDS.iter().enumerate() {
        let (w, h) = size_of(kind);
        let at = (40.0 + (i % 4) as f64 * 300.0, 30.0 + (i / 4) as f64 * 100.0);
        b.add(
            json!({"type": "symbol", "kind": kind, "color": "ink", "size": 2,
            "x1": at.0, "y1": at.1, "x2": at.0 + w * 2.0, "y2": at.1 + h * 0.5}),
        );
    }
    out.push(b.still(
        "physics-stretched-wide-and-flat",
        G,
        "Every kind pulled twice as wide and half as high: round parts stay round, leads stay at the ends.",
    ));

    let mut b = Board::new();
    for (i, (kind, _, _)) in KINDS.iter().enumerate() {
        let (w, h) = size_of(kind);
        let at = (40.0 + (i % 8) as f64 * 150.0, 40.0 + (i / 8) as f64 * 190.0);
        b.add(
            json!({"type": "symbol", "kind": kind, "color": "blue", "size": 3,
            "x1": at.0, "y1": at.1, "x2": at.0 + h * 0.5, "y2": at.1 + w * 0.6, "turns": 1}),
        );
    }
    out.push(b.still(
        "physics-stretched-tall-and-turned",
        G,
        "Every kind pulled tall and turned a quarter: the room the symbol is built for is the turned one.",
    ));

    let mut b = Board::new();
    for (i, (kind, _, _)) in KINDS.iter().enumerate() {
        let at = (20.0 + (i % 8) as f64 * 155.0, 30.0 + (i / 8) as f64 * 150.0);
        b.add(
            json!({"type": "symbol", "kind": kind, "color": "red", "size": 2,
            "x1": at.0, "y1": at.1, "x2": at.0 + 5.0, "y2": at.1 + 9.0}),
        );
    }
    out.push(b.still(
        "physics-smallest-box",
        G,
        "Boxes dragged out smaller than 24 units: the symbol keeps its minimum size.",
    ));
    out
}

/// The one number a kind has, across and beyond its range.
fn values() -> Vec<Scene> {
    let mut out = Vec::new();
    let row = |kind: &str, vals: &[f64], name: &str, about: &str, scale: f64| {
        let mut b = Board::new();
        for (i, v) in vals.iter().enumerate() {
            let at = (30.0 + (i % 4) as f64 * 310.0, 30.0 + (i / 4) as f64 * 290.0);
            b.add(symbol(
                kind,
                "ink",
                3.0,
                at,
                scale,
                json!({"value": v, "label": format!("{v}")}),
            ));
        }
        b.still(name, G, about)
    };
    out.push(row(
        "spring",
        &[3.0, 5.0, 8.0, 12.0, 16.0, 1.0, 99.0, 7.6],
        "physics-spring-coils",
        "A spring from 3 to 16 coils, then values outside the range (clamped) and a fraction (rounded).",
        1.0,
    ));
    out.push(row(
        "pendulum",
        &[-80.0, -30.0, 0.0, 15.0, 30.0, 60.0, 80.0, 200.0],
        "physics-pendulum-angles",
        "A pendulum from -80 to 80 degrees, at rest (no arc, no angle letter) and past the range.",
        0.7,
    ));
    out.push(row(
        "incline",
        &[5.0, 15.0, 30.0, 45.0, 60.0, 80.0, 2.0, 90.0],
        "physics-incline-angles",
        "An incline from 5 to 80 degrees; steep ones are cut to the room's height, then the clamps.",
        0.9,
    ));
    out.push(row(
        "switch",
        &[0.0, 1.0, 0.4, 0.5, 0.6, 7.0, -3.0, 0.499],
        "physics-switch-open-and-closed",
        "A switch open and closed, and the values either side of the one half it turns at.",
        1.0,
    ));
    for kind in ["convexLens", "concaveLens"] {
        out.push(row(
            kind,
            &[20.0, 35.0, 50.0, 75.0, 90.0, 0.0, 120.0, 22.5],
            &format!("physics-{}-focal-distance", kind.to_lowercase()),
            "A lens with its focal points from 20 to 90 per cent of the half width, and values outside.",
            0.9,
        ));
    }
    let mut b = Board::new();
    for (i, v) in [json!(3.0), json!(99.0), json!("x")].iter().enumerate() {
        b.add(symbol(
            "resistor",
            "ink",
            3.0,
            (60.0, 80.0 + i as f64 * 100.0),
            1.0,
            json!({"value": v}),
        ));
    }
    out.push(b.still(
        "physics-value-on-a-kind-without-one",
        G,
        "A value on a resistor is ignored; so is a spring's value that is not a number (it takes its initial 8 coils).",
    ));
    out
}

fn labels() -> Vec<Scene> {
    let mut b = Board::new();
    let cases = [
        ("resistor", "R = 10 Ω"),
        ("battery", "E = 12 V"),
        ("block", "m = 2 kg"),
        ("block", ""),
        ("bulb", "Đèn 1"),
        ("convexLens", "thấu kính hội tụ"),
        ("magnet", "Nam châm"),
        ("ground", "0 V"),
        ("incline", "α = 30°"),
        ("spring", "k = 100 N/m"),
        ("pendulum", "l = 1 m"),
        (
            "capacitor",
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        ),
    ];
    for (i, (kind, label)) in cases.iter().enumerate() {
        let at = (30.0 + (i % 4) as f64 * 310.0, 20.0 + (i / 4) as f64 * 200.0);
        b.add(symbol(kind, "ink", 3.0, at, 0.8, json!({"label": label})));
    }
    let mut out = vec![b.still(
        "physics-labels",
        G,
        "Labels with units, Greek, Vietnamese; a block's sits inside it (italic), the others' below; one is cut to 40 characters.",
    )];

    let mut b = Board::new();
    for (box_, turns) in [
        ([100.0, 100.0, 200.0, 220.0], 0),
        ([260.0, 100.0, 310.0, 130.0], 1),
        ([360.0, 100.0, 460.0, 124.0], 0),
        ([520.0, 100.0, 570.0, 130.0], 3),
        ([640.0, 100.0, 700.0, 300.0], 0),
    ]
    .iter()
    {
        b.add(json!({"type": "symbol", "kind": "bulb", "color": "red", "size": 3,
            "x1": box_[0], "y1": box_[1], "x2": box_[2], "y2": box_[3], "turns": turns, "label": "Đ"}));
    }
    out.push(b.still(
        "physics-labels-in-tight-boxes",
        G,
        "A label under symbols in boxes with little room: the symbol keeps at least its minimum and the label sits at the bottom of the box.",
    ));
    out
}

fn zooms() -> Vec<Scene> {
    let log = "physics-content";
    let mut out = Vec::new();
    for (name, zoom, centre) in [
        ("physics-zoom-0-2", 0.2, (650.0, 400.0)),
        ("physics-zoom-0-5", 0.5, (650.0, 400.0)),
        ("physics-zoom-1-37-fractional-pan", 1.37, (333.3, 211.7)),
        ("physics-zoom-3", 3.0, (240.0, 160.0)),
        ("physics-zoom-5", 5.0, (190.5, 110.25)),
    ] {
        let mut s = scene(
            name,
            G,
            "The same board at another zoom and pan: hairlines stay at the board's width, lettering is scaled.",
            Vec::new(),
            &[1500],
        );
        s.log = Some(log.into());
        s.view = Some(centred(zoom, centre.0, centre.1, 1280, 624));
        out.push(s);
    }
    out
}

fn board_of_previews() -> Board {
    let mut b = Board::new();
    b.add_as(
        "s1",
        symbol("spring", "ink", 3.0, (80.0, 80.0), 1.0, json!({"value": 6})),
    );
    b.add_as(
        "s2",
        symbol("incline", "green", 3.0, (400.0, 80.0), 1.0, json!({})),
    );
    b.add_as(
        "s3",
        symbol(
            "convexLens",
            "blue",
            3.0,
            (80.0, 300.0),
            1.0,
            json!({"value": 50}),
        ),
    );
    b.add_as(
        "s4",
        symbol(
            "resistor",
            "red",
            3.0,
            (500.0, 340.0),
            1.0,
            json!({"label": "R"}),
        ),
    );
    b
}

fn previews() -> Vec<Scene> {
    let at = |recs: &mut Vec<Value>, t: u32, user: &str, el: &str, set: Value| {
        recs.push(presence_rec(
            t,
            user,
            json!({"cursor": [500, 400], "preview": {"ref": el, "set": set}}),
        ));
    };
    let mut out = Vec::new();

    let mut recs = board_of_previews().snapshot();
    at(
        &mut recs,
        2000,
        "u1",
        "s1",
        json!({"x2": 380.0, "y2": 200.0, "value": 11}),
    );
    at(
        &mut recs,
        2100,
        "u2",
        "s2",
        json!({"value": 55, "turns": 2}),
    );
    out.push(scene(
        "physics-preview-resized-and-valued",
        G,
        "Two people drag symbols: a spring stretched and given more coils, an incline made steeper and turned (the board shows only the preview).",
        recs,
        &[1500, 2500],
    ));

    let mut recs = board_of_previews().snapshot();
    at(&mut recs, 2000, "u1", "s3", json!({"value": 80}));
    at(
        &mut recs,
        2100,
        "u1",
        "s3",
        json!({"value": 25, "label": "f = 25"}),
    );
    at(
        &mut recs,
        2200,
        "u2",
        "s4",
        json!({"kind": "bulb", "color": "orange"}),
    );
    out.push(scene(
        "physics-preview-value-slider-and-kind-change",
        G,
        "A lens's focal slider moving (the last preview wins); a resistor changed into a bulb and recoloured.",
        recs,
        &[1500, 2050, 2500],
    ));

    let mut recs = board_of_previews().snapshot();
    at(&mut recs, 2000, "u1", "s1", json!({"kind": "teleporter"}));
    at(&mut recs, 2100, "u2", "s2", json!({"x1": "left"}));
    out.push(scene(
        "physics-preview-that-does-not-decode-is-ignored",
        G,
        "A preview with an unknown kind or a corner that is not a number: nothing is drawn for it and the symbol stays as it is.",
        recs,
        &[1500, 2500],
    ));

    let mut recs = board_of_previews().snapshot();
    at(&mut recs, 2000, "u1", "s4", json!({"x2": 900.0}));
    recs.push(presence_rec(2500, "u1", json!({"cursor": [500, 400]})));
    out.push(scene(
        "physics-preview-ends-and-the-symbol-returns",
        G,
        "A preview replaced by a cursor alone: the symbol is drawn as it was.",
        recs,
        &[2200, 2700],
    ));
    out
}

pub fn scenes() -> Vec<Scene> {
    let mut out = kinds();
    out.extend(looks());
    out.extend(values());
    out.extend(labels());
    out.extend(zooms());
    out.extend(previews());
    out
}

/// The logs several scenes draw.
pub fn shared_logs() -> Vec<(&'static str, Vec<Value>)> {
    let mut b = Board::new();
    for (i, (kind, _, _)) in KINDS.iter().enumerate() {
        let at = (40.0 + (i % 6) as f64 * 210.0, 40.0 + (i / 6) as f64 * 200.0);
        b.add(symbol(
            kind,
            COLORS[i % 6],
            3.0,
            at,
            0.7,
            json!({"turns": i % 4, "label": "A"}),
        ));
    }
    vec![("physics-content", b.snapshot())]
}
