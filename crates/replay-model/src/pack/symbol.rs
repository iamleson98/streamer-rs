//! Physics symbols kept as what they are (a resistor, a spring of 8 coils, an incline of 30°):
//! decoded with the web player's checks, then built into lines, dots and letters around the
//! origin (y down). A port of `board/physics/symbols.ts`.

use std::f64::consts::{PI, SQRT_2, TAU};

use serde_json::{Map, Value};

use super::{BoxRect, box_of, clamp, corners, finite, js_round, utf16_prefix};

pub const MIN_SYMBOL: f64 = 24.0;
const MAX_LABEL: usize = 40;

/// Every kind of symbol, in the order of the toolbar's groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Block,
    Spring,
    Pulley,
    Pendulum,
    Incline,
    Battery,
    Resistor,
    Rheostat,
    Capacitor,
    Inductor,
    Switch,
    Bulb,
    Ammeter,
    Voltmeter,
    AcSource,
    Ground,
    ConvexLens,
    ConcaveLens,
    PlaneMirror,
    ConcaveMirror,
    ConvexMirror,
    PositiveCharge,
    NegativeCharge,
    Magnet,
}

impl SymbolKind {
    pub const ALL: [SymbolKind; 24] = [
        Self::Block,
        Self::Spring,
        Self::Pulley,
        Self::Pendulum,
        Self::Incline,
        Self::Battery,
        Self::Resistor,
        Self::Rheostat,
        Self::Capacitor,
        Self::Inductor,
        Self::Switch,
        Self::Bulb,
        Self::Ammeter,
        Self::Voltmeter,
        Self::AcSource,
        Self::Ground,
        Self::ConvexLens,
        Self::ConcaveLens,
        Self::PlaneMirror,
        Self::ConcaveMirror,
        Self::ConvexMirror,
        Self::PositiveCharge,
        Self::NegativeCharge,
        Self::Magnet,
    ];

    /// The name the element carries on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Spring => "spring",
            Self::Pulley => "pulley",
            Self::Pendulum => "pendulum",
            Self::Incline => "incline",
            Self::Battery => "battery",
            Self::Resistor => "resistor",
            Self::Rheostat => "rheostat",
            Self::Capacitor => "capacitor",
            Self::Inductor => "inductor",
            Self::Switch => "switch",
            Self::Bulb => "bulb",
            Self::Ammeter => "ammeter",
            Self::Voltmeter => "voltmeter",
            Self::AcSource => "acSource",
            Self::Ground => "ground",
            Self::ConvexLens => "convexLens",
            Self::ConcaveLens => "concaveLens",
            Self::PlaneMirror => "planeMirror",
            Self::ConcaveMirror => "concaveMirror",
            Self::ConvexMirror => "convexMirror",
            Self::PositiveCharge => "positiveCharge",
            Self::NegativeCharge => "negativeCharge",
            Self::Magnet => "magnet",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The one number a kind takes: `(min, max, initial)`.
    fn value_spec(self) -> Option<(f64, f64, f64)> {
        Some(match self {
            Self::Spring => (3.0, 16.0, 8.0),
            Self::Pendulum => (-80.0, 80.0, 30.0),
            Self::Incline => (5.0, 80.0, 30.0),
            Self::Switch => (0.0, 1.0, 1.0),
            Self::ConvexLens | Self::ConcaveLens => (20.0, 90.0, 50.0),
            _ => return None,
        })
    }

    /// Whether the label sits inside the symbol (a block's mass) rather than below it.
    pub fn label_inside(self) -> bool {
        self == Self::Block
    }

    /// Kinds drawn to fit the room whatever angle they show (they have no leads to join).
    pub fn fitted(self) -> bool {
        matches!(self, Self::Pendulum | Self::Incline | Self::Pulley)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    /// Quarter turns clockwise, 0 to 3.
    pub turns: u8,
    pub label: String,
    pub value: f64,
}

impl Symbol {
    /// Validates a symbol received from the server or another client.
    pub fn decode(d: &Map<String, Value>) -> Option<Self> {
        let [x1, y1, x2, y2] = corners(d)?;
        let kind = SymbolKind::parse(d.get("kind")?.as_str()?)?;
        let value = match kind.value_spec() {
            Some((min, max, initial)) => {
                finite(d.get("value")).map_or(initial, |v| clamp(v, min, max))
            }
            None => 0.0,
        };
        let turns = finite(d.get("turns")).map_or(0.0, |t| (js_round(t) % 4.0 + 4.0) % 4.0) as u8;
        let label = d
            .get("label")
            .and_then(Value::as_str)
            .map_or_else(String::new, |s| utf16_prefix(s, MAX_LABEL));
        Some(Self {
            kind,
            x1,
            y1,
            x2,
            y2,
            turns,
            label,
            value,
        })
    }

    /// The symbol's box (`symbolBox`).
    pub fn rect(&self) -> BoxRect {
        box_of(self.x1, self.y1, self.x2, self.y2, MIN_SYMBOL)
    }
}

/// One polyline of flat `[x, y, ...]` points.
#[derive(Debug, Clone, PartialEq)]
pub struct Prim {
    pub points: Vec<f64>,
    pub fill: bool,
    pub dashed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrawnText {
    pub text: &'static str,
    pub x: f64,
    pub y: f64,
    pub size: f64,
}

/// What a builder makes: lines, filled dots (x, y, radius) and letters, around the origin.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawing {
    pub lines: Vec<Prim>,
    pub dots: Vec<[f64; 3]>,
    pub texts: Vec<DrawnText>,
}

fn line(points: &[f64]) -> Prim {
    Prim {
        points: points.to_vec(),
        fill: false,
        dashed: false,
    }
}

fn filled(mut p: Prim) -> Prim {
    p.fill = true;
    p
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64, fill: bool) -> Prim {
    Prim {
        points: vec![x0, y0, x1, y0, x1, y1, x0, y1, x0, y0],
        fill,
        dashed: false,
    }
}

const STEPS: usize = 40;

fn arc_points(cx: f64, cy: f64, r: f64, from: f64, to: f64, steps: usize) -> Vec<f64> {
    (0..=steps)
        .flat_map(|i| {
            let a = from + ((to - from) * i as f64) / steps as f64;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}

fn circle(cx: f64, cy: f64, r: f64, fill: bool) -> Prim {
    Prim {
        points: arc_points(cx, cy, r, 0.0, TAU, STEPS),
        fill,
        dashed: false,
    }
}

fn rad(deg: f64) -> f64 {
    (deg * PI) / 180.0
}

/// An arrowhead at the tip for a line coming from `from`.
fn head(tip_x: f64, tip_y: f64, from_x: f64, from_y: f64, size: f64) -> Prim {
    let a = (tip_y - from_y).atan2(tip_x - from_x);
    let spread = rad(28.0);
    line(&[
        tip_x - size * (a - spread).cos(),
        tip_y - size * (a - spread).sin(),
        tip_x,
        tip_y,
        tip_x - size * (a + spread).cos(),
        tip_y - size * (a + spread).sin(),
    ])
}

/// Leads from both ends to a part `span` wide in the middle.
fn leads(hw: f64, span: f64) -> [Prim; 2] {
    [
        line(&[-hw, 0.0, -span / 2.0, 0.0]),
        line(&[span / 2.0, 0.0, hw, 0.0]),
    ]
}

fn text(text: &'static str, x: f64, y: f64, size: f64) -> DrawnText {
    DrawnText { text, x, y, size }
}

fn lines(lines: Vec<Prim>) -> Drawing {
    Drawing {
        lines,
        ..Drawing::default()
    }
}

fn meter(letter: &'static str, w: f64, h: f64) -> Drawing {
    let r = (h * 0.5).min(w * 0.3);
    let mut l = vec![circle(0.0, 0.0, r, false)];
    l.extend(leads(w / 2.0, 2.0 * r));
    Drawing {
        lines: l,
        dots: vec![],
        texts: vec![text(letter, 0.0, 0.0, r * 1.2)],
    }
}

fn charge(plus: bool, w: f64, h: f64) -> Drawing {
    let r = w.min(h) * 0.45;
    let mut l = vec![
        circle(0.0, 0.0, r, false),
        line(&[-r * 0.5, 0.0, r * 0.5, 0.0]),
    ];
    if plus {
        l.push(line(&[0.0, -r * 0.5, 0.0, r * 0.5]));
    }
    lines(l)
}

/// A thin lens on its axis: arrowheads out for converging, in for diverging, F on both sides.
fn lens(w: f64, h: f64, value: f64, converging: bool) -> Drawing {
    let hw = w / 2.0;
    let hh = h / 2.0;
    let f = (value / 100.0) * hw;
    let s = 10f64.min(hh * 0.15);
    let ends = if converging {
        [
            line(&[-s, -hh + s, 0.0, -hh, s, -hh + s]),
            line(&[-s, hh - s, 0.0, hh, s, hh - s]),
        ]
    } else {
        [
            line(&[-s, -hh, 0.0, -hh + s, s, -hh]),
            line(&[-s, hh, 0.0, hh - s, s, hh]),
        ]
    };
    let mut l = vec![line(&[-hw, 0.0, hw, 0.0]), line(&[0.0, -hh, 0.0, hh])];
    l.extend(ends);
    Drawing {
        lines: l,
        dots: vec![[f, 0.0, 3.0], [-f, 0.0, 3.0]],
        texts: vec![text("F", f, 15.0, 13.0), text("F'", -f, 15.0, 13.0)],
    }
}

/// A spherical mirror seen from the side it reflects on: the arc with hatching behind it, and the
/// principal axis.
fn mirror(w: f64, h: f64, concave: bool) -> Drawing {
    let hw = w / 2.0;
    let hh = h / 2.0;
    let sag = (hh * 0.38).min(hw * 0.6);
    let r = (hh * hh + sag * sag) / (2.0 * sag);
    let reach = (hh / r).min(1.0).asin();
    let vertex = if concave {
        -hw + 12f64.min(hw * 0.4)
    } else {
        hw - 2.0
    };
    let centre = vertex + if concave { r } else { -r };
    let arc: Vec<[f64; 2]> = (0..31)
        .map(|i| {
            let a = -reach + (2.0 * reach * i as f64) / 30.0;
            [
                centre + (if concave { -r } else { r }) * a.cos(),
                r * a.sin(),
            ]
        })
        .collect();
    let back = 10f64.min(hw * 0.4);
    let mut l = vec![
        line(&[-hw, 0.0, hw, 0.0]),
        line(&arc.iter().flatten().copied().collect::<Vec<_>>()),
    ];
    for [x, y] in arc.iter().step_by(3) {
        l.push(line(&[*x, *y, x - back, hh.min(y + back)]));
    }
    lines(l)
}

/// Builds a symbol for the room it has (`buildSymbol`).
pub fn build_symbol(kind: SymbolKind, w: f64, h: f64, value: f64) -> Drawing {
    match kind {
        SymbolKind::Block => lines(vec![rect(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0, true)]),
        SymbolKind::Spring => {
            let hw = w / 2.0;
            let lead = (w * 0.12).min(16.0);
            let coils = js_round(value);
            let amp = h * 0.4;
            let mut points = vec![-hw, 0.0, -hw + lead, 0.0];
            let mut i = 0.0;
            while i < coils * 2.0 {
                points.push(-hw + lead + ((w - 2.0 * lead) * (i + 0.5)) / (coils * 2.0));
                points.push(if i % 2.0 == 0.0 { -amp } else { amp });
                i += 1.0;
            }
            points.extend([hw - lead, 0.0, hw, 0.0]);
            lines(vec![line(&points)])
        }
        SymbolKind::Pulley => {
            let r = w.min(h) * 0.4;
            Drawing {
                lines: vec![
                    circle(0.0, 0.0, r, false),
                    line(&[0.0, 0.0, 0.0, -h / 2.0]),
                    line(&[-r, 0.0, -r, h / 2.0]),
                    line(&[r, 0.0, r, h / 2.0]),
                ],
                dots: vec![[0.0, 0.0, 3f64.max(r * 0.1)]],
                texts: vec![],
            }
        }
        SymbolKind::Pendulum => {
            let angle = rad(value);
            let top = -h / 2.0;
            let length = h * 0.85;
            let bob = 6f64.max(w.min(h) * 0.07);
            let bx = length * angle.sin();
            let by = top + length * angle.cos();
            let arc_r = length * 0.3;
            let mut l = vec![
                line(&[-w * 0.25, top, w * 0.25, top]),
                Prim {
                    points: vec![0.0, top, 0.0, top + length],
                    fill: false,
                    dashed: true,
                },
                line(&[0.0, top, bx, by]),
                circle(bx, by, bob, true),
            ];
            let mut texts = vec![];
            if angle != 0.0 {
                l.push(line(&arc_points(
                    0.0,
                    top,
                    arc_r,
                    PI / 2.0,
                    PI / 2.0 - angle,
                    24,
                )));
                texts.push(text(
                    "θ",
                    arc_r * 1.4 * (angle / 2.0).sin(),
                    top + arc_r * 1.4 * (angle / 2.0).cos(),
                    15.0,
                ));
            }
            Drawing {
                lines: l,
                dots: vec![],
                texts,
            }
        }
        SymbolKind::Incline => incline(w, h, value),
        SymbolKind::Battery => {
            let gap = 4f64.max(w * 0.07);
            Drawing {
                lines: vec![
                    line(&[-w / 2.0, 0.0, -gap, 0.0]),
                    line(&[-gap, -h * 0.45, -gap, h * 0.45]),
                    rect(gap - 2.0, -h * 0.25, gap + 2.0, h * 0.25, true),
                    line(&[gap, 0.0, w / 2.0, 0.0]),
                ],
                dots: vec![],
                texts: vec![
                    text("+", -gap - 9.0, -h * 0.4, 13.0),
                    text("−", gap + 9.0, -h * 0.4, 13.0),
                ],
            }
        }
        SymbolKind::Resistor => {
            let bw = w * 0.5;
            let bh = (h * 0.55).min(bw * 0.45);
            let mut l = leads(w / 2.0, bw).to_vec();
            l.push(rect(-bw / 2.0, -bh / 2.0, bw / 2.0, bh / 2.0, false));
            lines(l)
        }
        SymbolKind::Rheostat => {
            let bw = w * 0.5;
            let bh = (h * 0.5).min(bw * 0.4);
            let (x0, y0, x1, y1) = (-bw * 0.4, bh * 0.9, bw * 0.4, -bh * 0.9);
            let mut l = leads(w / 2.0, bw).to_vec();
            l.push(rect(-bw / 2.0, -bh / 2.0, bw / 2.0, bh / 2.0, false));
            l.push(line(&[x0, y0, x1, y1]));
            l.push(head(x1, y1, x0, y0, 9.0));
            lines(l)
        }
        SymbolKind::Capacitor => {
            let gap = 4f64.max(w * 0.06);
            lines(vec![
                line(&[-w / 2.0, 0.0, -gap, 0.0]),
                line(&[gap, 0.0, w / 2.0, 0.0]),
                line(&[-gap, -h * 0.4, -gap, h * 0.4]),
                line(&[gap, -h * 0.4, gap, h * 0.4]),
            ])
        }
        SymbolKind::Inductor => {
            let span = w * 0.7;
            let loops = 4usize;
            let r = (span / (loops as f64 * 2.0)).min(h * 0.5);
            let lead = (w - r * 2.0 * loops as f64) / 2.0;
            let mut points = vec![-w / 2.0, 0.0, -w / 2.0 + lead, 0.0];
            for i in 0..loops {
                let cx = -w / 2.0 + lead + r + i as f64 * 2.0 * r;
                points.extend(arc_points(cx, 0.0, r, PI, TAU, 12));
            }
            points.extend([w / 2.0 - lead, 0.0, w / 2.0, 0.0]);
            lines(vec![line(&points)])
        }
        SymbolKind::Switch => {
            let ends = w * 0.3;
            let closed = value >= 0.5;
            let length = (w * 0.6).min(h * 0.9);
            let lever = if closed {
                [ends, 0.0]
            } else {
                [-ends + length * rad(30.0).cos(), -length * rad(30.0).sin()]
            };
            Drawing {
                lines: vec![
                    line(&[-w / 2.0, 0.0, -ends, 0.0]),
                    line(&[ends, 0.0, w / 2.0, 0.0]),
                    line(&[-ends, 0.0, lever[0], lever[1]]),
                ],
                dots: vec![[-ends, 0.0, 3.0], [ends, 0.0, 3.0]],
                texts: vec![],
            }
        }
        SymbolKind::Bulb => {
            let r = (h * 0.5).min(w * 0.3);
            let d = r * (SQRT_2 / 2.0);
            let mut l = vec![
                circle(0.0, 0.0, r, false),
                line(&[-d, -d, d, d]),
                line(&[-d, d, d, -d]),
            ];
            l.extend(leads(w / 2.0, 2.0 * r));
            lines(l)
        }
        SymbolKind::Ammeter => meter("A", w, h),
        SymbolKind::Voltmeter => meter("V", w, h),
        SymbolKind::AcSource => {
            let r = (h * 0.5).min(w * 0.3);
            let wave: Vec<f64> = (0..21)
                .flat_map(|i| {
                    [
                        -r * 0.6 + (r * 1.2 * i as f64) / 20.0,
                        -r * 0.3 * ((TAU * i as f64) / 20.0).sin(),
                    ]
                })
                .collect();
            let mut l = vec![circle(0.0, 0.0, r, false), line(&wave)];
            l.extend(leads(w / 2.0, 2.0 * r));
            lines(l)
        }
        SymbolKind::Ground => lines(vec![
            line(&[0.0, -h / 2.0, 0.0, -h * 0.1]),
            line(&[-w / 2.0, -h * 0.1, w / 2.0, -h * 0.1]),
            line(&[-w * 0.3, h * 0.2, w * 0.3, h * 0.2]),
            line(&[-w * 0.12, h * 0.5, w * 0.12, h * 0.5]),
        ]),
        SymbolKind::ConvexLens => lens(w, h, value, true),
        SymbolKind::ConcaveLens => lens(w, h, value, false),
        SymbolKind::PlaneMirror => {
            let count = 2f64.max((h / 14.0).floor());
            let reach = (w * 0.25).min(12.0);
            let mut l = vec![line(&[0.0, -h / 2.0, 0.0, h / 2.0])];
            let mut i = 0.0;
            while i < count {
                let y = -h / 2.0 + (h * i) / count;
                l.push(line(&[0.0, y, reach, y + reach]));
                i += 1.0;
            }
            lines(l)
        }
        SymbolKind::ConcaveMirror => mirror(w, h, true),
        SymbolKind::ConvexMirror => mirror(w, h, false),
        SymbolKind::PositiveCharge => charge(true, w, h),
        SymbolKind::NegativeCharge => charge(false, w, h),
        SymbolKind::Magnet => {
            let size = (h * 0.55).min(w * 0.22);
            Drawing {
                lines: vec![
                    rect(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0, false),
                    line(&[0.0, -h / 2.0, 0.0, h / 2.0]),
                    filled(line(&[
                        -w / 2.0,
                        -h / 2.0,
                        0.0,
                        -h / 2.0,
                        0.0,
                        h / 2.0,
                        -w / 2.0,
                        h / 2.0,
                        -w / 2.0,
                        -h / 2.0,
                    ])),
                ],
                dots: vec![],
                texts: vec![
                    text("N", -w / 4.0, 0.0, size),
                    text("S", w / 4.0, 0.0, size),
                ],
            }
        }
    }
}

fn incline(w: f64, h: f64, value: f64) -> Drawing {
    let angle = rad(value);
    let base = w.min(h / angle.tan());
    let rise = base * angle.tan();
    let (ax, ay, bx, by, cx, cy) = (
        -base / 2.0,
        rise / 2.0,
        base / 2.0,
        rise / 2.0,
        base / 2.0,
        -rise / 2.0,
    );
    let slope = [angle.cos(), -angle.sin()];
    let up = [-angle.sin(), -angle.cos()];
    let size = base * 0.14;
    let px = ax + (cx - ax) * 0.55;
    let py = ay + (cy - ay) * 0.55;
    let corner = |s: f64, n: f64| [px + slope[0] * s + up[0] * n, py + slope[1] * s + up[1] * n];
    let block: Vec<f64> = [
        corner(-size / 2.0, 0.0),
        corner(size / 2.0, 0.0),
        corner(size / 2.0, size),
        corner(-size / 2.0, size),
        corner(-size / 2.0, 0.0),
    ]
    .into_iter()
    .flatten()
    .collect();
    let r = base * 0.18;
    let sq = base * 0.05;
    Drawing {
        lines: vec![
            filled(line(&[ax, ay, bx, by, cx, cy, ax, ay])),
            filled(line(&block)),
            line(&arc_points(ax, ay, r, 0.0, -angle, 20)),
            line(&[bx - sq, by, bx - sq, by - sq, bx, by - sq]),
        ],
        dots: vec![],
        texts: vec![text(
            "θ",
            ax + r * 1.55 * (angle / 2.0).cos(),
            ay - r * 1.55 * (angle / 2.0).sin(),
            15.0,
        )],
    }
}
