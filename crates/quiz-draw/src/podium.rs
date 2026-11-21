//! The end of the game (`quiz/components/podium-view.svelte` and `score-list.svelte`), as a recording shows
//! it: the drumroll is motion, which the render has switched off, so the three steps, the names and the full
//! order are all there at once.

use replay_model::quiz::protocol::ScoreLine;
use taffy::prelude::{AlignItems, JustifyContent, NodeId, length, percent};

use crate::color::Rgba;
use crate::kit::{BASE, BLACK, BOLD, Cx, EXTRABOLD, MEDIUM, R_LG, R_XL, SM, XL3, avatar_disc, spec};
use crate::tokens;
use crate::ui::{Content, S, Visual};

/// How a screen draws a player's picture and writes a score.
pub struct Flavour<'a> {
    pub avatar: &'a dyn Fn(&mut Cx, &str, f32, (f32, f32)) -> NodeId,
    pub points: &'a dyn Fn(&Cx, i64) -> String,
}

/// The race's flavour: animal avatars, "n pts".
pub fn race_flavour<'a>() -> Flavour<'a> {
    Flavour {
        avatar: &|c, name, size, font| avatar_disc(c, name, size, font),
        points: &|c, n| c.t("quiz.points", &[("n", n.to_string())]),
    }
}

pub fn podium(c: &mut Cx, top: &[ScoreLine], all: Option<&[ScoreLine]>, flavour: &Flavour) -> NodeId {
    let tall = c.b.env.min_h(701);
    let title = c.t("quiz.podium", &[]);
    let heading = c.b.text("h2", S::block(), Visual::default(), spec(title, XL3, EXTRABOLD, tokens::FOREGROUND));
    let header = c.b.div(
        "div",
        S::row().shrink0().items(AlignItems::CENTER).justify(JustifyContent::SPACE_BETWEEN).gap(12.0),
        Visual::default(),
        &[heading],
    );

    // On screen: second place, first, third.
    let steps: [(usize, f32, Rgba); 3] = [
        (2, if tall { 144.0 } else { 64.0 }, tokens::SLATE_500),
        (1, if tall { 192.0 } else { 96.0 }, tokens::AMBER_600),
        (3, if tall { 96.0 } else { 48.0 }, tokens::ORANGE_800),
    ];
    let mut cells = Vec::new();
    for (place, height, tone) in steps {
        let line = top.get(place - 1);
        let mut kids = Vec::new();
        if let Some(line) = line {
            let mut inner = Vec::new();
            if place == 1 {
                inner.push(c.b.node(
                    S::block().size(28.0),
                    Visual { tag: "svg", ..Default::default() },
                    Content::Art {
                        art: c.lib.icon("crown"),
                        tint: tokens::AMBER_500_TEXT,
                        fill: Some(tokens::AMBER_400),
                        stroke_width: None,
                    },
                    &[],
                ));
            }
            inner.push((flavour.avatar)(c, &line.name, 36.0, (20.0, 28.0)));
            inner.push(c.b.text(
                "span",
                S::block().w_full().clip(),
                Visual::default(),
                spec(line.name.clone(), BASE, BOLD, tokens::FOREGROUND).one_line().center(),
            ));
            inner.push(c.b.text(
                "span",
                S::block(),
                Visual::default(),
                spec((flavour.points)(c, line.score), SM, EXTRABOLD, tokens::FOREGROUND).tab().center(),
            ));
            kids.push(c.b.div(
                "div",
                S::col().w_full().min0().items(AlignItems::CENTER).gap(2.0),
                Visual::default(),
                &inner,
            ));
        }
        let label = c.b.text("span", S::block(), Visual::default(), spec(place.to_string(), XL3, BLACK, Rgba::WHITE));
        kids.push(c.b.div(
            "div",
            S::row().w_full().h(height).pad(0.0, 4.0, 0.0, 0.0).items(AlignItems::START).justify(JustifyContent::CENTER),
            Visual { opacity: line.is_none().then_some(0.3), flat_bottom: true, ..Visual::default().bg(tone).radius(18.0) },
            &[label],
        ));
        let mut style = S::col().max_w(176.0).min0().items(AlignItems::CENTER).gap(4.0);
        style.0.size.width = percent(1.0 / 3.0);
        cells.push(c.b.div("li", style, Visual::default(), &kids));
    }
    let steps_row = c.b.div(
        "ol",
        S::row().shrink0().items(AlignItems::END).justify(JustifyContent::CENTER).gap(16.0),
        Visual::default(),
        &cells,
    );

    let tail: Vec<NodeId> = all.map(|a| score_list(c, a, flavour)).into_iter().collect();
    let rest = c.b.div("div", S::block().min0().flex1(), Visual::default().scroll(), &tail);
    c.b.div("div", S::col().min0().flex1().gap(16.0), Visual::default().scroll(), &[header, steps_row, rest])
}

fn score_list(c: &mut Cx, lines: &[ScoreLine], flavour: &Flavour) -> NodeId {
    let mut rows = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let vis = Visual::default().bg(tokens::CARD).border(1.0, tokens::BORDER).radius(R_XL);
        let row = S::row().min0().items(AlignItems::CENTER).gap(12.0).pad(12.0, 8.0, 12.0, 8.0).border(1.0);
        if c.b.env.page {
            // The page skips what is off screen (`content-visibility: auto`): a row is 44 px and empty.
            rows.push(c.b.div("li", row.h(44.0), vis, &[]));
            continue;
        }
        let rank = c.b.text(
            "span",
            S::row().size(28.0).shrink0().items(AlignItems::CENTER).justify(JustifyContent::CENTER),
            Visual::default().bg(tokens::MUTED).radius(R_LG).vcenter(),
            spec((i + 1).to_string(), SM, BOLD, tokens::FOREGROUND).tab().center(),
        );
        let avatar = (flavour.avatar)(c, &line.name, 28.0, BASE);
        let name = c.b.text(
            "span",
            S::block().min0().flex1(),
            Visual::default(),
            spec(line.name.clone(), SM, MEDIUM, tokens::FOREGROUND).one_line(),
        );
        let points = c.b.text(
            "span",
            S::block(),
            Visual::default(),
            spec((flavour.points)(c, line.score), SM, BOLD, tokens::FOREGROUND).tab(),
        );
        rows.push(c.b.div("li", row, vis, &[rank, avatar, name, points]));
    }
    let mut list = S::col().gap(6.0);
    list.0.min_size.width = length(0.0);
    c.b.div("ol", list, Visual::default(), &rows)
}
