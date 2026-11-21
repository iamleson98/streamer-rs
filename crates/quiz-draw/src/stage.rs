//! The stage of a recording's quiz game: what the render's `QuizStageSource` + `createQuizDom` give. The
//! game's state at a time (`QuizReplay`), the screens as the player's components lay them out, the cut that
//! keeps half rows out of the video, and the picture.

use std::collections::HashSet;
use std::rc::Rc;

use board_draw::FontSet;
use replay_model::quiz::quest::QuestPhase;
use replay_model::quiz::{Mode, QuizRecord, QuizReplay};
use skia_safe::{Canvas, ClipOp, Rect};
use taffy::prelude::{AlignItems, Display, NodeId};

use crate::i18n::{Locale, student};
use crate::kit::{BASE, BOLD, Cx, Library, SM, spec};
use crate::text::TextEngine;
use crate::tokens;
use crate::ui::{Builder, Dump, Env, Painter, S, Tree, Visual, compute};
use crate::{quest, race};

/// The render page's viewport (the render browser's `--window-size=1280,720`): the page measures what the
/// cut looks at at this height, the picture is made at the stage's own.
const PAGE_H: u32 = 720;

/// One frame's layout and cut: a tree to paint and what the dump says about it.
struct Frame {
    b: Builder,
    root: NodeId,
    hidden: HashSet<NodeId>,
}

#[derive(Debug)]
pub struct QuizStage {
    replay: QuizReplay,
    locale: Locale,
    text: Rc<TextEngine>,
    lib: Library,
    motion: bool,
}

impl QuizStage {
    pub fn new(records: Vec<QuizRecord>, locale: Locale, fonts: Rc<FontSet>) -> Self {
        Self {
            replay: QuizReplay::new(records, student(locale)),
            locale,
            text: TextEngine::new(fonts),
            lib: Library::new(),
            motion: true,
        }
    }

    /// Draws the podium's drumroll and confetti (the steps rise, the names pop, pieces fall), which the player shows
    /// to people who have not asked for less motion. On by default (the video shows what the player shows); `with_motion(false)` draws the settled screens, which is what
    /// the reduced-motion render looks like.
    #[must_use]
    pub fn with_motion(mut self, on: bool) -> Self {
        self.motion = on;
        self
    }

    /// The lesson had a game in it.
    pub fn has(&self) -> bool {
        self.replay.has()
    }

    /// A game is on the stage at `t_ms`: it began and has not ended. Time only moves forward in a render
    /// (going back starts the game again).
    pub fn on(&mut self, t_ms: f64) -> bool {
        self.replay.seek(t_ms);
        self.replay.on()
    }

    /// Draws the stage into `stage` (its origin and size in px) at `t_ms`.
    pub fn draw(&mut self, canvas: &Canvas, stage: Rect, t_ms: f64) {
        let frame = self.frame(stage.width(), stage.height(), t_ms);
        canvas.save();
        canvas.clip_rect(stage, ClipOp::Intersect, false);
        Painter {
            tree: &frame.b.tree,
            text: &self.text,
            hidden: &frame.hidden,
            origin: Default::default(),
            raster_root: Default::default(),
        }
        .paint(canvas, frame.root, stage.left, stage.top);
        canvas.restore();
    }

    /// The layout tree of the picture as text (tag, box, depth), for finding where a box went wrong.
    pub fn debug_tree(&mut self, w: f32, h: f32, t_ms: f64) -> String {
        let frame = self.frame(w, h, t_ms);
        let mut out = String::new();
        fn walk(tree: &Tree, id: NodeId, depth: usize, out: &mut String) {
            let l = tree.layout(id).expect("layout");
            let tag = tree.get_node_context(id).map_or("", |c| c.vis.tag);
            let style = tree.style(id).expect("style");
            out.push_str(&format!(
                "{}{tag} x={:.2} y={:.2} w={:.2} h={:.2}{}\n",
                "  ".repeat(depth),
                l.location.x,
                l.location.y,
                l.size.width,
                l.size.height,
                if style.display == Display::None {
                    " (none)"
                } else {
                    ""
                }
            ));
            for k in tree.children(id).expect("children") {
                walk(tree, k, depth + 1, out);
            }
        }
        walk(&frame.b.tree, frame.root, 0, &mut out);
        out
    }

    /// The page's layout dump of the stage at `t_ms` (text lines and painted boxes).
    pub fn layout_dump(&mut self, w: f32, h: f32, t_ms: f64) -> Dump {
        let frame = self.frame(w, h, t_ms);
        Painter {
            tree: &frame.b.tree,
            text: &self.text,
            hidden: &frame.hidden,
            origin: Default::default(),
            raster_root: Default::default(),
        }
        .dump(frame.root, 0.0, 0.0)
    }

    fn frame(&mut self, w: f32, h: f32, t_ms: f64) -> Frame {
        self.replay.seek(t_ms);
        // The page lays the screens out in a tall viewport and cuts what its scrolling boxes would cut;
        // the picture is made at the stage's own height with the same rows left out.
        let (mut page, page_root) = self.build(
            Env {
                viewport_h: PAGE_H,
                page: true,
            },
            t_ms,
        );
        compute(&mut page.tree, page_root, &self.text, w, h);
        let hidden_at = cut(&mut page, page_root, &self.text, w, h);
        let (mut image, root) = self.build(
            Env {
                viewport_h: h as u32,
                page: false,
            },
            t_ms,
        );
        // The page's tree and the picture's must be the same screen node for node: the cut is found in one and applied
        // to the other by position.
        debug_assert_eq!(
            page.order.len(),
            image.order.len(),
            "the two trees differ at {h} px"
        );
        let hidden: HashSet<NodeId> = hidden_at
            .into_iter()
            .filter_map(|i| image.order.get(i).copied())
            .collect();
        for id in &hidden {
            hide(&mut image.tree, *id);
        }
        compute(&mut image.tree, root, &self.text, w, h);
        Frame {
            b: image,
            root,
            hidden,
        }
    }

    fn build(&self, env: Env, t_ms: f64) -> (Builder, NodeId) {
        let mut c = Cx {
            b: Builder::new(self.text.clone(), env),
            lib: &self.lib,
            locale: self.locale,
            motion: self.motion,
            t_ms,
            since: &self.replay.since,
        };
        // A Quest game that has not shown its lobby (the recording began in the middle of it) says there is none.
        let none = self.replay.mode == Mode::Quest && self.replay.quest.phase == QuestPhase::None;
        let (title, body) = if none {
            (
                String::new(),
                c.b.div("div", S::block(), Visual::default(), &[]),
            )
        } else {
            match self.replay.mode {
                Mode::Quest => (
                    self.replay.quest.title.clone(),
                    quest::body(&mut c, &self.replay.quest, t_ms),
                ),
                Mode::Race => (
                    self.replay.game.title.clone(),
                    race::body(&mut c, &self.replay, t_ms),
                ),
            }
        };
        let mut kids = Vec::new();
        // Built at every height (the picture's tree and the page's must correspond node for node); a short
        // stage hides it (`[@media(min-height:561px)]:block`).
        let heading = c.b.text(
            "h2",
            if env.min_h(561) {
                S::block()
            } else {
                S::none()
            }
            .shrink0()
            .clip(),
            Visual::default(),
            spec(title, BASE, BOLD, tokens::FOREGROUND).one_line(),
        );
        let body =
            c.b.div("div", S::col().min0().flex1(), Visual::default(), &[body]);
        if none {
            let text = c.t("replay.quiz.none", &[]);
            let message = c.b.text(
                "p",
                S::block().m_auto(),
                Visual::default(),
                spec(text, SM, 400, tokens::MUTED_FOREGROUND),
            );
            kids.push(message);
        } else {
            kids.push(heading);
            kids.push(body);
        }
        let stage = c.b.div(
            "div",
            S::col()
                .full()
                .min0()
                .gap(8.0)
                .p(20.0)
                .items(AlignItems::STRETCH)
                .relative(),
            Visual::default().bg(tokens::BACKGROUND).clipped(),
            &kids,
        );
        let root =
            c.b.div("div", S::block().full(), Visual::default(), &[stage]);
        (c.b, root)
    }
}

fn hide(tree: &mut Tree, id: NodeId) {
    let mut style = tree.style(id).expect("style").clone();
    style.display = Display::None;
    tree.set_style(id, style).expect("style");
}

/// The nodes of a subtree in document order.
fn preorder(tree: &Tree, id: NodeId, out: &mut Vec<NodeId>) {
    out.push(id);
    for kid in tree.children(id).expect("children") {
        preorder(tree, kid, out);
    }
}

/// A node's box in the root's coordinates.
fn bounds(
    tree: &Tree,
    id: NodeId,
    parent: &std::collections::HashMap<NodeId, NodeId>,
) -> (f32, f32, f32, f32) {
    let l = tree.layout(id).expect("layout");
    let (mut x, mut y) = (l.location.x, l.location.y);
    let mut at = id;
    while let Some(p) = parent.get(&at) {
        let pl = tree.layout(*p).expect("layout");
        x += pl.location.x;
        y += pl.location.y;
        at = *p;
    }
    (x, y, l.size.width, l.size.height)
}

/// Hides what a scrolling box cuts at its bottom edge, so the video never shows half a row (`cutPartialRows` in
/// `export/quiz-dom.svelte.ts`): rows that do not end inside it are taken out. Returns what was hidden as
/// indices into the builder's node order.
fn cut(b: &mut Builder, root: NodeId, text: &TextEngine, w: f32, h: f32) -> Vec<usize> {
    let mut all = Vec::new();
    preorder(&b.tree, root, &mut all);
    let index: std::collections::HashMap<NodeId, usize> =
        b.order.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut hidden = Vec::new();
    for id in all {
        let Some(ctx) = b.tree.get_node_context(id).cloned() else {
            continue;
        };
        let style = b.tree.style(id).expect("style");
        if !ctx.vis.scroll || style.display == Display::None || !shown_chain(&b.tree, id) {
            continue;
        }
        let l = b.tree.layout(id).expect("layout");
        if l.scroll_height() <= 1.0 {
            continue;
        }
        let mut parent = std::collections::HashMap::new();
        fn fill(tree: &Tree, id: NodeId, parent: &mut std::collections::HashMap<NodeId, NodeId>) {
            for kid in tree.children(id).expect("children") {
                parent.insert(kid, id);
                fill(tree, kid, parent);
            }
        }
        fill(&b.tree, root, &mut parent);
        let bound = {
            let (_, y, _, hh) = bounds(&b.tree, id, &parent);
            y + hh
        };
        let mut to_hide = Vec::new();
        visit(&b.tree, id, bound, &parent, &mut to_hide);
        if to_hide.is_empty() {
            continue;
        }
        for n in &to_hide {
            hide(&mut b.tree, *n);
            if let Some(i) = index.get(n) {
                hidden.push(*i);
            }
        }
        compute(&mut b.tree, root, text, w, h);
    }
    hidden
}

fn shown_chain(tree: &Tree, id: NodeId) -> bool {
    tree.style(id).map_or(true, |s| s.display != Display::None)
}

/// The elements among a node's children that are shown, with the tag their kind is by.
fn element_kids(tree: &Tree, id: NodeId) -> Vec<NodeId> {
    tree.children(id)
        .expect("children")
        .into_iter()
        .filter(|k| {
            tree.get_node_context(*k)
                .is_some_and(|c| !c.vis.tag.is_empty())
        })
        .collect()
}

/// A list's rows: two or more shown children of one kind (items of a list, cards of a board).
fn rows_of(tree: &Tree, id: NodeId) -> Vec<NodeId> {
    let kids: Vec<NodeId> = element_kids(tree, id)
        .into_iter()
        .filter(|k| tree.style(*k).is_ok_and(|s| s.display != Display::None))
        .collect();
    let tag = |k: &NodeId| tree.get_node_context(*k).map_or("", |c| c.vis.tag);
    if kids.len() > 1 && kids.iter().all(|k| tag(k) == tag(&kids[0])) {
        kids
    } else {
        vec![]
    }
}

fn visit(
    tree: &Tree,
    parent_id: NodeId,
    bound: f32,
    parent: &std::collections::HashMap<NodeId, NodeId>,
    out: &mut Vec<NodeId>,
) {
    let kids: Vec<NodeId> = element_kids(tree, parent_id)
        .into_iter()
        .filter(|k| tree.style(*k).is_ok_and(|s| s.display != Display::None))
        .collect();
    // Columns side by side (a grid's two) are cut one by one, as the player's `cutPartialRows` does since
    // lesson-replay#3: a stretched column ends below the box and must not take its whole content out.
    let tops: Vec<f32> = kids.iter().map(|k| bounds(tree, *k, parent).1).collect();
    if kids.len() > 1 && tops.iter().all(|t| (t - tops[0]).abs() < 1.0) {
        for kid in kids {
            visit(tree, kid, bound, parent, out);
        }
        return;
    }
    let rows = rows_of(tree, parent_id);
    let items = if rows.is_empty() { kids } else { rows };
    let whole = items
        .iter()
        .position(|k| {
            let (_, y, _, h) = bounds(tree, *k, parent);
            y + h > bound + 0.5
        })
        .unwrap_or(items.len());
    if let Some(straddling) = items.get(whole)
        && rows_of(tree, parent_id).is_empty()
    {
        // A wrapper around the list: look inside the child that crosses the edge.
        let (_, y, _, _) = bounds(tree, *straddling, parent);
        if y < bound {
            visit(tree, *straddling, bound, parent, out);
        }
        out.extend(items.iter().skip(whole + 1).copied());
        return;
    }
    out.extend(items.iter().skip(whole).copied());
}
