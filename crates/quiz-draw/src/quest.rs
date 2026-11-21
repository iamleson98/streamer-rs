//! The Quest stage of a recording (`stages/quest-replay-stage.svelte`, with `quest/components/quest-board.svelte`):
//! the lobby's head count, the board while the game runs, and the podium.

use replay_model::quiz::avatar::fallback_character;
use replay_model::quiz::format::{accuracy, clock, countdown};
use replay_model::quiz::protocol::Row;
use replay_model::quiz::quest::{QuestGame, QuestPhase};
use taffy::prelude::{AlignItems, JustifyContent, NodeId};

use crate::kit::{
    BASE, BLACK, BOLD, Cx, LG, R_XL, SEMIBOLD, SM, character_disc, ring, ring_text, spec,
};
use crate::podium::{Flavour, podium};
use crate::tokens;
use crate::ui::{Content, S, Visual};

/// The row of the board: 44 px on the tall page, 36 px at the stage's own height
/// (`[--row-h:2.75rem] [@media(max-height:700px)]:[--row-h:2.25rem]`).
fn row_h(c: &Cx) -> f32 {
    if c.b.env.max_h(700) { 36.0 } else { 44.0 }
}

/// The body of the stage for a Quest game at `now`.
pub fn body(c: &mut Cx, game: &QuestGame, now: f64) -> NodeId {
    match game.phase {
        QuestPhase::Lobby => lobby(c, game),
        QuestPhase::Running => running(c, game, now),
        _ => match &game.podium {
            Some(p) => {
                let top: Vec<_> = p.top.iter().take(3).cloned().collect();
                let flavour = Flavour {
                    avatar: &|c, name, size, _| {
                        let id = game
                            .character_of(name)
                            .unwrap_or_else(|| fallback_character(name));
                        character_disc(c, id, if size >= 36.0 { 40.0 } else { size })
                    },
                    points: &|c, n| c.t("quest.podium.coins", &[("n", n.to_string())]),
                    bare_on_steps: false,
                };
                podium(c, &top, None, &flavour)
            }
            None => c.b.div("div", S::block(), Visual::default(), &[]),
        },
    }
}

fn lobby(c: &mut Cx, game: &QuestGame) -> NodeId {
    let count = c.b.text(
        "p",
        S::block(),
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
        S::block(),
        Visual::default(),
        spec(line, LG, SEMIBOLD, tokens::MUTED_FOREGROUND).center(),
    );
    c.b.div(
        "div",
        S::block().m_auto(),
        Visual::default(),
        &[count, caption],
    )
}

fn pill(c: &mut Cx, text: String) -> NodeId {
    c.b.text(
        "span",
        S::block().pad(12.0, 4.0, 12.0, 4.0).border(1.0),
        Visual::default()
            .bg(tokens::CARD)
            .border(1.0, tokens::BORDER)
            .radius(9999.0),
        spec(text, SM, BOLD, tokens::FOREGROUND).tab(),
    )
}

fn running(c: &mut Cx, game: &QuestGame, now: f64) -> NodeId {
    let cd = countdown(game.ends_at as f64, (game.seconds * 1000) as f64, now);
    let percent = accuracy(game.board.correct, game.board.answers);
    let players = if game.board.players != 0 {
        game.board.players
    } else {
        game.players
    };
    let pills = [
        format!("{}: {}", c.t("quest.host.players", &[]), players),
        format!("{}: {}", c.t("quest.host.answers", &[]), game.board.answers),
        format!(
            "{}: {}",
            c.t("quest.host.accuracy", &[]),
            percent.map_or_else(|| "-".to_owned(), |p| format!("{p}%"))
        ),
    ];
    let pills: Vec<NodeId> = pills.into_iter().map(|p| pill(c, p)).collect();
    let stats = c.b.div(
        "div",
        S::row().min0().wrap().items(AlignItems::CENTER).gap(6.0),
        Visual::default(),
        &pills,
    );
    let time = ring_text(c, &clock(cd.left), SM);
    let tone = if cd.left <= 30_000.0 {
        tokens::RED_700
    } else {
        tokens::PRIMARY
    };
    let timer = ring(c, 64.0, cd.fraction as f32, tone, time);
    let header = c.b.div(
        "div",
        S::row()
            .shrink0()
            .items(AlignItems::CENTER)
            .justify(JustifyContent::SPACE_BETWEEN)
            .gap(8.0),
        Visual::default(),
        &[stats, timer],
    );
    let list = if game.board.top.is_empty() {
        let text = c.t("quest.host.boardEmpty", &[]);
        c.b.text(
            "p",
            S::block().p(16.0),
            Visual::default(),
            spec(text, SM, 400, tokens::MUTED_FOREGROUND).center(),
        )
    } else {
        board(c, &game.board.top)
    };
    let region = c.b.div(
        "div",
        S::block().min0().flex1(),
        Visual::default().scroll(),
        &[list],
    );
    c.b.div(
        "div",
        S::col().min0().flex1().gap(12.0),
        Visual::default(),
        &[header, region],
    )
}

/// The live board (`quest-board.svelte`): rows placed by rank inside a box of fixed height, in the DOM order of
/// their keys.
fn board(c: &mut Cx, rows: &[Row]) -> NodeId {
    let h = row_h(c);
    let key_of = |seen: &mut std::collections::HashMap<String, usize>, name: &str| {
        let n = seen.entry(name.to_owned()).or_insert(0);
        *n += 1;
        if *n > 1 {
            format!("{name}#{n}")
        } else {
            name.to_owned()
        }
    };
    // Where each row stood on the board before this one, by key: it slides from there (650 ms).
    let mut before_seen = std::collections::HashMap::new();
    let before: Vec<String> = c
        .since
        .board_before
        .iter()
        .map(|r| key_of(&mut before_seen, &r.name))
        .collect();
    let before_coins: Vec<i64> = c.since.board_before.iter().map(|r| r.coins).collect();
    let age = c.age(c.since.board);
    let mut seen_now = std::collections::HashMap::new();
    let mut keyed: Vec<(String, usize, &Row)> = rows
        .iter()
        .enumerate()
        .map(|(rank, row)| (key_of(&mut seen_now, &row.name), rank, row))
        .collect();
    keyed.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    let mut items = Vec::new();
    for (key, rank, row) in keyed {
        let first = rank == 0;
        // A row that changed place slides there (`translateY` with a 650 ms transition) from `top: 0`.
        let moving = match (age, before.iter().position(|k| *k == key)) {
            (Some(a), Some(old)) if old != rank && a < 650.0 => {
                Some(crate::motion::quest_row_place(a, old as f32, rank as f32))
            }
            _ => None,
        };
        let shown = match (age, before.iter().position(|k| *k == key)) {
            (Some(a), Some(old)) => crate::motion::count_up(a, before_coins[old], row.coins),
            _ => row.coins,
        };
        let number = c.b.text(
            "span",
            S::block().w(24.0).shrink0(),
            Visual::default(),
            spec((rank + 1).to_string(), SM, BLACK, tokens::FOREGROUND)
                .tab()
                .center(),
        );
        let disc = character_disc(c, row.character, 32.0);
        let name = c.b.text(
            "span",
            S::block().min0().flex1(),
            Visual::default(),
            spec(row.name.clone(), BASE, BOLD, tokens::FOREGROUND).one_line(),
        );
        let coin = c.b.node(
            S::block().size(20.0),
            Visual {
                tag: "svg",
                ..Default::default()
            },
            Content::Art {
                art: c.lib.coin.clone(),
                tint: tokens::FOREGROUND,
                fill: None,
                stroke_width: None,
            },
            &[],
        );
        let coins = c.b.text(
            "span",
            S::block(),
            Visual::default(),
            spec(shown.to_string(), BASE, BLACK, tokens::FOREGROUND).tab(),
        );
        let score = c.b.div(
            "span",
            S::row().shrink0().items(AlignItems::CENTER).gap(4.0),
            Visual::default(),
            &[coin, coins],
        );
        let vis = if first {
            Visual::default()
                .bg(tokens::AMBER_50)
                .border(1.0, tokens::AMBER_500)
                .radius(R_XL)
        } else {
            Visual::default()
                .bg(tokens::CARD)
                .border(1.0, tokens::BORDER)
                .radius(R_XL)
        };
        let card = c.b.div(
            "div",
            S::row()
                .full()
                .min0()
                .items(AlignItems::CENTER)
                .gap(12.0)
                .pad(12.0, 0.0, 12.0, 0.0)
                .border(1.0),
            vis,
            &[number, disc, name, score],
        );
        let mut li = S::block()
            .abs(
                Some(0.0),
                Some(if moving.is_some() {
                    0.0
                } else {
                    rank as f32 * h
                }),
                Some(0.0),
                None,
            )
            .h(h)
            .pad(0.0, 2.0, 0.0, 2.0);
        li.0.size.height = taffy::prelude::length(h);
        let row = c.b.div("li", li, Visual::default(), &[card]);
        if let Some(place) = moving {
            c.animate(
                row,
                None,
                Some(crate::ui::Xform::new(0.0, place * h, 0.0, 1.0, 1.0)),
            );
        }
        items.push(row);
    }
    c.b.div(
        "ol",
        S::block().min0().h(h * rows.len().max(1) as f32).relative(),
        Visual::default().clipped(),
        &items,
    )
}
