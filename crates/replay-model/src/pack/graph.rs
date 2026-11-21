//! A graph on the board: axes, grid and curves kept as their formulas (`board/math/graph.ts`).
//! Everything received from another client passes [`Graph::decode`], which clamps or drops what is
//! out of bounds.

use serde_json::{Map, Value};

use super::expression::{MAX_EXPRESSION_LENGTH, Params};
use super::{BoxRect, box_of, clamp, corners, finite, utf16_len, utf16_prefix};
use crate::element::ColorToken;

pub const MAX_SERIES: usize = 8;
const MAX_PARAMS: usize = 8;
/// The smallest box a graph or a solid is drawn in, either way.
pub const MIN_BOX: f64 = 80.0;
const MAX_LABEL: usize = 24;
const MAX_VALUE: f64 = 1e9;
const MIN_SPAN: f64 = 1e-6;

/// Points worth marking on a curve, found numerically from its samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Roots,
    Extrema,
    Intercept,
}

const MARKS: [(&str, Mark); 3] = [
    ("roots", Mark::Roots),
    ("extrema", Mark::Extrema),
    ("intercept", Mark::Intercept),
];

/// y = f(x), optionally with the area under it shaded between two x values.
#[derive(Debug, Clone, PartialEq)]
pub struct FnSeries {
    pub color: ColorToken,
    pub dash: bool,
    pub expr: String,
    pub shade: Option<(f64, f64)>,
    pub marks: Vec<Mark>,
}

/// (x(t), y(t)) for t from `t.0` to `t.1`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSeries {
    pub color: ColorToken,
    pub dash: bool,
    pub x: String,
    pub y: String,
    pub t: (f64, f64),
}

/// A labelled point.
#[derive(Debug, Clone, PartialEq)]
pub struct PointSeries {
    pub color: ColorToken,
    pub x: f64,
    pub y: f64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Series {
    Fn(FnSeries),
    Param(ParamSeries),
    Point(PointSeries),
}

/// The visible part of the maths plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub xmin: f64,
    pub xmax: f64,
    pub ymin: f64,
    pub ymax: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub view: View,
    pub grid: bool,
    pub axes: bool,
    /// Trigonometric functions read degrees instead of radians.
    pub degrees: bool,
    /// Ticks on x at multiples of π.
    pub pi_ticks: bool,
    /// Values of the letters the formulas use besides the variable.
    pub params: Params,
    pub series: Vec<Series>,
}

fn is_param_name(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let c = chars.next().filter(|_| chars.next().is_none())?;
    (c.is_ascii_alphabetic() || ('α'..='ω').contains(&c) || ('Α'..='Ω').contains(&c)).then_some(c)
}

fn finite_pair(raw: Option<&Value>) -> Option<(f64, f64)> {
    let list = raw?.as_array().filter(|a| a.len() == 2)?;
    Some((finite(list.first())?, finite(list.get(1))?))
}

fn decode_series(raw: &Value) -> Option<Series> {
    let d = raw.as_object()?;
    let color = ColorToken::parse(d.get("color")?.as_str()?)?;
    let dash = d.get("dash") == Some(&Value::Bool(true));
    let text = |key: &str| {
        d.get(key)
            .and_then(Value::as_str)
            .filter(|s| utf16_len(s) <= MAX_EXPRESSION_LENGTH)
    };
    match d.get("kind")?.as_str()? {
        "fn" => {
            let expr = text("expr")?.to_owned();
            let shade = finite_pair(d.get("shade"))
                .filter(|(a, b)| a != b)
                .map(|(a, b)| (a.min(b), a.max(b)));
            let marks = match d.get("marks").and_then(Value::as_array) {
                Some(list) => MARKS
                    .iter()
                    .filter(|(name, _)| list.iter().any(|m| m.as_str() == Some(name)))
                    .map(|(_, mark)| *mark)
                    .collect(),
                None => Vec::new(),
            };
            Some(Series::Fn(FnSeries {
                color,
                dash,
                expr,
                shade,
                marks,
            }))
        }
        "param" => {
            let (x, y) = (text("x")?.to_owned(), text("y")?.to_owned());
            let t = finite_pair(d.get("t")).filter(|(a, b)| a != b)?;
            Some(Series::Param(ParamSeries {
                color,
                dash,
                x,
                y,
                t,
            }))
        }
        "point" => {
            let (x, y) = (finite(d.get("x"))?, finite(d.get("y"))?);
            let label = d
                .get("label")
                .and_then(Value::as_str)
                .map(|l| utf16_prefix(l, MAX_LABEL))
                .unwrap_or_default();
            Some(Series::Point(PointSeries {
                color,
                x: clamp(x, -MAX_VALUE, MAX_VALUE),
                y: clamp(y, -MAX_VALUE, MAX_VALUE),
                label,
            }))
        }
        _ => None,
    }
}

fn decode_view(raw: Option<&Value>) -> Option<View> {
    let list = raw?.as_array().filter(|a| a.len() == 4)?;
    let mut v = [0.0; 4];
    for (slot, n) in v.iter_mut().zip(list) {
        *slot = clamp(finite(Some(n))?, -MAX_VALUE, MAX_VALUE);
    }
    let [xmin, xmax, ymin, ymax] = v;
    (xmax - xmin >= MIN_SPAN && ymax - ymin >= MIN_SPAN).then_some(View {
        xmin,
        xmax,
        ymin,
        ymax,
    })
}

impl Graph {
    /// Validates a graph received from the server or another client: a malformed series is
    /// dropped, a malformed graph is refused.
    pub fn decode(d: &Map<String, Value>) -> Option<Self> {
        let [x1, y1, x2, y2] = corners(d)?;
        let view = decode_view(d.get("view"))?;
        let list = d.get("series")?.as_array()?;
        let mut params = Params::new();
        if let Some(raw) = d.get("params").and_then(Value::as_object) {
            for (name, value) in raw {
                if params.len() >= MAX_PARAMS {
                    break;
                }
                if let (Some(c), Some(v)) = (is_param_name(name), finite(Some(value))) {
                    params.insert(c, clamp(v, -MAX_VALUE, MAX_VALUE));
                }
            }
        }
        let series = list
            .iter()
            .take(MAX_SERIES)
            .filter_map(decode_series)
            .collect();
        let flag = |key: &str, wanted: bool| d.get(key) == Some(&Value::Bool(wanted));
        Some(Self {
            x1,
            y1,
            x2,
            y2,
            view,
            grid: !flag("grid", false),
            axes: !flag("axes", false),
            degrees: flag("degrees", true),
            pi_ticks: flag("piTicks", true),
            params,
            series,
        })
    }

    /// The graph's box, normalized and kept between the size limits (`graphBox`).
    pub fn rect(&self) -> BoxRect {
        box_of(self.x1, self.y1, self.x2, self.y2, MIN_BOX)
    }

    pub fn plot_space(&self) -> PlotSpace {
        let BoxRect { x, y, w, h } = self.rect();
        let View {
            xmin,
            xmax,
            ymin,
            ymax,
        } = self.view;
        PlotSpace {
            x,
            y,
            w,
            h,
            xmin,
            ymin,
            sx: w / (xmax - xmin),
            sy: h / (ymax - ymin),
        }
    }
}

/// Maps between the maths plane and board units for one graph.
#[derive(Debug, Clone, Copy)]
pub struct PlotSpace {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    xmin: f64,
    ymin: f64,
    sx: f64,
    sy: f64,
}

impl PlotSpace {
    /// Board x of a maths x.
    pub fn px(&self, v: f64) -> f64 {
        self.x + (v - self.xmin) * self.sx
    }

    /// Board y of a maths y.
    pub fn py(&self, v: f64) -> f64 {
        self.y + self.h - (v - self.ymin) * self.sy
    }
}
