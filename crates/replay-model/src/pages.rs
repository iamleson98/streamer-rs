//! The board's sheets: in what order they come and which one the teacher is on. Port of the parts
//! of `board/pages.ts` a replay uses. A sheet is one `page` element; nothing about them is stored
//! beyond that.

use crate::element::{ElementKind, Page};
use crate::scene::OrderedElement;

#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub id: String,
    pub page: Page,
}

/// The sheets among the board's elements, in the order they were laid: left to right.
pub fn sheets_of(list: &[OrderedElement]) -> Vec<Sheet> {
    let mut sheets: Vec<Sheet> = list
        .iter()
        .filter_map(|e| match &e.el.kind {
            ElementKind::Page(page) => Some(Sheet {
                id: e.id.clone(),
                page: page.clone(),
            }),
            _ => None,
        })
        .collect();
    sheets.sort_by(|a, b| a.page.x.total_cmp(&b.page.x).then_with(|| a.id.cmp(&b.id)));
    sheets
}

/// The sheet the teacher's view names; the first one until they have moved or when it is gone.
pub fn viewed_sheet<'a>(sheets: &'a [Sheet], view: Option<&str>) -> Option<&'a Sheet> {
    view.and_then(|v| sheets.iter().find(|s| s.id == v))
        .or_else(|| sheets.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::BoardScene;
    use crate::scene::tests::env;
    use serde_json::json;

    fn page(x: f64) -> serde_json::Value {
        json!({"type":"page","color":"ink","size":2,"z":0,"x":x,"y":0,"w":1280,"h":720})
    }

    fn scene() -> BoardScene {
        let mut s = BoardScene::new();
        s.merge([
            env("b", 1, 1, false, page(1600.0)),
            env("c", 1, 1, false, page(0.0)),
            env("a", 1, 1, false, page(1600.0)),
            env(
                "ink",
                1,
                1,
                false,
                json!({"type":"line","color":"ink","size":2,"z":5,"x1":0,"y1":0,"x2":1,"y2":1}),
            ),
        ]);
        s
    }

    #[test]
    fn sheets_run_left_to_right_and_ties_go_by_id() {
        let ids: Vec<String> = sheets_of(&scene().ordered())
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    #[test]
    fn the_viewed_sheet_is_the_one_named_or_else_the_first() {
        let sheets = sheets_of(&scene().ordered());
        assert_eq!(viewed_sheet(&sheets, Some("b")).unwrap().id, "b");
        assert_eq!(viewed_sheet(&sheets, Some("gone")).unwrap().id, "c");
        assert_eq!(viewed_sheet(&sheets, None).unwrap().id, "c");
        assert!(viewed_sheet(&[], Some("b")).is_none());
    }
}
