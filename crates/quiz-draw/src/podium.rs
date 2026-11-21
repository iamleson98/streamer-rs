//! The end of the game (`quiz/components/podium-view.svelte` and `score-list.svelte`), as a recording shows
//! it: the drumroll is motion, which the render has switched off, so the three steps, the names and the full
//! order are all there at once.

use replay_model::quiz::protocol::ScoreLine;
use taffy::prelude::{AlignItems, JustifyContent, NodeId, length, percent};

use crate::color::Rgba;
use crate::kit::{
    BASE, BLACK, BOLD, Cx, EXTRABOLD, MEDIUM, R_LG, R_XL, SM, XL3, avatar_disc, ease, spec,
};
use crate::tokens;
use crate::ui::{Content, S, Visual, Xform};

/// Draws a player's picture of a size, with the emoji's text step.
pub type AvatarFn<'a> = dyn Fn(&mut Cx, &str, f32, (f32, f32)) -> NodeId + 'a;

/// How a screen draws a player's picture and writes a score.
pub struct Flavour<'a> {
    pub avatar: &'a AvatarFn<'a>,
    /// The score written under a name on the podium and in the list.
    pub points: &'a dyn Fn(&Cx, i64) -> String,
    /// The race's podium shows a bare number under a name (`CountUp`), the list "n pts".
    pub bare_on_steps: bool,
}

/// The race's flavour: animal avatars, "n pts".
impl std::fmt::Debug for Flavour<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flavour").finish_non_exhaustive()
    }
}

pub fn race_flavour<'a>() -> Flavour<'a> {
    Flavour {
        avatar: &|c, name, size, font| avatar_disc(c, name, size, font),
        points: &|c, n| c.t("quiz.points", &[("n", n.to_string())]),
        bare_on_steps: true,
    }
}

pub fn podium(
    c: &mut Cx,
    top: &[ScoreLine],
    all: Option<&[ScoreLine]>,
    flavour: &Flavour,
) -> NodeId {
    let tall = c.b.env.min_h(701);
    let title = c.t("quiz.podium", &[]);
    let heading = c.b.text(
        "h2",
        S::block(),
        Visual::default(),
        spec(title, XL3, EXTRABOLD, tokens::FOREGROUND),
    );
    let header = c.b.div(
        "div",
        S::row()
            .shrink0()
            .items(AlignItems::CENTER)
            .justify(JustifyContent::SPACE_BETWEEN)
            .gap(12.0),
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
        let delay = match place {
            2 => 1100.0,
            1 => 2200.0,
            _ => 0.0,
        };
        let m = c.age(c.since.podium);
        let mut kids = Vec::new();
        if let Some(line) = line {
            let mut inner = Vec::new();
            if place == 1 {
                inner.push(c.b.node(
                    S::block().size(28.0),
                    Visual {
                        tag: "svg",
                        ..Default::default()
                    },
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
            inner.push(
                c.b.text(
                    "span",
                    S::block().w_full().clip(),
                    Visual::default(),
                    spec(line.name.clone(), BASE, BOLD, tokens::FOREGROUND)
                        .one_line()
                        .center(),
                ),
            );
            inner.push(
                c.b.text(
                    "span",
                    S::block(),
                    Visual::default(),
                    spec(
                        if flavour.bare_on_steps {
                            m.map_or(line.score, |m| crate::motion::count_up(m, 0, line.score))
                                .to_string()
                        } else {
                            (flavour.points)(c, line.score)
                        },
                        SM,
                        EXTRABOLD,
                        tokens::FOREGROUND,
                    )
                    .tab()
                    .center(),
                ),
            );
            let mut name_vis = Visual::default();
            if let Some(m) = m {
                // `qz-step-name`: pops in 450 ms after its step starts.
                let e = ease((m - delay - 450.0) / 420.0, 0.34, 1.56, 0.64, 1.0);
                let k = 0.4 + 0.6 * e;
                name_vis.opacity = Some(((m - delay - 450.0) / 420.0).clamp(0.0, 1.0));
                name_vis.xform = Some(Xform {
                    tx: 0.0,
                    ty: 0.0,
                    rot: 0.0,
                    sx: k,
                    sy: k,
                    from_bottom: false,
                    skew_x: 0.0,
                    tx_w: 0.0,
                });
            }
            kids.push(c.b.div(
                "div",
                S::col().w_full().min0().items(AlignItems::CENTER).gap(2.0),
                name_vis,
                &inner,
            ));
        }
        let label = c.b.text(
            "span",
            S::block(),
            Visual::default(),
            spec(place.to_string(), XL3, BLACK, Rgba::WHITE),
        );
        kids.push(
            c.b.div(
                "div",
                S::row()
                    .w_full()
                    .h(height)
                    .pad(0.0, 4.0, 0.0, 0.0)
                    .items(AlignItems::START)
                    .justify(JustifyContent::CENTER),
                Visual {
                    opacity: line.is_none().then_some(0.3),
                    flat_bottom: true,
                    // `qz-step`: rises from the bottom over 700 ms.
                    xform: m.map(|m| Xform {
                        tx: 0.0,
                        ty: 0.0,
                        rot: 0.0,
                        sx: 1.0,
                        sy: ease((m - delay) / 700.0, 0.34, 1.3, 0.64, 1.0),
                        from_bottom: true,
                        skew_x: 0.0,
                        tx_w: 0.0,
                    }),
                    ..Visual::default().bg(tone).radius(18.0)
                },
                &[label],
            ),
        );
        let mut style = S::col()
            .max_w(176.0)
            .min0()
            .items(AlignItems::CENTER)
            .gap(4.0);
        style.0.size.width = percent(1.0 / 3.0);
        cells.push(c.b.div("li", style, Visual::default(), &kids));
    }
    let steps_row = c.b.div(
        "ol",
        S::row()
            .shrink0()
            .items(AlignItems::END)
            .justify(JustifyContent::CENTER)
            .gap(16.0),
        Visual::default(),
        &cells,
    );

    let tail: Vec<NodeId> = all.map(|a| score_list(c, a, flavour)).into_iter().collect();
    let rest = c.b.div(
        "div",
        S::block().min0().flex1(),
        Visual::default().scroll(),
        &tail,
    );
    let mut kids = vec![header, steps_row, rest];
    if let Some(m) = c.age(c.since.podium) {
        kids.push(confetti(c, m));
    }
    c.b.div(
        "div",
        S::col().min0().flex1().gap(16.0).relative(),
        Visual::default().scroll(),
        &kids,
    )
}

/// More rows than a stage can show (it is under 700 px tall; a row is 46).
const MAX_ROWS: usize = 24;

fn score_list(c: &mut Cx, lines: &[ScoreLine], flavour: &Flavour) -> NodeId {
    let mut rows = Vec::new();
    // Only rows that can be on screen are built: the cut takes out the others, so a game of thousands costs
    // what a game of a few dozen does.
    for (i, line) in lines.iter().take(MAX_ROWS).enumerate() {
        let vis = Visual::default()
            .bg(tokens::CARD)
            .border(1.0, tokens::BORDER)
            .radius(R_XL);
        let row = S::row()
            .min0()
            .items(AlignItems::CENTER)
            .gap(12.0)
            .pad(12.0, 8.0, 12.0, 8.0)
            .border(1.0);
        let rank = c.b.text(
            "span",
            S::row()
                .size(28.0)
                .shrink0()
                .items(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
            Visual::default().bg(tokens::MUTED).radius(R_LG).vcenter(),
            spec((i + 1).to_string(), SM, BOLD, tokens::FOREGROUND)
                .tab()
                .center(),
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
            spec(
                (flavour.points)(c, line.score),
                SM,
                BOLD,
                tokens::FOREGROUND,
            )
            .tab(),
        );
        // The page skips what is off screen (`content-visibility: auto`): there a row is its 44 px intrinsic content size
        // plus its padding and border, whatever it holds.
        let row = if c.b.env.page {
            row.h(62.0).clip()
        } else {
            row
        };
        rows.push(c.b.div("li", row, vis, &[rank, avatar, name, points]));
    }
    let mut list = S::col().gap(6.0);
    list.0.min_size.width = length(0.0);
    c.b.div("ol", list, Visual::default(), &rows)
}

/// The confetti (`qz-confetti`): 36 pieces that fall from the top once the first place is up, drifting sideways
/// and turning, then fade out. Every number is the player's CSS variable for that piece.
fn confetti(c: &mut Cx, m: f32) -> NodeId {
    let tones = [
        tokens::CONFETTI_0,
        tokens::CONFETTI_1,
        tokens::CONFETTI_2,
        tokens::CONFETTI_3,
        tokens::CONFETTI_4,
        tokens::CONFETTI_5,
    ];
    let fall = c.b.env.viewport_h as f32 * 1.05;
    let pieces: Vec<NodeId> = (0..36usize)
        .map(|i| {
            let dur = (2.6 + (i % 5) as f32 * 0.35) * 1000.0;
            let start = 2300.0 + (i % 9) as f32 * 140.0;
            let p = ((m - start) / dur).clamp(0.0, 1.0);
            let x = if i % 2 == 1 { 70.0 } else { -70.0 };
            let turn = 360.0 + (i % 5) as f32 * 120.0;
            let mut style = S::block().w(8.0).h(12.0);
            style.0.position = taffy::prelude::Position::Absolute;
            style.0.inset.top = taffy::prelude::length(0.0);
            style.0.inset.left = taffy::prelude::percent(((i * 29) % 100) as f32 / 100.0);
            c.b.div(
                "i",
                style,
                Visual {
                    opacity: Some(1.0 - p),
                    xform: Some(Xform {
                        tx: x * p,
                        ty: -20.0 + (fall + 20.0) * p,
                        rot: turn * p,
                        sx: 1.0,
                        sy: 1.0,
                        from_bottom: false,
                        skew_x: 0.0,
                        tx_w: 0.0,
                    }),
                    ..Visual::default().bg(tones[i % 6]).radius(6.0)
                },
                &[],
            )
        })
        .collect();
    c.b.div(
        "div",
        S::block()
            .abs(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .clip(),
        Visual {
            z: 30,
            ..Visual::default().clipped()
        },
        &pieces,
    )
}
