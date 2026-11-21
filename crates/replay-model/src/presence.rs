//! What a board shows of the other people on it: a pointer and the stroke being drawn (ephemeral,
//! never stored), put back together from the updates received. Port of `board/presence.ts`.

use serde_json::{Map, Value};

use crate::element::{BoardElement, ElementKind, Point, decode_element, patch_element};

/// A participant who sent nothing for this long is no longer shown.
pub const PRESENCE_TTL_MS: f64 = 6_000.0;

#[derive(Debug, Clone, PartialEq)]
pub struct RemotePresence {
    pub cursor: Option<Point>,
    /// The element they are drawing right now.
    pub preview: Option<BoardElement>,
    /// Which of their strokes `preview` is (its updates carry it).
    pub stroke: Option<f64>,
    /// The element `preview` is an edit of: hidden from the board while it is shown.
    pub target: Option<String>,
    /// When the last update arrived (ms into the recording).
    pub at: f64,
}

/// A pointer and what is being drawn, as sent. A stroke's `preview` holds the points from index
/// `from` on (earlier ones were sent before) and the stroke's number `sid`; any other element is
/// sent whole.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PresencePayload {
    pub cursor: Option<Value>,
    pub preview: Option<Map<String, Value>>,
}

impl PresencePayload {
    pub(crate) fn from_value(p: &Value) -> Self {
        let Some(p) = p.as_object() else {
            return Self::default();
        };
        Self {
            cursor: p.get("cursor").cloned(),
            preview: p.get("preview").and_then(Value::as_object).cloned(),
        }
    }
}

/// JavaScript's `Number(v)` for what a cursor coordinate may be.
fn js_number(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse().unwrap_or(f64::NAN)
            }
        }
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Null => 0.0,
        _ => f64::NAN,
    }
}

/// Applies a pointer update to what is shown of that person (none: they left the board). A
/// stroke's points are appended to the ones received; one received from its middle (an update was
/// lost, or this viewer just came) continues what is shown, and the finished element replaces it
/// all.
pub fn apply_presence(
    prev: Option<&RemotePresence>,
    payload: &PresencePayload,
    at: f64,
    element: &dyn Fn(&str) -> Option<BoardElement>,
) -> Option<RemotePresence> {
    let cursor = match &payload.cursor {
        Some(Value::Array(c)) if c.len() == 2 => Some(Point {
            x: js_number(&c[0]),
            y: js_number(&c[1]),
        }),
        _ => None,
    };
    let raw = payload.preview.as_ref();
    if cursor.is_none() && raw.is_none() {
        return None;
    }
    let stroke = raw.and_then(|r| r.get("sid")).and_then(Value::as_f64);
    let target = raw
        .and_then(|r| r.get("ref"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(RemotePresence {
        cursor,
        preview: merge_preview(prev, raw, element),
        stroke,
        target,
        at,
    })
}

/// `prev.stroke === raw.sid` where either may be `undefined`.
fn same_stroke(prev: Option<&RemotePresence>, raw: &Map<String, Value>) -> bool {
    let prev = prev.and_then(|p| p.stroke);
    match raw.get("sid") {
        None => prev.is_none(),
        Some(Value::Number(n)) => prev.is_some() && prev == n.as_f64(),
        Some(_) => false,
    }
}

fn merge_preview(
    prev: Option<&RemotePresence>,
    raw: Option<&Map<String, Value>>,
    element: &dyn Fn(&str) -> Option<BoardElement>,
) -> Option<BoardElement> {
    let raw = raw?;
    if let Some(id) = raw.get("ref").and_then(Value::as_str) {
        let base = element(id)?;
        return patch_element(&base, raw.get("set"));
    }
    let from = match raw.get("from").and_then(Value::as_f64) {
        Some(f) if f > 0.0 => f.floor() as usize,
        _ => 0,
    };
    let shown = prev.and_then(|p| p.preview.as_ref());
    if let Some(shown) = shown.filter(|_| from > 0 && same_stroke(prev, raw))
        && let ElementKind::Stroke {
            highlighter,
            points: shown_points,
        } = &shown.kind
    {
        {
            let more = raw
                .get("points")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if more.is_empty() {
                return Some(shown.clone());
            }
            let kept = shown_points.len().min(from.saturating_mul(3));
            let mut points: Vec<Value> = shown_points[..kept]
                .iter()
                .map(|p| Value::from(*p))
                .collect();
            points.extend(more);
            // An update without the stroke's look draws it as it was.
            let mut merged = Map::new();
            merged.insert("type".into(), "stroke".into());
            merged.insert("color".into(), shown.color.as_str().into());
            merged.insert("size".into(), shown.size.into());
            merged.insert("z".into(), shown.z.into());
            merged.insert("highlighter".into(), (*highlighter).into());
            merged.extend(raw.iter().map(|(k, v)| (k.clone(), v.clone())));
            merged.insert("points".into(), Value::Array(points));
            return ink_only(decode_element(&merged)).or_else(|| Some(shown.clone()));
        }
    }
    ink_only(decode_element(raw))
}

/// A pointer's preview is ink in the making: a sheet or the view is never shown that way.
fn ink_only(el: Option<BoardElement>) -> Option<BoardElement> {
    el.filter(|e| !matches!(e.kind, ElementKind::Page(_) | ElementKind::View { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(v: Value) -> PresencePayload {
        PresencePayload::from_value(&v)
    }

    fn none(_: &str) -> Option<BoardElement> {
        None
    }

    fn stroke(sid: u32, from: u32, points: Value) -> Value {
        json!({"preview":{"type":"stroke","color":"red","size":4,"z":1,"sid":sid,"from":from,"points":points}})
    }

    fn points_of(p: &RemotePresence) -> Vec<f64> {
        match &p.preview.as_ref().unwrap().kind {
            ElementKind::Stroke { points, .. } => points.clone(),
            _ => panic!("stroke"),
        }
    }

    #[test]
    fn a_pointer_alone_is_kept_and_nothing_at_all_removes_the_person() {
        let p = apply_presence(None, &payload(json!({"cursor":[3,4]})), 100.0, &none).unwrap();
        assert_eq!(p.cursor, Some(Point { x: 3.0, y: 4.0 }));
        assert!(p.preview.is_none() && p.at == 100.0);
        assert!(apply_presence(Some(&p), &payload(json!({})), 200.0, &none).is_none());
        assert!(apply_presence(Some(&p), &payload(json!({"cursor":[1]})), 200.0, &none).is_none());
    }

    #[test]
    fn cursor_coordinates_are_read_as_javascript_numbers() {
        let p = apply_presence(None, &payload(json!({"cursor":["7", null]})), 0.0, &none).unwrap();
        assert_eq!(p.cursor, Some(Point { x: 7.0, y: 0.0 }));
    }

    #[test]
    fn a_stroke_grows_by_appending_the_points_from_its_index() {
        let first = apply_presence(
            None,
            &payload(stroke(1, 0, json!([0, 0, 0.5, 1, 1, 0.5]))),
            10.0,
            &none,
        )
        .unwrap();
        assert_eq!(first.stroke, Some(1.0));
        assert_eq!(points_of(&first).len(), 6);
        let next = apply_presence(
            Some(&first),
            &payload(stroke(1, 2, json!([2, 2, 0.5]))),
            20.0,
            &none,
        )
        .unwrap();
        assert_eq!(
            points_of(&next),
            [0.0, 0.0, 0.5, 1.0, 1.0, 0.5, 2.0, 2.0, 0.5]
        );
        // An update that restates points already shown replaces them from `from` on.
        let again = apply_presence(
            Some(&next),
            &payload(stroke(1, 1, json!([9, 9, 0.5]))),
            30.0,
            &none,
        )
        .unwrap();
        assert_eq!(points_of(&again), [0.0, 0.0, 0.5, 9.0, 9.0, 0.5]);
    }

    #[test]
    fn an_update_from_the_middle_of_another_stroke_starts_a_new_preview() {
        let first = apply_presence(
            None,
            &payload(stroke(1, 0, json!([0, 0, 0.5]))),
            10.0,
            &none,
        )
        .unwrap();
        // Another stroke number: `from` is not honoured, the preview is what was sent.
        let other = apply_presence(
            Some(&first),
            &payload(stroke(2, 5, json!([7, 7, 0.5]))),
            20.0,
            &none,
        )
        .unwrap();
        assert_eq!(points_of(&other), [7.0, 7.0, 0.5]);
    }

    #[test]
    fn an_update_without_points_keeps_what_is_shown_and_one_without_a_look_reuses_it() {
        let first = apply_presence(
            None,
            &payload(stroke(1, 0, json!([0, 0, 0.5]))),
            10.0,
            &none,
        )
        .unwrap();
        let empty =
            apply_presence(Some(&first), &payload(stroke(1, 1, json!([]))), 20.0, &none).unwrap();
        assert_eq!(empty.preview, first.preview);
        let bare = json!({"preview":{"sid":1,"from":1,"points":[1,1,0.5]}});
        let grown = apply_presence(Some(&first), &payload(bare), 30.0, &none).unwrap();
        let el = grown.preview.unwrap();
        assert_eq!(
            (el.color, el.size, el.z),
            (crate::ColorToken::Red, 4.0, 1.0)
        );
        assert_eq!(
            points_of(&RemotePresence {
                preview: Some(el),
                ..first.clone()
            })
            .len(),
            6
        );
    }

    #[test]
    fn a_missing_sid_matches_a_missing_sid() {
        let no_sid = |from: u32, pts: Value| json!({"preview":{"type":"stroke","color":"red","size":4,"z":1,"from":from,"points":pts}});
        let first =
            apply_presence(None, &payload(no_sid(0, json!([0, 0, 0.5]))), 1.0, &none).unwrap();
        assert_eq!(first.stroke, None);
        let next = apply_presence(
            Some(&first),
            &payload(no_sid(1, json!([1, 1, 0.5]))),
            2.0,
            &none,
        )
        .unwrap();
        assert_eq!(points_of(&next).len(), 6);
    }

    #[test]
    fn a_sheet_or_the_view_is_never_shown_as_a_preview() {
        let page = json!({"preview":{"type":"page","color":"ink","size":2,"z":0,"x":0,"y":0,"w":1280,"h":720}});
        assert!(
            apply_presence(None, &payload(page), 1.0, &none)
                .unwrap()
                .preview
                .is_none()
        );
        let view = json!({"cursor":[1,1],"preview":{"type":"view","color":"ink","size":1,"z":0,"page":"a"}});
        assert!(
            apply_presence(None, &payload(view), 1.0, &none)
                .unwrap()
                .preview
                .is_none()
        );
    }

    #[test]
    fn a_ref_edits_the_element_it_names_and_is_hidden_from_the_board_by_target() {
        let graph = decode_element(
            json!({"type":"graph","color":"blue","size":2,"z":1,"params":{"a":1}})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let lookup = |id: &str| (id == "g1").then(|| graph.clone());
        let upd = json!({"preview":{"ref":"g1","set":{"params":{"a":5}}}});
        let p = apply_presence(None, &payload(upd), 1.0, &lookup).unwrap();
        assert_eq!(p.target.as_deref(), Some("g1"));
        let ElementKind::Pack(pack) = &p.preview.unwrap().kind else {
            panic!("pack")
        };
        assert_eq!(pack.data["params"], json!({"a":5}));
        // An element that is not on the board gives no preview, but the person is still there.
        let gone = apply_presence(
            None,
            &payload(json!({"preview":{"ref":"zz","set":{}}})),
            1.0,
            &lookup,
        )
        .unwrap();
        assert!(gone.preview.is_none());
    }
}
