//! The board's state: every element's latest envelope, merged with the server's last-writer-wins
//! rule (higher version, then lower nonce), so a replay converges on what the database held
//! whatever order writes arrived in. Deleted elements stay as tombstones: an older write that
//! arrives late must not bring them back. Port of `board/scene.svelte.ts` (without the local edit
//! API, undo history and lazy pack loading: a replay only reads).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::element::{
    BoardElement, ElementKind, MeasureText, Rect, VIEW_ID, decode_element, element_bounds,
};

/// A board element as the server stores and sends it (its `data` is the element's own JSON).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Envelope {
    pub id: String,
    pub version: i64,
    pub version_nonce: i64,
    pub is_deleted: bool,
    pub data: Map<String, Value>,
}

/// Whether `next` replaces `current` (the server's rule, exactly).
pub fn wins(next: &Envelope, current: &Envelope) -> bool {
    if next.version != current.version {
        return next.version > current.version;
    }
    next.version_nonce < current.version_nonce
}

/// A live element and its id, as the board is drawn from.
#[derive(Debug, Clone)]
pub struct OrderedElement {
    pub id: String,
    pub el: Arc<BoardElement>,
}

struct Entry {
    env: Envelope,
    el: Option<Arc<BoardElement>>,
}

#[derive(Default)]
pub struct BoardScene {
    /// Bumps on every visible change.
    revision: u64,
    entries: HashMap<String, Entry>,
    ordered: RefCell<Option<Arc<[OrderedElement]>>>,
}

impl std::fmt::Debug for BoardScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardScene")
            .field("revision", &self.revision)
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl BoardScene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The live element (none when deleted, unknown or malformed).
    pub fn element(&self, id: &str) -> Option<&BoardElement> {
        self.entries.get(id)?.el.as_deref()
    }

    /// The sheet the teacher is on, once they have moved: the id the view names.
    pub fn view(&self) -> Option<&str> {
        match &self.element(VIEW_ID)?.kind {
            ElementKind::View { page } => Some(page),
            _ => None,
        }
    }

    /// Merges writes from the server; true if anything changed.
    pub fn merge(&mut self, envelopes: impl IntoIterator<Item = Envelope>) -> bool {
        let mut changed = false;
        for env in envelopes {
            if self
                .entries
                .get(&env.id)
                .is_some_and(|cur| !wins(&env, &cur.env))
            {
                continue;
            }
            let el = if env.is_deleted {
                None
            } else {
                decode_element(&env.data).map(Arc::new)
            };
            self.entries.insert(env.id.clone(), Entry { env, el });
            changed = true;
        }
        if changed {
            self.touch();
        }
        changed
    }

    /// Forgets everything (the teacher cleared the board).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.touch();
    }

    /// Live elements, bottom to top: by `z`, ties by id. The view is a control, not an element of
    /// the board.
    pub fn ordered(&self) -> Arc<[OrderedElement]> {
        self.ordered
            .borrow_mut()
            .get_or_insert_with(|| {
                let mut list: Vec<OrderedElement> = self
                    .entries
                    .iter()
                    .filter_map(|(id, e)| {
                        let el = e.el.as_ref()?;
                        (!matches!(el.kind, ElementKind::View { .. })).then(|| OrderedElement {
                            id: id.clone(),
                            el: Arc::clone(el),
                        })
                    })
                    .collect();
                list.sort_by(|a, b| {
                    // `a.z - b.z || compare ids`, the web player's compareZ.
                    (a.el.z - b.el.z)
                        .partial_cmp(&0.0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.id.cmp(&b.id))
                });
                list.into()
            })
            .clone()
    }

    /// The box around every live element (none on an empty board).
    pub fn content_bounds(&self, measure: &dyn MeasureText) -> Option<Rect> {
        let list = self.ordered();
        if list.is_empty() {
            return None;
        }
        Some(list.iter().fold(
            Rect {
                min_x: f64::INFINITY,
                min_y: f64::INFINITY,
                max_x: f64::NEG_INFINITY,
                max_y: f64::NEG_INFINITY,
            },
            |acc, e| {
                let b = element_bounds(&e.el, measure);
                Rect {
                    min_x: acc.min_x.min(b.min_x),
                    min_y: acc.min_y.min(b.min_y),
                    max_x: acc.max_x.max(b.max_x),
                    max_y: acc.max_y.max(b.max_y),
                }
            },
        ))
    }

    fn touch(&mut self) {
        *self.ordered.get_mut() = None;
        self.revision += 1;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn env(id: &str, version: i64, nonce: i64, deleted: bool, data: Value) -> Envelope {
        Envelope {
            id: id.into(),
            version,
            version_nonce: nonce,
            is_deleted: deleted,
            data: data.as_object().unwrap().clone(),
        }
    }

    pub fn line(z: f64, x2: f64) -> Value {
        json!({"type":"line","color":"ink","size":2,"z":z,"x1":0,"y1":0,"x2":x2,"y2":0})
    }

    fn x2_of(scene: &BoardScene, id: &str) -> f64 {
        match scene.element(id).unwrap().kind {
            ElementKind::Shape { x2, .. } => x2,
            _ => panic!("shape"),
        }
    }

    #[test]
    fn a_higher_version_wins_and_a_tie_goes_to_the_lower_nonce() {
        let a = env("a", 2, 10, false, line(1.0, 1.0));
        assert!(wins(&a, &env("a", 1, 0, false, line(1.0, 1.0))));
        assert!(!wins(&a, &env("a", 3, 99, false, line(1.0, 1.0))));
        assert!(wins(&a, &env("a", 2, 11, false, line(1.0, 1.0))));
        assert!(!wins(&a, &env("a", 2, 10, false, line(1.0, 1.0))));
        assert!(!wins(&a, &env("a", 2, 9, false, line(1.0, 1.0))));
    }

    #[test]
    fn merge_ignores_losing_writes_whatever_their_order() {
        let mut s = BoardScene::new();
        assert!(s.merge([env("a", 2, 5, false, line(1.0, 20.0))]));
        // An older write arriving late changes nothing and does not bump the revision.
        let rev = s.revision();
        assert!(!s.merge([env("a", 1, 0, false, line(1.0, 10.0))]));
        assert_eq!(s.revision(), rev);
        assert_eq!(x2_of(&s, "a"), 20.0);
        // Same version, lower nonce wins.
        assert!(s.merge([env("a", 2, 4, false, line(1.0, 30.0))]));
        assert_eq!(x2_of(&s, "a"), 30.0);
    }

    #[test]
    fn a_tombstone_keeps_an_older_write_from_bringing_the_element_back() {
        let mut s = BoardScene::new();
        s.merge([env("a", 1, 1, false, line(1.0, 1.0))]);
        s.merge([env("a", 2, 1, true, json!({}))]);
        assert!(s.element("a").is_none());
        assert!(!s.merge([env("a", 1, 1, false, line(1.0, 1.0))]));
        assert!(s.element("a").is_none());
        assert!(s.ordered().is_empty());
    }

    #[test]
    fn a_malformed_element_is_kept_as_nothing_and_still_blocks_older_writes() {
        let mut s = BoardScene::new();
        assert!(s.merge([env("a", 3, 1, false, json!({"type":"line"}))]));
        assert!(s.element("a").is_none());
        assert!(!s.merge([env("a", 2, 1, false, line(1.0, 1.0))]));
    }

    #[test]
    fn elements_are_ordered_by_z_then_id_and_the_view_is_left_out() {
        let mut s = BoardScene::new();
        s.merge([
            env("c", 1, 1, false, line(5.0, 1.0)),
            env("b", 1, 1, false, line(5.0, 1.0)),
            env("a", 1, 1, false, line(9.0, 1.0)),
            env("d", 1, 1, false, line(1.0, 1.0)),
            env(
                "view",
                1,
                1,
                false,
                json!({"type":"view","color":"ink","size":1,"z":0,"page":"s1"}),
            ),
        ]);
        let ids: Vec<String> = s.ordered().iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids, ["d", "b", "c", "a"]);
        assert_eq!(s.view(), Some("s1"));
    }

    #[test]
    fn clear_forgets_everything_and_bumps_the_revision() {
        let mut s = BoardScene::new();
        s.merge([env("a", 1, 1, false, line(1.0, 1.0))]);
        let rev = s.revision();
        s.clear();
        assert!(s.ordered().is_empty());
        assert!(s.revision() > rev);
        // After a clear an older write is accepted again: the web player's clear drops tombstones too.
        assert!(s.merge([env("a", 1, 1, false, line(1.0, 1.0))]));
    }

    #[test]
    fn content_bounds_cover_every_live_element() {
        let mut s = BoardScene::new();
        let m = |_: &str, _: f64| 0.0;
        assert!(s.content_bounds(&m).is_none());
        s.merge([
            env("a", 1, 1, false, line(1.0, 10.0)),
            env("b", 1, 1, false, line(2.0, 50.0)),
        ]);
        let b = s.content_bounds(&m).unwrap();
        assert_eq!(
            (b.min_x, b.max_x, b.min_y, b.max_y),
            (-1.0, 51.0, -1.0, 1.0)
        );
    }
}
