//! The race's stage of a recording (`stages/race-replay-stage.svelte`, with the host's lobby, question and
//! reveal screens of `quiz/components/`): what the teacher saw, watching only. The player's own parts
//! (answering, their rank) are not on a recording's screen.

use replay_model::quiz::QuizReplay;
use replay_model::quiz::fit::{Text, js_len, option_size, question_size, tile_columns};
use replay_model::quiz::format::countdown;
use replay_model::quiz::protocol::ScoreLine;
use replay_model::quiz::race::{Phase, QuizGame, Reveal};
use taffy::prelude::{AlignContent, AlignItems, GridTemplateComponent, JustifyContent, NodeId, TrackSizingFunction};
use taffy::style::{MaxTrackSizingFunction, MinTrackSizingFunction};

use crate::color::Rgba;
use crate::kit::{
    BASE, BLACK, BOLD, Cx, EXTRABOLD, LG, R_2XL, R_LG, R_XL, SEMIBOLD, SM, XL, XL3, XS, avatar_disc, ring,
    ring_text, spec, tile_colours,
};
use crate::podium::{podium, race_flavour};
use crate::tokens;
use crate::ui::{Content, S, Visual};

pub fn body(c: &mut Cx, replay: &QuizReplay, now: f64, _w: f32, _h: f32) -> NodeId {
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
        Visual { tag: "svg", ..Default::default() },
        Content::Art { art: c.lib.icon("users"), tint: tokens::PRIMARY, fill: None, stroke_width: None },
        &[],
    );
    let count = c.b.text(
        "p",
        S::block().shrink0(),
        Visual::default(),
        spec(game.players.to_string(), (96.0, 96.0), BLACK, tokens::FOREGROUND).tab().center(),
    );
    let line = format!(
        "{} · {}",
        c.t("quiz.host.joined", &[("count", game.players.to_string())]),
        c.t("quiz.host.questions", &[("count", game.question_count.to_string())])
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
        kids.push(c.b.text("p", S::block().shrink0(), Visual::default(), spec(text, SM, 400, tokens::MUTED_FOREGROUND).center()));
    } else {
        let mut chips = Vec::new();
        for name in names {
            let avatar = avatar_disc(c, name, 36.0, XL);
            let label = c.b.text(
                "span",
                S::block().max_w(128.0).min0(),
                Visual::default(),
                spec(name.clone(), SM, BOLD, tokens::FOREGROUND).one_line(),
            );
            chips.push(c.b.div(
                "li",
                S::row().max_w_pct(1.0).min0().items(AlignItems::CENTER).gap(8.0).pad(6.0, 4.0, 12.0, 4.0).border(1.0),
                Visual::default().bg(tokens::CARD).border(1.0, tokens::BORDER).radius(9999.0),
                &[avatar, label],
            ));
        }
        if game.joined.len() > SHOWN {
            let more = c.t("quiz.lobby.more", &[("n", (game.joined.len() - SHOWN).to_string())]);
            chips.push(c.b.text(
                "li",
                S::block().pad(8.0, 0.0, 8.0, 0.0),
                Visual::default(),
                spec(more, SM, SEMIBOLD, tokens::MUTED_FOREGROUND),
            ));
        }
        kids.push(c.b.div(
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
        ));
    }
    c.b.div("div", S::col().min0().flex1().items(AlignItems::CENTER).gap(16.0), Visual::default(), &kids)
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
    TrackSizingFunction { min: MinTrackSizingFunction::length(0.0), max: MaxTrackSizingFunction::fr(1.0) }
}

fn fixed(px: f32) -> TrackSizingFunction {
    TrackSizingFunction { min: MinTrackSizingFunction::length(px), max: MaxTrackSizingFunction::length(px) }
}

fn cols(tracks: Vec<TrackSizingFunction>) -> Vec<GridTemplateComponent<String>> {
    tracks.into_iter().map(GridTemplateComponent::Single).collect()
}

fn question(c: &mut Cx, game: &QuizGame, now: f64) -> NodeId {
    let q = game.question.as_ref().expect("a question phase has a question");
    let cd = countdown(q.ends_at as f64, q.limit_ms as f64, now);
    let ring_px = if c.b.env.min_h(700) { 64.0 } else { 56.0 };

    // The header: question number, the answered ring and the countdown.
    let number = c.t("quiz.questionOf", &[("n", q.q.to_string()), ("total", q.count.to_string())]);
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
        let label = c.b.text("span", S::block(), Visual::default(), spec(text, SM, BOLD, tokens::MUTED_FOREGROUND));
        let dots: Vec<NodeId> = (0..3)
            .map(|_| c.b.div("span", S::block().size(6.0), Visual::default().bg(tokens::MUTED_FOREGROUND).radius(9999.0), &[]))
            .collect();
        let dots = c.b.div("span", S::row().gap(4.0), Visual::default(), &dots);
        left.push(c.b.div(
            "span",
            S::row().items(AlignItems::CENTER).gap(8.0).pad(12.0, 4.0, 12.0, 4.0),
            Visual::default().bg(tokens::MUTED).radius(9999.0),
            &[label, dots],
        ));
    }
    let left = c.b.div("div", S::row().min0().wrap().items(AlignItems::CENTER).gap(6.0), Visual::default(), &left);

    let toggle = c.b.div("button", S::block().size(44.0).shrink0(), Visual::default().hidden(), &[]);
    let fraction = if game.players == 0 { 0.0 } else { game.answered as f32 / game.players as f32 };
    let answered_text = ring_text(c, &format!("{}/{}", game.answered, game.players), SM);
    let answered = ring(c, ring_px, fraction, tokens::GREEN_600, answered_text);
    let (shown, f) = if game.closing { (0, 0.0) } else { (cd.seconds, cd.fraction as f32) };
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
    let right = c.b.div("div", S::row().shrink0().items(AlignItems::CENTER).gap(8.0), Visual::default(), &[toggle, answered, timer]);
    let header = c.b.div(
        "div",
        S::row().shrink0().items(AlignItems::CENTER).justify(JustifyContent::SPACE_BETWEEN).gap(8.0),
        Visual::default(),
        &[left, right],
    );

    let ranking_live = !game.ranking.is_empty();
    let standings_lines: Vec<ScoreLine> =
        if ranking_live { game.ranking.clone() } else { game.standings.clone() }.into_iter().take(10).collect();
    let options: Vec<String> = q.options.clone();
    let tiles: Vec<TileSpec> = options
        .iter()
        .enumerate()
        .map(|(i, text)| TileSpec {
            index: i,
            text: text.clone(),
            count: game.counts.get(i).copied().unwrap_or(0),
            fraction: game.counts.get(i).copied().unwrap_or(0) as f32 / game.counts.iter().copied().max().unwrap_or(0).max(1) as f32,
            state: TileState::Idle,
        })
        .collect();
    let side = (!c.b.env.max_h(460)).then(|| standings(c, &standings_lines, ranking_live));
    let region = region(c, Some(&q.text), &tiles, side, &[], &options);
    let timer_glow = (cd.urgent && !game.closing).then(|| {
        c.b.div(
            "div",
            S::block().abs(Some(0.0), Some(0.0), Some(0.0), Some(0.0)),
            Visual {
                glow: Some((90.0, 12.0, Rgba::new(239, 68, 68, 0.45))),
                opacity: Some(0.5),
                z: 30,
                from_root: true,
                ..Visual::default()
            },
            &[],
        )
    });
    let mut kids: Vec<NodeId> = timer_glow.into_iter().collect();
    kids.extend([header, region]);
    c.b.div("div", S::col().min0().flex1().gap(12.0), Visual::default(), &kids)
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
        column.push(c.b.text(
            "h2",
            S::block(),
            Visual::default(),
            spec(text, step(question_size(js_len(text))), EXTRABOLD, tokens::FOREGROUND).balanced(),
        ));
    }
    let longest = options.iter().map(|o| js_len(o)).max().unwrap_or(0);
    // The grid's width in the page: the left column of a 1240 px region (956 px).
    let columns = tile_columns(956.0, longest);
    let cells: Vec<NodeId> = tiles.iter().map(|t| tile(c, t)).collect();
    let cells: Vec<NodeId> = cells
        .into_iter()
        .map(|t| c.b.div("li", S::row().min0(), Visual::default(), &[t]))
        .collect();
    let mut grid = S::grid().min0().gap(12.0).cols((0..columns).map(|_| fr1()).map(GridTemplateComponent::Single).collect());
    grid.0.grid_auto_rows = vec![fr1()];
    let grid = c.b.div("ul", grid, Visual::default(), &cells);
    let mut tail = vec![grid];
    tail.extend_from_slice(after);
    column.extend(tail);
    space_y(c, &column, 12.0);
    let left = c.b.div("div", S::block().min0(), Visual::default(), &column);
    let mut kids = vec![left];
    if let Some(side) = side {
        kids.push(c.b.div("div", S::block().min0(), Visual::default(), &[side]));
    }
    let tracks = if kids.len() == 2 { vec![fr1(), fixed(272.0)] } else { vec![fr1()] };
    let wide = c.b.div("div", S::grid().min0().gap(12.0).cols(cols(tracks)).align_start(), Visual::default(), &kids);
    c.b.div("div", S::block().min0().flex1(), Visual::default().scroll(), &[wide])
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
        Visual { scale_x: Some(t.fraction), ..Visual::default().bg(Rgba::BLACK.with_alpha(0.2)) },
        &[],
    ));
    let shapes = ["triangle", "diamond", "circle", "square", "star", "hexagon"];
    let icon = c.b.node(
        S::block().size(if compact { 20.0 } else { 28.0 }),
        Visual { tag: "svg", ..Default::default() },
        Content::Art { art: c.lib.icon(shapes[t.index % 6]), tint: white, fill: Some(white.with_alpha(0.9)), stroke_width: Some(2.5) },
        &[],
    );
    let key = c.b.text(
        "span",
        S::block(),
        Visual::default(),
        spec((t.index + 1).to_string(), (11.0, 11.0), BOLD, white.with_alpha(0.9)),
    );
    kids.push(c.b.div(
        "span",
        S::col().shrink0().items(AlignItems::CENTER).gap(2.0).pad(6.0, if compact { 2.0 } else { 4.0 }, 6.0, if compact { 2.0 } else { 4.0 }).relative(),
        Visual::default().bg(white.with_alpha(0.15)).radius(R_XL),
        &[icon, key],
    ));
    kids.push(c.b.text(
        "span",
        S::block().min0().flex1().relative(),
        Visual::default(),
        spec(t.text.clone(), step(option_size(js_len(&t.text))), BOLD, white),
    ));
    // `min-w-[3ch]`: three digits of the count's face.
    let three = c.b.text.intrinsic(&spec("000", XL3, EXTRABOLD, white).tab()).0;
    let mut count = S::block().shrink0().relative();
    count.0.min_size.width = taffy::prelude::length(three);
    kids.push(c.b.text("span", count, Visual::default(), spec(t.count.to_string(), XL3, EXTRABOLD, white).tab().right()));
    let slot = if compact { 24.0 } else { 28.0 };
    let correct = t.state == TileState::Correct;
    let check = c.b.node(
        S::block().size(16.0),
        Visual { tag: "svg", ..Default::default() },
        Content::Art { art: c.lib.icon("check"), tint: if correct { white } else { Rgba::BLACK }, fill: None, stroke_width: Some(3.5) },
        &[],
    );
    let mut badge = Visual::default().bg(if correct { tokens::GREEN_500 } else { white }).radius(9999.0);
    if correct {
        badge = badge.ring(2.0, white);
    } else {
        badge.opacity = Some(0.0);
    }
    kids.push(c.b.div(
        "span",
        S::row().size(slot).shrink0().items(AlignItems::CENTER).justify(JustifyContent::CENTER).relative(),
        badge,
        &[check],
    ));
    let (from, to) = tile_colours(t.index);
    let mut vis = Visual::default().radius(R_2XL).clipped();
    vis.bg = Some(crate::ui::Fill::ToBottomRight(from, to));
    vis.tile_overlay = true;
    match t.state {
        TileState::Idle => vis.inset_ring = Some((1.0, white.with_alpha(0.25))),
        TileState::Correct => vis.inset_ring = Some((4.0, white)),
        TileState::Wrong => vis.grayscale = true,
    }
    let mut style = S::row().w_full().min0().items(AlignItems::CENTER).gap(gap).p(pad).relative().clip();
    style.0.min_size.height = taffy::prelude::length(min_h);
    c.b.div("div", style, vis, &kids)
}

// ───────────────────────── standings ─────────────────────────

fn standings(c: &mut Cx, lines: &[ScoreLine], live: bool) -> NodeId {
    let title = c.t(if live { "quiz.standings.live" } else { "quiz.topFive" }, &[]);
    let mut head = vec![c.b.text("span", S::block(), Visual::default(), spec(title, SM, BOLD, tokens::MUTED_FOREGROUND))];
    if live {
        let pulse = c.b.div("span", S::block().size(10.0).abs(Some(0.0), Some(0.0), None, None), Visual::default().bg(tokens::RED_500).radius(9999.0), &[]);
        let dot = c.b.div("span", S::block().size(10.0).relative(), Visual::default().bg(tokens::RED_500).radius(9999.0), &[]);
        head.push(c.b.div("span", S::block().size(10.0).relative(), Visual::default(), &[pulse, dot]));
    }
    let h3 = c.b.div("h3", S::row().items(AlignItems::CENTER).gap(8.0), Visual::default(), &head);
    let mut rows = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let rank = c.b.text(
            "span",
            S::row().size(28.0).shrink0(),
            Visual::default().bg(tokens::MUTED).radius(R_LG).vcenter(),
            spec((i + 1).to_string(), SM, EXTRABOLD, tokens::FOREGROUND).tab().center(),
        );
        let avatar = avatar_disc(c, &line.name, 28.0, BASE);
        let name = c.b.text(
            "span",
            S::block().min0().flex1(),
            Visual::default(),
            spec(line.name.clone(), SM, SEMIBOLD, tokens::FOREGROUND).one_line(),
        );
        let score = c.b.text(
            "span",
            S::block().w(64.0).shrink0(),
            Visual::default(),
            spec(line.score.to_string(), BASE, EXTRABOLD, tokens::FOREGROUND).tab().right(),
        );
        rows.push(c.b.div(
            "li",
            S::row().abs(Some(0.0), Some(i as f32 * 48.0), Some(0.0), None).h(40.0).items(AlignItems::CENTER).gap(12.0).pad(12.0, 0.0, 12.0, 0.0).border(1.0),
            Visual::default().bg(tokens::CARD).border(1.0, tokens::BORDER).radius(R_XL),
            &[rank, avatar, name, score],
        ));
    }
    let list = c.b.div("ol", S::block().h(lines.len().max(1) as f32 * 48.0).relative(), Visual::default(), &rows);
    let mut kids = vec![h3, list];
    // `space-y-2`: 8 px under every child but the last.
    if lines.is_empty() {
        let text = c.t("quiz.standings.empty", &[]);
        kids.push(c.b.text("p", S::block(), Visual::default(), spec(text, SM, 400, tokens::MUTED_FOREGROUND)));
    }
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
            state: if reveal.correct & (1 << i) != 0 { TileState::Correct } else { TileState::Wrong },
        })
        .collect();
    let mut after = Vec::new();
    if let Some(text) = reveal.explanation.as_deref().filter(|t| !t.is_empty()) {
        let label = c.t("quiz.explanation", &[]);
        let mut s = spec(label, BASE, BOLD, tokens::FOREGROUND);
        s.spans.push(crate::text::Span { text: format!(" {text}"), weight: 400, color: tokens::FOREGROUND });
        after.push(c.b.text(
            "p",
            S::block().p(12.0).border(1.0),
            Visual::default().bg(tokens::MUTED_60).border(1.0, tokens::BORDER).radius(R_2XL),
            s,
        ));
    }
    let gap = if reveal.top.len() >= 2 { reveal.top[0].score - reveal.top[1].score } else { i64::MAX };
    if gap <= 100 {
        let swords = c.b.node(
            S::block().size(24.0).shrink0(),
            Visual { tag: "svg", ..Default::default() },
            Content::Art { art: c.lib.icon("swords"), tint: tokens::AMBER_950, fill: None, stroke_width: None },
            &[],
        );
        let text = c.t("quiz.closeRace", &[("points", gap.to_string())]);
        let label = c.b.text("span", S::block(), Visual::default(), spec(text, BASE, BOLD, tokens::AMBER_950));
        after.push(c.b.div(
            "p",
            S::row().items(AlignItems::CENTER).gap(8.0).p(12.0),
            Visual::default().bg(tokens::AMBER_100).radius(R_2XL),
            &[swords, label],
        ));
    }
    let side = (!reveal.top.is_empty() && !c.b.env.max_h(460)).then(|| standings(c, &reveal.top, false));
    let heading = game.question.as_ref().map(|q| q.text.clone());
    let body = region(c, heading.as_deref(), &tiles, side, &after, &options);
    c.b.div("div", S::col().min0().flex1().gap(12.0), Visual::default(), &[body])
}
