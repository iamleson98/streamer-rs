//! The race's stage of a recording (`stages/race-replay-stage.svelte`, with the host's lobby, question and
//! reveal screens of `quiz/components/`): what the teacher saw, watching only. The player's own parts
//! (answering, their rank) are not on a recording's screen.

use replay_model::quiz::QuizReplay;
use replay_model::quiz::fit::{Text, js_len, option_size, question_size, tile_columns};
use replay_model::quiz::format::countdown;
use replay_model::quiz::protocol::ScoreLine;
use replay_model::quiz::race::{Phase, QuizGame, Reveal};
use taffy::prelude::{
    AlignContent, AlignItems, GridTemplateComponent, JustifyContent, NodeId, TrackSizingFunction,
};
use taffy::style::{MaxTrackSizingFunction, MinTrackSizingFunction};

use crate::color::Rgba;
use crate::kit::{
    BASE, BLACK, BOLD, Cx, EXTRABOLD, LG, R_2XL, R_LG, R_XL, SEMIBOLD, SM, XL, XL3, XS,
    avatar_disc, ring, ring_text, spec, tile_colours,
};
use crate::motion;
use crate::podium::{podium, race_flavour};
use crate::tokens;
use crate::ui::{Content, S, Visual, Xform};

pub fn body(c: &mut Cx, replay: &QuizReplay, now: f64) -> NodeId {
    let game = &replay.game;
    match game.phase {
        Phase::Lobby => lobby(c, game),
        Phase::Question => question(c, game, now),
        Phase::Reveal => reveal(c, game),
        _ => match &game.podium {
            Some(p) => podium(c, &p.top, p.all.as_deref(), &race_flavour()),
            None => c.b.div("div", S::block(), Visual::default(), &[]),
        },
    }
}

fn step(t: Text) -> (f32, f32) {
    match t {
        Text::Sm => SM,
        Text::Base => BASE,
        Text::Lg => LG,
        Text::Xl => XL,
        Text::X2l => (24.0, 32.0),
        Text::X3l => XL3,
        Text::X4l => (36.0, 40.0),
    }
}

// ───────────────────────── lobby ─────────────────────────

fn lobby(c: &mut Cx, game: &QuizGame) -> NodeId {
    const SHOWN: usize = 120;
    let users = c.b.node(
        S::block().size(48.0).shrink0(),
        Visual {
            tag: "svg",
            ..Default::default()
        },
        Content::Art {
            art: c.lib.icon("users"),
            tint: tokens::PRIMARY,
            fill: None,
            stroke_width: None,
        },
        &[],
    );
    let count_age = c.age(c.since.players);
    let count = c.b.text(
        "p",
        S::block().shrink0(),
        Visual::default(),
        spec(
            game.players.to_string(),
            (96.0, 96.0),
            BLACK,
            tokens::FOREGROUND,
        )
        .tab()
        .center(),
    );
    if let Some(age) = count_age {
        let (o, k) = motion::pop(age);
        c.animate(count, Some(o), Some(Xform::new(0.0, 0.0, 0.0, k, k)));
    }
    let line = format!(
        "{} · {}",
        c.t("quiz.host.joined", &[("count", game.players.to_string())]),
        c.t(
            "quiz.host.questions",
            &[("count", game.question_count.to_string())]
        )
    );
    let caption = c.b.text(
        "p",
        S::block().shrink0(),
        Visual::default(),
        spec(line, LG, SEMIBOLD, tokens::MUTED_FOREGROUND).center(),
    );
    let mut kids = vec![users, count, caption];
    let names: Vec<&String> = game.joined.iter().rev().take(SHOWN).rev().collect();
    if names.is_empty() {
        let text = c.t("quiz.lobby.waitingNames", &[]);
        kids.push(c.b.text(
            "p",
            S::block().shrink0(),
            Visual::default(),
            spec(text, SM, 400, tokens::MUTED_FOREGROUND).center(),
        ));
    } else {
        let mut chips = Vec::new();
        let first = game.joined.len() - names.len();
        for (j, name) in names.into_iter().enumerate() {
            let avatar = avatar_disc(c, name, 36.0, XL);
            let label = c.b.text(
                "span",
                S::block().max_w(128.0).min0(),
                Visual::default(),
                spec(name.clone(), SM, BOLD, tokens::FOREGROUND).one_line(),
            );
            let chip_age = c.age(c.since.joined.get(first + j).copied());
            let chip = c.b.div(
                "li",
                S::row()
                    .max_w_pct(1.0)
                    .min0()
                    .items(AlignItems::CENTER)
                    .gap(8.0)
                    .pad(6.0, 4.0, 12.0, 4.0)
                    .border(1.0),
                Visual::default()
                    .bg(tokens::CARD)
                    .border(1.0, tokens::BORDER)
                    .radius(9999.0),
                &[avatar, label],
            );
            if let Some(age) = chip_age {
                let (o, k) = motion::pop(age);
                c.animate(chip, Some(o), Some(Xform::new(0.0, 0.0, 0.0, k, k)));
            }
            chips.push(chip);
        }
        if game.joined.len() > SHOWN {
            let more = c.t(
                "quiz.lobby.more",
                &[("n", (game.joined.len() - SHOWN).to_string())],
            );
            chips.push(c.b.text(
                "li",
                S::block().pad(8.0, 0.0, 8.0, 0.0),
                Visual::default(),
                spec(more, SM, SEMIBOLD, tokens::MUTED_FOREGROUND),
            ));
        }
        kids.push(
            c.b.div(
                "ul",
                S::row()
                    .w_full()
                    .max_w(768.0)
                    .min0()
                    .flex1()
                    .wrap()
                    .content(AlignContent::START)
                    .justify(JustifyContent::CENTER)
                    .gap(8.0),
                Visual::default().scroll(),
                &chips,
            ),
        );
    }
    c.b.div(
        "div",
        S::col().min0().flex1().items(AlignItems::CENTER).gap(16.0),
        Visual::default(),
        &kids,
    )
}

// ───────────────────────── question ─────────────────────────

/// Tailwind's `space-y-*`: a margin under every child but the last.
fn space_y(c: &mut Cx, kids: &[NodeId], px: f32) {
    for id in kids.iter().take(kids.len().saturating_sub(1)) {
        let mut st = c.b.tree.style(*id).expect("style").clone();
        st.margin.bottom = taffy::prelude::length(px);
        c.b.tree.set_style(*id, st).expect("style");
    }
}

fn fr1() -> TrackSizingFunction {
    TrackSizingFunction {
        min: MinTrackSizingFunction::length(0.0),
        max: MaxTrackSizingFunction::fr(1.0),
    }
}

fn fixed(px: f32) -> TrackSizingFunction {
    TrackSizingFunction {
        min: MinTrackSizingFunction::length(px),
        max: MaxTrackSizingFunction::length(px),
    }
}

fn cols(tracks: Vec<TrackSizingFunction>) -> Vec<GridTemplateComponent<String>> {
    tracks
        .into_iter()
        .map(GridTemplateComponent::Single)
        .collect()
}

fn question(c: &mut Cx, game: &QuizGame, now: f64) -> NodeId {
    let q = game
        .question
        .as_ref()
        .expect("a question phase has a question");
    let cd = countdown(q.ends_at as f64, q.limit_ms as f64, now);
    let ring_px = if c.b.env.min_h(700) { 64.0 } else { 56.0 };

    // The header: question number, the answered ring and the countdown.
    let number = c.t(
        "quiz.questionOf",
        &[("n", q.q.to_string()), ("total", q.count.to_string())],
    );
    let mut left = vec![c.b.text(
        "span",
        S::block().pad(12.0, 4.0, 12.0, 4.0),
        Visual::default().bg(tokens::PRIMARY_10).radius(9999.0),
        spec(number, SM, BOLD, tokens::PRIMARY),
    )];
    if q.kind == 1 {
        let text = c.t("quiz.kind.multiple", &[]);
        left.push(c.b.text(
            "span",
            S::block().pad(12.0, 4.0, 12.0, 4.0),
            Visual::default().bg(tokens::MUTED).radius(9999.0),
            spec(text, XS, BOLD, tokens::FOREGROUND),
        ));
    }
    if game.closing {
        let text = c.t("quiz.closing.host", &[]);
        let label = c.b.text(
            "span",
            S::block(),
            Visual::default(),
            spec(text, SM, BOLD, tokens::MUTED_FOREGROUND),
        );
        let closing_age = c.age(c.since.closing);
        let dots: Vec<NodeId> = (0..3)
            .map(|n| {
                let dot = c.b.div(
                    "span",
                    S::block().size(6.0),
                    Visual::default()
                        .bg(tokens::MUTED_FOREGROUND)
                        .radius(9999.0),
                    &[],
                );
                if let Some(age) = closing_age {
                    c.animate(
                        dot,
                        None,
                        Some(Xform::new(
                            0.0,
                            motion::dot(age, n as f32 * 150.0),
                            0.0,
                            1.0,
                            1.0,
                        )),
                    );
                }
                dot
            })
            .collect();
        let dots = c.b.div("span", S::row().gap(4.0), Visual::default(), &dots);
        left.push(
            c.b.div(
                "span",
                S::row()
                    .items(AlignItems::CENTER)
                    .gap(8.0)
                    .pad(12.0, 4.0, 12.0, 4.0),
                Visual::default().bg(tokens::MUTED).radius(9999.0),
                &[label, dots],
            ),
        );
    }
    let left = c.b.div(
        "div",
        S::row().min0().wrap().items(AlignItems::CENTER).gap(6.0),
        Visual::default(),
        &left,
    );

    let toggle = c.b.div(
        "button",
        S::block().size(44.0).shrink0(),
        Visual::default().hidden(),
        &[],
    );
    let fraction = if game.players == 0 {
        0.0
    } else {
        game.answered as f32 / game.players as f32
    };
    let answered_text = ring_text(c, &format!("{}/{}", game.answered, game.players), SM);
    let answered = ring(c, ring_px, fraction, tokens::GREEN_600, answered_text);
    let (shown, f) = if game.closing {
        (0, 0.0)
    } else {
        (cd.seconds, cd.fraction as f32)
    };
    let tone = if cd.urgent {
        tokens::RED_700
    } else if cd.seconds <= 10 {
        tokens::AMBER_600_TEXT
    } else {
        tokens::PRIMARY
    };
    let tone = if game.closing { tokens::PRIMARY } else { tone };
    let timer_text = ring_text(c, &shown.to_string(), XL);
    let timer = ring(c, ring_px, f, tone, timer_text);
    let urgent_since = (q.ends_at - 5000) as f64;
    let urgent_age = c.age(Some(urgent_since.max(c.since.question.unwrap_or(f64::MIN))));
    if let (true, Some(age)) = (cd.urgent && !game.closing, urgent_age) {
        let k = motion::pulse(age);
        c.animate(timer, None, Some(Xform::new(0.0, 0.0, 0.0, k, k)));
    }
    let right = c.b.div(
        "div",
        S::row().shrink0().items(AlignItems::CENTER).gap(8.0),
        Visual::default(),
        &[toggle, answered, timer],
    );
    let header = c.b.div(
        "div",
        S::row()
            .shrink0()
            .items(AlignItems::CENTER)
            .justify(JustifyContent::SPACE_BETWEEN)
            .gap(8.0),
        Visual::default(),
        &[left, right],
    );

    let ranking_live = !game.ranking.is_empty();
    let standings_lines: Vec<ScoreLine> = if ranking_live {
        game.ranking.clone()
    } else {
        game.standings.clone()
    }
    .into_iter()
    .take(10)
    .collect();
    let options: Vec<String> = q.options.clone();
    let tiles: Vec<TileSpec> = options
        .iter()
        .enumerate()
        .map(|(i, text)| TileSpec {
            index: i,
            text: text.clone(),
            count: game.counts.get(i).copied().unwrap_or(0),
            fraction: game.counts.get(i).copied().unwrap_or(0) as f32
                / game.counts.iter().copied().max().unwrap_or(0).max(1) as f32,
            state: TileState::Idle,
        })
        .collect();
    let side = Some(standings(
        c,
        &standings_lines,
        ranking_live,
        &[],
        c.age(c.since.ranking),
    ));
    let region = region(c, Some(&q.text), &tiles, side, &[], &options);
    let glow_opacity = urgent_age.map_or(0.5, motion::edge);
    let timer_glow = (cd.urgent && !game.closing).then(|| {
        c.b.div(
            "div",
            S::block().abs(Some(0.0), Some(0.0), Some(0.0), Some(0.0)),
            Visual {
                glow: Some((90.0, 12.0, Rgba::new(239, 68, 68, 0.45))),
                opacity: Some(glow_opacity),
                z: 30,
                from_root: true,
                ..Visual::default()
            },
            &[],
        )
    });
    let mut kids: Vec<NodeId> = Vec::new();
    // "Question 2 of 5" scales in over the stage and fades out (650 ms), above everything.
    if let Some((opacity, scale)) = c.age(c.since.question).and_then(motion::splash) {
        let text = c.t(
            "quiz.questionOf",
            &[("n", q.q.to_string()), ("total", q.count.to_string())],
        );
        let card = c.b.text(
            "div",
            S::block().pad(32.0, 20.0, 32.0, 20.0),
            Visual {
                drop_shadow: Some((0.0, 25.0, 50.0, -12.0, Rgba::BLACK.with_alpha(0.25))),
                opacity: Some(opacity),
                xform: Some(Xform::new(0.0, 0.0, 0.0, scale, scale)),
                ..Visual::default().bg(tokens::PRIMARY).radius(22.0)
            },
            spec(text, (48.0, 48.0), EXTRABOLD, tokens::PRIMARY_FOREGROUND),
        );
        kids.push(
            c.b.div(
                "div",
                S::row()
                    .abs(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .items(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER),
                Visual {
                    z: 20,
                    from_root: true,
                    ..Visual::default()
                },
                &[card],
            ),
        );
    }
    kids.extend(timer_glow);
    kids.extend([header, region]);
    c.b.div(
        "div",
        S::col().min0().flex1().gap(12.0),
        Visual::default(),
        &kids,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileState {
    Idle,
    Correct,
    Wrong,
}

struct TileSpec {
    index: usize,
    text: String,
    count: i64,
    fraction: f32,
    state: TileState,
}

/// The scrolling region both the open question and the reveal have: the question, the option tiles (and
/// whatever follows them) in the left column, the standings in the right.
fn region(
    c: &mut Cx,
    heading: Option<&str>,
    tiles: &[TileSpec],
    side: Option<NodeId>,
    after: &[NodeId],
    options: &[String],
) -> NodeId {
    let mut column = Vec::new();
    if let Some(text) = heading {
        // `cn` (tailwind-merge) drops `leading-tight` for the size class that follows it: the step's own line height.
        column.push(
            c.b.text(
                "h2",
                S::block(),
                Visual::default(),
                spec(
                    text,
                    step(question_size(js_len(text))),
                    EXTRABOLD,
                    tokens::FOREGROUND,
                )
                .balanced(),
            ),
        );
    }
    let longest = options.iter().map(|o| js_len(o)).max().unwrap_or(0);
    // The grid's width in the page: the left column of a 1240 px region (956 px).
    let columns = tile_columns(956.0, longest);
    let cells: Vec<NodeId> = tiles.iter().map(|t| tile(c, t)).collect();
    let cells: Vec<NodeId> = cells
        .into_iter()
        .map(|t| c.b.div("li", S::row().min0(), Visual::default(), &[t]))
        .collect();
    let mut grid = S::grid().min0().gap(12.0).cols(
        (0..columns)
            .map(|_| fr1())
            .map(GridTemplateComponent::Single)
            .collect(),
    );
    grid.0.grid_auto_rows = vec![fr1()];
    let grid = c.b.div("ul", grid, Visual::default(), &cells);
    let mut tail = vec![grid];
    tail.extend_from_slice(after);
    column.extend(tail);
    space_y(c, &column, 12.0);
    let left =
        c.b.div("div", S::block().min0(), Visual::default(), &column);
    let mut kids = vec![left];
    if let Some(side) = side {
        // `[@media(max-height:460px)]:hidden`: built always, hidden on a very short stage.
        let wrapper = if c.b.env.max_h(460) {
            S::none()
        } else {
            S::block()
        };
        kids.push(c.b.div("div", wrapper.min0(), Visual::default(), &[side]));
    }
    // The host's grid keeps its 17rem column (`@2xl`) whether or not the standings are in it.
    let tracks = vec![fr1(), fixed(272.0)];
    let wide = c.b.div(
        "div",
        S::grid().min0().gap(12.0).cols(cols(tracks)).align_start(),
        Visual::default(),
        &kids,
    );
    c.b.div(
        "div",
        S::block().min0().flex1(),
        Visual::default().scroll(),
        &[wide],
    )
}

/// One answer as a tile (`option-tile.svelte`, the host's: no button).
fn tile(c: &mut Cx, t: &TileSpec) -> NodeId {
    let compact = c.b.env.max_h(700);
    let (gap, pad, min_h) = if compact {
        (6.0, 6.0, 48.0)
    } else if c.b.env.min_h(700) {
        (12.0, 12.0, 96.0)
    } else {
        (12.0, 12.0, 80.0)
    };
    let white = Rgba::WHITE;
    let mut kids = Vec::new();
    // The bar that grows with the count, from the left edge.
    kids.push(c.b.div(
        "span",
        S::block().abs(Some(0.0), Some(0.0), Some(0.0), Some(0.0)),
        Visual {
            scale_x: Some(t.fraction),
            ..Visual::default().bg(Rgba::BLACK.with_alpha(0.2))
        },
        &[],
    ));
    let age = c.age(c.since.reveal);
    if let (TileState::Correct, Some(a)) = (t.state, age) {
        // `qz-shine`: a bright slanted band crossing the tile once, after the right answers light up.
        let mut shine = S::block().abs(Some(0.0), Some(0.0), None, Some(0.0));
        shine.0.size.width = taffy::prelude::percent(0.25);
        let band = c.b.div(
            "span",
            shine,
            Visual::default().bg(Rgba::WHITE.with_alpha(0.35)),
            &[],
        );
        c.animate(
            band,
            Some(1.0),
            Some(Xform {
                tx_w: motion::shine(a),
                skew_x: -12.0,
                ..Xform::new(0.0, 0.0, 0.0, 1.0, 1.0)
            }),
        );
        kids.push(band);
    }
    let shapes = ["triangle", "diamond", "circle", "square", "star", "hexagon"];
    let icon = c.b.node(
        S::block().size(if compact { 20.0 } else { 28.0 }),
        Visual {
            tag: "svg",
            ..Default::default()
        },
        Content::Art {
            art: c.lib.icon(shapes[t.index % 6]),
            tint: white,
            fill: Some(white.with_alpha(0.9)),
            stroke_width: Some(2.5),
        },
        &[],
    );
    let key = c.b.text(
        "span",
        S::block(),
        Visual::default(),
        spec(
            (t.index + 1).to_string(),
            (11.0, 11.0),
            BOLD,
            white.with_alpha(0.9),
        ),
    );
    kids.push(
        c.b.div(
            "span",
            S::col()
                .shrink0()
                .items(AlignItems::CENTER)
                .gap(2.0)
                .pad(
                    6.0,
                    if compact { 2.0 } else { 4.0 },
                    6.0,
                    if compact { 2.0 } else { 4.0 },
                )
                .relative(),
            Visual::default().bg(white.with_alpha(0.15)).radius(R_XL),
            &[icon, key],
        ),
    );
    kids.push(c.b.text(
        "span",
        S::block().min0().flex1().relative(),
        Visual::default(),
        spec(
            t.text.clone(),
            step(option_size(js_len(&t.text))),
            BOLD,
            white,
        ),
    ));
    // `min-w-[3ch]`: three digits of the count's face.
    let three =
        c.b.text
            .intrinsic(&spec("000", XL3, EXTRABOLD, white).tab())
            .0;
    let mut count = S::block().shrink0().relative();
    count.0.min_size.width = taffy::prelude::length(three);
    kids.push(
        c.b.text(
            "span",
            count,
            Visual::default(),
            spec(t.count.to_string(), XL3, EXTRABOLD, white)
                .tab()
                .right(),
        ),
    );
    let slot = if compact { 24.0 } else { 28.0 };
    let correct = t.state == TileState::Correct;
    let check = c.b.node(
        S::block().size(16.0),
        Visual {
            tag: "svg",
            ..Default::default()
        },
        Content::Art {
            art: c.lib.icon("check"),
            tint: if correct { white } else { Rgba::BLACK },
            fill: None,
            stroke_width: Some(3.5),
        },
        &[],
    );
    let mut badge = Visual::default()
        .bg(if correct { tokens::GREEN_500 } else { white })
        .radius(9999.0);
    if correct {
        badge = badge.ring(2.0, white);
        if let Some(a) = age {
            // `qz-pop` on the check.
            let (o, k) = motion::pop(a);
            badge.opacity = Some(o);
            badge.xform = Some(Xform::new(0.0, 0.0, 0.0, k, k));
        }
    } else {
        badge.opacity = Some(0.0);
    }
    kids.push(
        c.b.div(
            "span",
            S::row()
                .size(slot)
                .shrink0()
                .items(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .relative(),
            badge,
            &[check],
        ),
    );
    let (from, to) = tile_colours(t.index);
    let mut vis = Visual::default().radius(R_2XL).clipped();
    vis.bg = Some(crate::ui::Fill::ToBottomRight(from, to));
    vis.tile_overlay = true;
    match t.state {
        TileState::Idle => vis.inset_ring = Some((1.0, white.with_alpha(0.25))),
        TileState::Correct => vis.inset_ring = Some((4.0, white)),
        TileState::Wrong => vis.grayscale = true,
    }
    let mut style = S::row()
        .w_full()
        .min0()
        .items(AlignItems::CENTER)
        .gap(gap)
        .p(pad)
        .relative()
        .clip();
    style.0.min_size.height = taffy::prelude::length(min_h);
    c.b.div("div", style, vis, &kids)
}

// ───────────────────────── standings ─────────────────────────

/// A key per line that stays the same for the same name (the nth of equal names counts apart).
fn keys(list: &[ScoreLine]) -> Vec<String> {
    list.iter()
        .enumerate()
        .map(|(i, l)| {
            format!(
                "{}#{}",
                l.name,
                list[..i].iter().filter(|x| x.name == l.name).count()
            )
        })
        .collect()
}

/// The standings (`standings.svelte`): `from` is where the rows stood before (they slide from there and count up
/// from their old score), `age` how long ago they appeared; without motion they are simply in place.
fn standings(
    c: &mut Cx,
    lines: &[ScoreLine],
    live: bool,
    from: &[ScoreLine],
    age: Option<f32>,
) -> NodeId {
    let title = c.t(
        if live {
            "quiz.standings.live"
        } else {
            "quiz.topFive"
        },
        &[],
    );
    let mut head = vec![c.b.text(
        "span",
        S::block(),
        Visual::default(),
        spec(title, SM, BOLD, tokens::MUTED_FOREGROUND),
    )];
    if live {
        let pulse = c.b.div(
            "span",
            S::block().size(10.0).abs(Some(0.0), Some(0.0), None, None),
            Visual::default().bg(tokens::RED_500).radius(9999.0),
            &[],
        );
        if let Some(a) = c.age(c.since.ranking) {
            let k = motion::pulse(a);
            c.animate(pulse, None, Some(Xform::new(0.0, 0.0, 0.0, k, k)));
        }
        let dot = c.b.div(
            "span",
            S::block().size(10.0).relative(),
            Visual::default().bg(tokens::RED_500).radius(9999.0),
            &[],
        );
        head.push(c.b.div(
            "span",
            S::block().size(10.0).relative(),
            Visual::default(),
            &[pulse, dot],
        ));
    }
    let h3 = c.b.div(
        "h3",
        S::row().items(AlignItems::CENTER).gap(8.0),
        Visual::default(),
        &head,
    );
    let (now_keys, was_keys) = (keys(lines), keys(from));
    let mut rows = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let was = was_keys.iter().position(|k| *k == now_keys[i]);
        let start_index = was.unwrap_or(i);
        let start_score = was.map_or(0, |w| from[w].score);
        // A row that moved is lit for 1.4 s (`qz-mover`). That animation's `transform` replaces the row's own
        // `translateY` in the page, so while it runs the row sits at the top of the list, scaled; the slide it
        // was meant to make never shows. After it, the row is at its place.
        let (shown, lit) = match age {
            Some(a) => (
                motion::count_up(a, start_score, line.score),
                (start_index != i).then(|| motion::mover(a)).flatten(),
            ),
            None => (line.score, None),
        };
        let place = if lit.is_some() { 0.0 } else { i as f32 };
        let rank = c.b.text(
            "span",
            S::row().size(28.0).shrink0(),
            Visual::default().bg(tokens::MUTED).radius(R_LG).vcenter(),
            spec((i + 1).to_string(), SM, EXTRABOLD, tokens::FOREGROUND)
                .tab()
                .center(),
        );
        let avatar = avatar_disc(c, &line.name, 28.0, BASE);
        let name = c.b.text(
            "span",
            S::block().min0().flex1(),
            Visual::default(),
            spec(line.name.clone(), SM, SEMIBOLD, tokens::FOREGROUND).one_line(),
        );
        let mut kids = vec![rank, avatar, name];
        if lit.is_some() {
            let up = i < start_index;
            let (icon, tone) = if up {
                ("arrow-up", tokens::GREEN_800)
            } else {
                ("arrow-down", tokens::RED_700)
            };
            let arrow = c.b.node(
                S::block().size(16.0),
                Visual {
                    tag: "svg",
                    ..Default::default()
                },
                Content::Art {
                    art: c.lib.icon(icon),
                    tint: tone,
                    fill: None,
                    stroke_width: Some(3.0),
                },
                &[],
            );
            let mut slot = S::block().w(16.0).h(20.0).pad(0.0, 2.0, 0.0, 0.0);
            slot.0.flex_shrink = 0.0;
            kids.push(c.b.div("span", slot, Visual::default(), &[arrow]));
        }
        kids.push(
            c.b.text(
                "span",
                S::block().w(64.0).shrink0(),
                Visual::default(),
                spec(shown.to_string(), BASE, EXTRABOLD, tokens::FOREGROUND)
                    .tab()
                    .right(),
            ),
        );
        let border = if lit.is_some() {
            tokens::PRIMARY
        } else {
            tokens::BORDER
        };
        let row = c.b.div(
            "li",
            S::row()
                .abs(Some(0.0), Some(place * 48.0), Some(0.0), None)
                .h(40.0)
                .items(AlignItems::CENTER)
                .gap(12.0)
                .pad(12.0, 0.0, 12.0, 0.0)
                .border(1.0),
            Visual::default()
                .bg(tokens::CARD)
                .border(1.0, border)
                .radius(R_XL),
            &kids,
        );
        if let Some((o, k)) = lit {
            c.animate(row, Some(o), Some(Xform::new(0.0, 0.0, 0.0, k, k)));
        }
        rows.push(row);
    }
    let list = c.b.div(
        "ol",
        S::block().h(lines.len().max(1) as f32 * 48.0).relative(),
        Visual::default(),
        &rows,
    );
    let mut kids = vec![h3, list];
    if lines.is_empty() {
        let text = c.t("quiz.standings.empty", &[]);
        kids.push(c.b.text(
            "p",
            S::block(),
            Visual::default(),
            spec(text, SM, 400, tokens::MUTED_FOREGROUND),
        ));
    }
    // `space-y-2`: 8 px under every child but the last.
    space_y(c, &kids, 8.0);
    c.b.div("section", S::block().min0(), Visual::default(), &kids)
}

// ───────────────────────── reveal ─────────────────────────

fn reveal(c: &mut Cx, game: &QuizGame) -> NodeId {
    let reveal: &Reveal = game.reveal.as_ref().expect("a reveal phase has a reveal");
    let options: Vec<String> = match &game.question {
        Some(q) => q.options.clone(),
        None => reveal.counts.iter().map(|_| String::new()).collect(),
    };
    let biggest = reveal.counts.iter().copied().max().unwrap_or(0).max(1);
    let tiles: Vec<TileSpec> = options
        .iter()
        .enumerate()
        .map(|(i, text)| TileSpec {
            index: i,
            text: text.clone(),
            count: reveal.counts.get(i).copied().unwrap_or(0),
            fraction: reveal.counts.get(i).copied().unwrap_or(0) as f32 / biggest as f32,
            state: if (reveal.correct as i32) & (1i32 << (i % 32)) != 0 {
                TileState::Correct
            } else {
                TileState::Wrong
            },
        })
        .collect();
    let mut after = Vec::new();
    if let Some(text) = reveal.explanation.as_deref().filter(|t| !t.is_empty()) {
        let label = c.t("quiz.explanation", &[]);
        let mut s = spec(label, BASE, BOLD, tokens::FOREGROUND);
        s.spans.push(crate::text::Span {
            text: format!(" {text}"),
            weight: 400,
            color: tokens::FOREGROUND,
        });
        let node = c.b.text(
            "p",
            S::block().p(12.0).border(1.0),
            Visual::default()
                .bg(tokens::MUTED_60)
                .border(1.0, tokens::BORDER)
                .radius(R_2XL),
            s,
        );
        if let Some(a) = c.age(c.since.reveal) {
            let (o, ty) = motion::slide(a, 250.0);
            c.animate(node, Some(o), Some(Xform::new(0.0, ty, 0.0, 1.0, 1.0)));
        }
        after.push(node);
    }
    let gap = if reveal.top.len() >= 2 {
        reveal.top[0].score.saturating_sub(reveal.top[1].score)
    } else {
        i64::MAX
    };
    if gap <= 100 {
        let swords = c.b.node(
            S::block().size(24.0).shrink0(),
            Visual {
                tag: "svg",
                ..Default::default()
            },
            Content::Art {
                art: c.lib.icon("swords"),
                tint: tokens::AMBER_950,
                fill: None,
                stroke_width: None,
            },
            &[],
        );
        let text = c.t("quiz.closeRace", &[("points", gap.to_string())]);
        let label = c.b.text(
            "span",
            S::block(),
            Visual::default(),
            spec(text, BASE, BOLD, tokens::AMBER_950),
        );
        let banner = c.b.div(
            "p",
            S::row().items(AlignItems::CENTER).gap(8.0).p(12.0),
            Visual::default().bg(tokens::AMBER_100).radius(R_2XL),
            &[swords, label],
        );
        if let Some(a) = c.age(c.since.reveal) {
            let (o, ty) = motion::slide(a, 450.0);
            c.animate(banner, Some(o), Some(Xform::new(0.0, ty, 0.0, 1.0, 1.0)));
        }
        after.push(banner);
    }
    // Before this reveal the rows stood where the live ranking left them, or where the reveal before put them.
    let earlier: &[ScoreLine] = if game.ranking.is_empty() {
        &game.prev_top
    } else {
        &game.ranking
    };
    let side = (!reveal.top.is_empty())
        .then(|| standings(c, &reveal.top, false, earlier, c.age(c.since.reveal)));
    let heading = game.question.as_ref().map(|q| q.text.clone());
    let body = region(c, heading.as_deref(), &tiles, side, &after, &options);
    c.b.div(
        "div",
        S::col().min0().flex1().gap(12.0),
        Visual::default(),
        &[body],
    )
}
