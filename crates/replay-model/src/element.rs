//! Whiteboard elements and `decodeElement`: the only way in, which drops anything malformed rather
//! than trusting what another client sent. Port of `board/elements.ts` (the parts a replay needs:
//! there is no pointer, so no hit testing, translating or compacting).

use serde_json::{Map, Value};

/// The most pages a deck has (mirrors the server's limit).
pub const MAX_DECK_PAGES: u32 = 120;
/// The size a sheet may have, in board units, on either side.
const SHEET_SIZE: (f64, f64) = (100.0, 8000.0);
/// The element id the teacher's view is always written under.
pub const VIEW_ID: &str = "view";
/// Line height relative to the font size.
pub const LINE_HEIGHT: f64 = 1.25;
/// The longest text kept (in UTF-16 code units, as the web player counts).
pub const MAX_TEXT_LENGTH: usize = 2000;

/// A palette token, resolved to a colour per theme when drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorToken {
    Ink,
    Red,
    Orange,
    Green,
    Blue,
    Violet,
}

impl ColorToken {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ink" => Self::Ink,
            "red" => Self::Red,
            "orange" => Self::Orange,
            "green" => Self::Green,
            "blue" => Self::Blue,
            "violet" => Self::Violet,
            _ => return None,
        })
    }

    /// The token as written in the log.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ink => "ink",
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Violet => "violet",
        }
    }
}

/// A point in board units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// An axis-aligned box in board units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Rect {
    pub fn intersects(&self, other: &Rect) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }
}

/// Measures one line of text at a font size (canvas `measureText`).
pub trait MeasureText {
    fn width(&self, line: &str, font_size: f64) -> f64;
}

impl<F: Fn(&str, f64) -> f64> MeasureText for F {
    fn width(&self, line: &str, font_size: f64) -> f64 {
        self(line, font_size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Line,
    Arrow,
    Rect,
    Ellipse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackKind {
    Graph,
    Solid,
    Symbol,
}

impl PackKind {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "graph" => Self::Graph,
            "solid" => Self::Solid,
            "symbol" => Self::Symbol,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Solid => "solid",
            Self::Symbol => "symbol",
        }
    }
}

/// An element of a feature pack (graph, solid, symbol). Kept as the JSON it arrived as, with the
/// common fields validated: the maths and physics slices decode and draw them.
#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub kind: PackKind,
    pub data: Map<String, Value>,
}

/// The picture of a page of a lesson deck.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeckPage {
    pub deck: String,
    /// 1-based.
    pub n: u32,
}

/// A sheet of the board: a frame with a place and a size, blank or showing a deck page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub picture: Option<DeckPage>,
}

impl Page {
    pub fn rect(&self) -> Rect {
        Rect {
            min_x: self.x,
            min_y: self.y,
            max_x: self.x + self.w,
            max_y: self.y + self.h,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElementKind {
    Stroke {
        highlighter: bool,
        /// Flat `[x, y, pressure, x, y, pressure, ...]`.
        points: Vec<f64>,
    },
    Shape {
        shape: ShapeKind,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    Text {
        x: f64,
        /// Top of the first line.
        y: f64,
        text: String,
    },
    Page(Page),
    /// Which sheet the teacher is on: a control, never drawn or measured.
    View {
        page: String,
    },
    Pack(Pack),
}

/// A validated board element.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardElement {
    pub color: ColorToken,
    /// Stroke width, or font size for text (at most 200).
    pub size: f64,
    /// Stacking order: creation time; ties break on the element id.
    pub z: f64,
    pub kind: ElementKind,
}

impl BoardElement {
    /// The element's `type` as written in the log.
    pub fn type_name(&self) -> &'static str {
        match &self.kind {
            ElementKind::Stroke { .. } => "stroke",
            ElementKind::Shape { shape, .. } => match shape {
                ShapeKind::Line => "line",
                ShapeKind::Arrow => "arrow",
                ShapeKind::Rect => "rect",
                ShapeKind::Ellipse => "ellipse",
            },
            ElementKind::Text { .. } => "text",
            ElementKind::Page(_) => "page",
            ElementKind::View { .. } => "view",
            ElementKind::Pack(p) => p.kind.as_str(),
        }
    }
}

fn finite(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn is_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// The first `MAX_TEXT_LENGTH` UTF-16 code units of `text`. The web player may cut a surrogate
/// pair in two; the half that is left draws as a replacement character, so it is dropped here.
fn clip_text(text: &str) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > MAX_TEXT_LENGTH {
            break;
        }
        out.push(c);
    }
    out
}

/// Validates element data received from the server or another client.
pub fn decode_element(data: &Map<String, Value>) -> Option<BoardElement> {
    let color = ColorToken::parse(data.get("color")?.as_str()?)?;
    let size = finite(data.get("size")).filter(|s| *s > 0.0)?;
    let z = finite(data.get("z"))?;
    let size = size.min(200.0);
    let ty = data.get("type")?.as_str()?;
    let make = |kind| {
        Some(BoardElement {
            color,
            size,
            z,
            kind,
        })
    };
    match ty {
        "stroke" => {
            let points = data.get("points")?.as_array()?;
            if points.len() < 3 || points.len() % 3 != 0 {
                return None;
            }
            let points = points
                .iter()
                .map(|p| finite(Some(p)))
                .collect::<Option<Vec<f64>>>()?;
            make(ElementKind::Stroke {
                highlighter: data.get("highlighter") == Some(&Value::Bool(true)),
                points,
            })
        }
        "line" | "arrow" | "rect" | "ellipse" => {
            let shape = match ty {
                "line" => ShapeKind::Line,
                "arrow" => ShapeKind::Arrow,
                "rect" => ShapeKind::Rect,
                _ => ShapeKind::Ellipse,
            };
            make(ElementKind::Shape {
                shape,
                x1: finite(data.get("x1"))?,
                y1: finite(data.get("y1"))?,
                x2: finite(data.get("x2"))?,
                y2: finite(data.get("y2"))?,
            })
        }
        "text" => {
            let text = data.get("text")?.as_str().filter(|t| !t.is_empty())?;
            make(ElementKind::Text {
                x: finite(data.get("x"))?,
                y: finite(data.get("y"))?,
                text: clip_text(text),
            })
        }
        "page" => decode_page(data).and_then(|page| make(ElementKind::Page(page))),
        "view" => {
            let page = data.get("page")?.as_str().filter(|p| is_id(p))?;
            make(ElementKind::View {
                page: page.to_owned(),
            })
        }
        _ => {
            let kind = PackKind::parse(ty)?;
            let mut data = data.clone();
            data.insert("size".into(), Value::from(size));
            make(ElementKind::Pack(Pack { kind, data }))
        }
    }
}

/// A sheet from data of another client: finite place, sane size, a deck page named whole or not at all.
fn decode_page(d: &Map<String, Value>) -> Option<Page> {
    let (x, y) = (finite(d.get("x"))?, finite(d.get("y"))?);
    let (w, h) = (finite(d.get("w"))?, finite(d.get("h"))?);
    let in_range = |v: f64| (SHEET_SIZE.0..=SHEET_SIZE.1).contains(&v);
    if !in_range(w) || !in_range(h) {
        return None;
    }
    let sheet = |picture| {
        Some(Page {
            x,
            y,
            w,
            h,
            picture,
        })
    };
    // `undefined` in the web player: absent (JSON has no other way to say it).
    if !d.contains_key("deck") && !d.contains_key("n") {
        return sheet(None);
    }
    let deck = d.get("deck")?.as_str()?;
    if deck.len() != 26
        || !deck
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return None;
    }
    let n = d.get("n")?.as_f64()?;
    if n.fract() != 0.0 || n < 1.0 || n > f64::from(MAX_DECK_PAGES) {
        return None;
    }
    sheet(Some(DeckPage {
        deck: deck.to_owned(),
        n: n as u32,
    }))
}

/// A graph, solid or symbol with some fields replaced: how a drag is shown to the others before it
/// is written. `params` of a graph is merged, so a slider sends only the letter it moves.
pub fn patch_element(base: &BoardElement, set: Option<&Value>) -> Option<BoardElement> {
    let ElementKind::Pack(pack) = &base.kind else {
        return None;
    };
    let changes = set?.as_object()?;
    let mut data = pack.data.clone();
    let base_params = data.get("params").and_then(Value::as_object).cloned();
    data.extend(changes.iter().map(|(k, v)| (k.clone(), v.clone())));
    data.insert("type".into(), pack.kind.as_str().into());
    if pack.kind == PackKind::Graph {
        let merged = match changes.get("params").and_then(Value::as_object) {
            Some(changed) => {
                let mut params = base_params.unwrap_or_default();
                params.extend(changed.iter().map(|(k, v)| (k.clone(), v.clone())));
                Some(params)
            }
            None => base_params,
        };
        if let Some(params) = merged {
            data.insert("params".into(), Value::Object(params));
        }
    }
    decode_element(&data)
}

/// How far ink reaches from an element's geometry.
fn ink_reach(el: &BoardElement) -> f64 {
    match &el.kind {
        ElementKind::Text { .. } | ElementKind::Page(_) | ElementKind::View { .. } => 0.0,
        ElementKind::Pack(_) => el.size / 2.0,
        ElementKind::Stroke { highlighter, .. } => stroke_radius(el.size, *highlighter),
        ElementKind::Shape { shape, .. } => {
            el.size / 2.0
                + if *shape == ShapeKind::Arrow {
                    (el.size * 3.5).max(12.0)
                } else {
                    0.0
                }
        }
    }
}

/// Half the drawn width of a freehand stroke (pens taper, highlighters are wide).
pub fn stroke_radius(size: f64, highlighter: bool) -> f64 {
    size * if highlighter { 1.5 } else { 0.8 }
}

/// The box an element covers (board units), including the stroke width.
pub fn element_bounds(el: &BoardElement, measure: &dyn MeasureText) -> Rect {
    let pad = ink_reach(el);
    let padded = |min_x: f64, min_y: f64, max_x: f64, max_y: f64| Rect {
        min_x: min_x - pad,
        min_y: min_y - pad,
        max_x: max_x + pad,
        max_y: max_y + pad,
    };
    match &el.kind {
        ElementKind::Stroke { points, .. } => {
            let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
            let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            for p in points.as_chunks::<3>().0 {
                min_x = min_x.min(p[0]);
                max_x = max_x.max(p[0]);
                min_y = min_y.min(p[1]);
                max_y = max_y.max(p[1]);
            }
            padded(min_x, min_y, max_x, max_y)
        }
        ElementKind::Text { x, y, text } => {
            let lines: Vec<&str> = text.split('\n').collect();
            let width = lines
                .iter()
                .map(|l| measure.width(l, el.size))
                .fold(f64::NEG_INFINITY, f64::max);
            Rect {
                min_x: *x,
                min_y: *y,
                max_x: x + width,
                max_y: y + lines.len() as f64 * el.size * LINE_HEIGHT,
            }
        }
        ElementKind::Page(page) => page.rect(),
        ElementKind::View { .. } => Rect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 0.0,
            max_y: 0.0,
        },
        // Until the pack slices land: the box the element is dragged out as.
        ElementKind::Pack(pack) => {
            let n = |k: &str| finite(pack.data.get(k)).unwrap_or(0.0);
            padded(
                n("x1").min(n("x2")),
                n("y1").min(n("y2")),
                n("x1").max(n("x2")),
                n("y1").max(n("y2")),
            )
        }
        ElementKind::Shape { x1, y1, x2, y2, .. } => {
            padded(x1.min(*x2), y1.min(*y2), x1.max(*x2), y1.max(*y2))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(v: Value) -> Option<BoardElement> {
        decode_element(v.as_object().unwrap())
    }

    fn width_of_chars(line: &str, size: f64) -> f64 {
        line.chars().count() as f64 * size / 2.0
    }

    #[test]
    fn common_fields_are_checked_and_size_is_clamped() {
        let ok = json!({"type":"line","color":"red","size":500,"z":3,"x1":0,"y1":0,"x2":1,"y2":1});
        assert_eq!(decode(ok).unwrap().size, 200.0);
        for bad in [
            json!({"type":"line","color":"pink","size":2,"z":1,"x1":0,"y1":0,"x2":1,"y2":1}),
            json!({"type":"line","color":"red","size":0,"z":1,"x1":0,"y1":0,"x2":1,"y2":1}),
            json!({"type":"line","color":"red","size":-1,"z":1,"x1":0,"y1":0,"x2":1,"y2":1}),
            json!({"type":"line","color":"red","size":"2","z":1,"x1":0,"y1":0,"x2":1,"y2":1}),
            json!({"type":"line","color":"red","size":2,"x1":0,"y1":0,"x2":1,"y2":1}),
            json!({"type":"line","color":"red","size":2,"z":1,"x1":0,"y1":0,"x2":1}),
            json!({"type":"blob","color":"red","size":2,"z":1}),
        ] {
            assert!(decode(bad).is_none());
        }
    }

    #[test]
    fn strokes_need_whole_triples_of_numbers() {
        let stroke = |points: Value| {
            decode(json!({"type":"stroke","color":"ink","size":4,"z":1,"points":points}))
        };
        assert!(stroke(json!([1, 2, 0.5])).is_some());
        assert!(stroke(json!([1, 2])).is_none());
        assert!(stroke(json!([1, 2, 0.5, 3])).is_none());
        assert!(stroke(json!([1, 2, "x"])).is_none());
        let el = decode(
            json!({"type":"stroke","color":"ink","size":4,"z":1,"points":[1,2,0.5],"highlighter":"yes"}),
        )
        .unwrap();
        // Only a real `true` makes a highlighter.
        assert!(matches!(
            el.kind,
            ElementKind::Stroke {
                highlighter: false,
                ..
            }
        ));
    }

    #[test]
    fn text_must_be_non_empty_and_is_cut_at_2000_utf16_units() {
        let text = |t: &str| {
            decode(json!({"type":"text","color":"ink","size":20,"z":1,"x":0,"y":0,"text":t}))
        };
        assert!(text("").is_none());
        let long = "a".repeat(2500);
        let Some(BoardElement {
            kind: ElementKind::Text { text: kept, .. },
            ..
        }) = text(&long)
        else {
            panic!("text")
        };
        assert_eq!(kept.len(), 2000);
        // An astral character counts as two units, and a pair that would be cut is dropped.
        let astral = format!("{}{}", "a".repeat(1999), "\u{1F600}");
        let Some(BoardElement {
            kind: ElementKind::Text { text: kept, .. },
            ..
        }) = text(&astral)
        else {
            panic!("text")
        };
        assert_eq!(kept, "a".repeat(1999));
    }

    #[test]
    fn sheets_have_a_sane_size_and_a_whole_deck_page() {
        let page = |extra: Value| {
            let mut d =
                json!({"type":"page","color":"ink","size":2,"z":0,"x":0,"y":0,"w":1280,"h":720});
            d.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            decode(d)
        };
        assert!(page(json!({})).is_some());
        assert!(page(json!({"w":99})).is_none());
        assert!(page(json!({"h":8001})).is_none());
        let deck = "a".repeat(26);
        assert!(page(json!({"deck":deck,"n":3})).is_some());
        assert!(page(json!({"deck":deck})).is_none());
        assert!(page(json!({"n":3})).is_none());
        assert!(page(json!({"deck":"short","n":1})).is_none());
        assert!(page(json!({"deck":deck,"n":0})).is_none());
        assert!(page(json!({"deck":deck,"n":121})).is_none());
        assert!(page(json!({"deck":deck,"n":1.5})).is_none());
    }

    #[test]
    fn the_view_names_a_sheet_by_a_plain_id() {
        let view =
            |page: Value| decode(json!({"type":"view","color":"ink","size":1,"z":0,"page":page}));
        assert!(view(json!("abc123")).is_some());
        assert!(view(json!("ABC")).is_none());
        assert!(view(json!("")).is_none());
        assert!(view(json!("a".repeat(65))).is_none());
        assert!(view(json!(7)).is_none());
    }

    #[test]
    fn pack_elements_decode_as_opaque_data() {
        let el = decode(json!({"type":"graph","color":"blue","size":300,"z":1,"params":{"a":1}}))
            .unwrap();
        assert_eq!(el.size, 200.0);
        let ElementKind::Pack(pack) = &el.kind else {
            panic!("pack")
        };
        assert_eq!(pack.kind, PackKind::Graph);
        assert_eq!(pack.data["size"], json!(200.0));
    }

    #[test]
    fn bounds_follow_the_web_players_reach_rules() {
        let m = width_of_chars;
        let line = decode(
            json!({"type":"line","color":"ink","size":4,"z":1,"x1":10,"y1":20,"x2":0,"y2":5}),
        )
        .unwrap();
        let b = element_bounds(&line, &m);
        assert_eq!(
            (b.min_x, b.min_y, b.max_x, b.max_y),
            (-2.0, 3.0, 12.0, 22.0)
        );
        // An arrow's head reaches size * 3.5, but at least 12.
        let arrow = decode(
            json!({"type":"arrow","color":"ink","size":2,"z":1,"x1":0,"y1":0,"x2":10,"y2":0}),
        )
        .unwrap();
        assert_eq!(element_bounds(&arrow, &m).min_x, -13.0);
        let stroke = decode(
            json!({"type":"stroke","color":"ink","size":10,"z":1,"points":[0,0,0.5,10,4,0.5]}),
        )
        .unwrap();
        let b = element_bounds(&stroke, &m);
        assert_eq!((b.min_x, b.max_x, b.max_y), (-8.0, 18.0, 12.0));
        let hl = decode(json!({"type":"stroke","color":"ink","size":10,"highlighter":true,"z":1,"points":[0,0,0.5]}))
            .unwrap();
        assert_eq!(element_bounds(&hl, &m).max_x, 15.0);
    }

    #[test]
    fn text_bounds_use_the_widest_line_and_the_line_height() {
        let el = decode(
            json!({"type":"text","color":"ink","size":20,"z":1,"x":5,"y":6,"text":"ab\nabcd"}),
        )
        .unwrap();
        let b = element_bounds(&el, &width_of_chars);
        assert_eq!((b.min_x, b.min_y, b.max_x, b.max_y), (5.0, 6.0, 45.0, 56.0));
    }

    #[test]
    fn patching_merges_graph_params_and_ignores_other_elements() {
        let graph = decode(json!({"type":"graph","color":"blue","size":2,"z":1,"x1":0,"y1":0,"x2":4,"y2":4,"params":{"a":1,"b":2}}))
            .unwrap();
        let patched = patch_element(&graph, Some(&json!({"params":{"b":9},"x2":8}))).unwrap();
        let ElementKind::Pack(pack) = &patched.kind else {
            panic!("pack")
        };
        assert_eq!(pack.data["params"], json!({"a":1,"b":9}));
        assert_eq!(pack.data["x2"], json!(8));
        assert!(patch_element(&graph, None).is_none());
        let line =
            decode(json!({"type":"line","color":"ink","size":2,"z":1,"x1":0,"y1":0,"x2":1,"y2":1}))
                .unwrap();
        assert!(patch_element(&line, Some(&json!({"x2":3}))).is_none());
    }
}
