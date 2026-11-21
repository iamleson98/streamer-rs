//! A 3D solid kept as its parameters (a pyramid with a square base of side 4 and height 5), never
//! as drawn lines: the decoding half of `board/math/solids.ts`. The drawing half is
//! [`super::solid_flat`].

use serde_json::{Map, Value};

use super::graph::MIN_BOX;
use super::{BoxRect, box_of, corners, finite, is_js_space, js_round, utf16_prefix};

pub const MAX_POINTS: usize = 8;
pub const MAX_SEGMENTS: usize = 12;
pub const MAX_NAME: usize = 3;
pub const MIN_SIDES: u32 = 3;
pub const MAX_SIDES: u32 = 12;
const MAX_DIM: f64 = 1e6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Cube,
    Cuboid,
    Prism,
    Pyramid,
    Cylinder,
    Cone,
    Sphere,
}

impl Shape {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "cube" => Self::Cube,
            "cuboid" => Self::Cuboid,
            "prism" => Self::Prism,
            "pyramid" => Self::Pyramid,
            "cylinder" => Self::Cylinder,
            "cone" => Self::Cone,
            "sphere" => Self::Sphere,
            _ => return None,
        })
    }
}

/// Which face's centre a point sits at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Centre {
    Base,
    Top,
}

/// A point the teacher adds to a polyhedron: on the segment between two points that exist at `t`
/// (0 to 1), or at the centre of the base (or of the top of a prism).
#[derive(Debug, Clone, PartialEq)]
pub enum SolidPoint {
    Between {
        name: String,
        from: String,
        to: String,
        t: f64,
    },
    Centre {
        name: String,
        centre: Centre,
    },
}

impl SolidPoint {
    pub fn name(&self) -> &str {
        match self {
            Self::Between { name, .. } | Self::Centre { name, .. } => name,
        }
    }
}

/// A segment between two points; `dashed` `None` lets the drawing decide.
#[derive(Debug, Clone, PartialEq)]
pub struct SolidSegment {
    pub from: String,
    pub to: String,
    pub dashed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Solid {
    pub shape: Shape,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    /// Edge or radius, depth and height; what a shape does not use is ignored.
    pub a: f64,
    pub b: f64,
    pub h: f64,
    /// Sides of a prism's or pyramid's base.
    pub n: u32,
    pub labels: bool,
    /// Hidden edges drawn dashed (otherwise left out).
    pub hidden: bool,
    /// The faces lightly filled.
    pub tint: bool,
    pub points: Vec<SolidPoint>,
    pub segments: Vec<SolidSegment>,
}

fn positive(v: Option<&Value>, fallback: f64) -> f64 {
    finite(v)
        .filter(|n| *n > 0.0)
        .map_or(fallback, |n| n.min(MAX_DIM))
}

fn decode_points(raw: Option<&Value>) -> Vec<SolidPoint> {
    let mut out: Vec<SolidPoint> = Vec::new();
    for p in raw
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_POINTS)
    {
        let Some(d) = p.as_object() else { continue };
        let name = d
            .get("name")
            .and_then(Value::as_str)
            .map(|n| utf16_prefix(n.trim_matches(is_js_space), MAX_NAME))
            .unwrap_or_default();
        if name.is_empty() || out.iter().any(|q| q.name() == name) {
            continue;
        }
        let centre = match d.get("centre").and_then(Value::as_str) {
            Some("base") => Some(Centre::Base),
            Some("top") => Some(Centre::Top),
            _ => None,
        };
        if let Some(centre) = centre {
            out.push(SolidPoint::Centre { name, centre });
        } else if let (Some(from), Some(to), Some(t)) = (
            d.get("from").and_then(Value::as_str),
            d.get("to").and_then(Value::as_str),
            finite(d.get("t")),
        ) {
            out.push(SolidPoint::Between {
                name,
                from: from.to_owned(),
                to: to.to_owned(),
                t: 1f64.min(0f64.max(t)),
            });
        }
    }
    out
}

fn decode_segments(raw: Option<&Value>) -> Vec<SolidSegment> {
    let mut out = Vec::new();
    for s in raw
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_SEGMENTS)
    {
        let Some(d) = s.as_object() else { continue };
        let (Some(from), Some(to)) = (
            d.get("from").and_then(Value::as_str),
            d.get("to").and_then(Value::as_str),
        ) else {
            continue;
        };
        if from == to {
            continue;
        }
        out.push(SolidSegment {
            from: from.to_owned(),
            to: to.to_owned(),
            dashed: d.get("dashed").and_then(Value::as_bool),
        });
    }
    out
}

impl Solid {
    /// Validates a solid received from the server or another client.
    pub fn decode(d: &Map<String, Value>) -> Option<Self> {
        let [x1, y1, x2, y2] = corners(d)?;
        let shape = Shape::parse(d.get("shape")?.as_str()?)?;
        let flag = |key: &str, wanted: bool| d.get(key) == Some(&Value::Bool(wanted));
        let n = finite(d.get("n")).map_or(4.0, |n| {
            js_round(n).clamp(f64::from(MIN_SIDES), f64::from(MAX_SIDES))
        });
        Some(Self {
            shape,
            x1,
            y1,
            x2,
            y2,
            a: positive(d.get("a"), 4.0),
            b: positive(d.get("b"), 4.0),
            h: positive(d.get("h"), 4.0),
            n: n as u32,
            labels: !flag("labels", false),
            hidden: !flag("hidden", false),
            tint: flag("tint", true),
            points: decode_points(d.get("points")),
            segments: decode_segments(d.get("segments")),
        })
    }

    /// The solid's box, normalized and kept between the size limits.
    pub fn rect(&self) -> BoxRect {
        box_of(self.x1, self.y1, self.x2, self.y2, MIN_BOX)
    }
}
