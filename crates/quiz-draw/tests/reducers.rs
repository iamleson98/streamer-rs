//! The game reducers against the web player's own: every scene of the corpus (made by
//! `parity/gen.ts` from lesson-replay's code) is replayed at its times and the state compared
//! field by field with what the player's reducers gave.

use std::{fs, path::PathBuf};

use replay_model::quiz::{
    Mode, QuizReplay,
    avatar::{avatar_of, fallback_character},
    fit::{option_size, question_size, tile_columns, js_len},
    format::{accuracy, clock, countdown},
    read_quiz_log,
};
use serde_json::{Value, json};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// Every number as a float, so `12000` and `12000.0` are the same, to nine places (serde_json reads a
/// long decimal a unit in the last place off without its `float_roundtrip` feature).
fn norm(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!((n.as_f64().unwrap() * 1e9).round() / 1e9),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        other => other.clone(),
    }
}

fn student(locale: &str) -> String {
    let strings: Value = serde_json::from_str(&fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/strings.json")).unwrap()).unwrap();
    strings[locale]["replay.quiz.student"].as_str().unwrap().to_owned()
}

fn count(c: replay_model::quiz::format::Countdown) -> Value {
    json!({"left": c.left, "seconds": c.seconds, "fraction": c.fraction, "urgent": c.urgent, "over": c.over})
}

fn view(replay: &QuizReplay, t: f64) -> Value {
    let g = &replay.game;
    let q = &replay.quest;
    let race = json!({
        "phase": g.phase, "closing": g.closing, "stage": 2, "title": g.title,
        "questionCount": g.question_count, "players": g.players, "answered": g.answered,
        "counts": g.counts, "ranking": g.ranking, "standings": g.standings, "joined": g.joined,
        "prevTop": g.prev_top, "question": g.question, "reveal": g.reveal, "podium": g.podium,
        "endedReason": g.ended_reason,
    });
    let mut names: Vec<String> = vec![];
    let mut add = |n: &str| {
        if !names.iter().any(|x| x == n) {
            names.push(n.to_owned());
        }
    };
    g.joined.iter().for_each(|n| add(n));
    g.ranking.iter().for_each(|l| add(&l.name));
    g.standings.iter().for_each(|l| add(&l.name));
    if let Some(r) = &g.reveal {
        r.top.iter().for_each(|l| add(&l.name));
    }
    if let Some(p) = &g.podium {
        p.top.iter().for_each(|l| add(&l.name));
        p.all.iter().flatten().for_each(|l| add(&l.name));
    }
    q.board.top.iter().for_each(|r| add(&r.name));
    if let Some(p) = &q.podium {
        p.top.iter().for_each(|l| add(&l.name));
        p.all.iter().flatten().for_each(|l| add(&l.name));
    }
    let quest = json!({
        "phase": q.phase, "title": q.title, "questionCount": q.question_count, "seconds": q.seconds,
        "friendly": q.friendly, "players": q.players, "endsAt": q.ends_at, "endedReason": q.ended_reason,
        "roster": q.roster, "board": q.board, "podium": q.podium,
        "characters": names.iter().map(|n| json!([n, q.character_of(n)])).collect::<Vec<_>>(),
    });
    let q_count = g.question.as_ref().map(|p| count(countdown(p.ends_at as f64, p.limit_ms as f64, t)));
    let quest_count = (q.ends_at != 0).then(|| count(countdown(q.ends_at as f64, (q.seconds * 1000) as f64, t)));
    let longest = g.question.as_ref().map_or(0, |p| p.options.iter().map(|o| js_len(o)).max().unwrap_or(0));
    let derived = json!({
        "questionSize": g.question.as_ref().map(|p| question_size(js_len(&p.text)).class()),
        "optionSizes": g.question.as_ref().map_or(vec![], |p| p.options.iter().map(|o| option_size(js_len(o)).class()).collect()),
        "columns": if g.question.is_some() { [200.0, 400.0, 560.0, 640.0].iter().map(|w| tile_columns(*w, longest)).collect() } else { vec![] },
        "accuracy": accuracy(q.board.correct, q.board.answers),
        "clock": clock(countdown(q.ends_at as f64, (q.seconds * 1000) as f64, t).left),
    });
    let avatars: Vec<Value> = names
        .iter()
        .map(|n| {
            let a = avatar_of(n);
            json!({"name": n, "emoji": a.emoji, "disc": a.disc, "fallback": fallback_character(n)})
        })
        .collect();
    json!({
        "t": t, "mode": replay.mode, "on": replay.on(), "has": replay.has(),
        "race": race, "quest": quest, "countdown": q_count, "questCountdown": quest_count,
        "derived": derived, "avatars": avatars,
    })
}

#[test]
fn every_scene_matches_the_players_reducers() {
    let index: Value = serde_json::from_str(&fs::read_to_string(corpus().join("index.json")).unwrap()).unwrap();
    let scenes = index["scenes"].as_array().unwrap();
    assert!(scenes.len() > 30);
    let mut checked = 0;
    for scene in scenes {
        let name = scene["name"].as_str().unwrap();
        let log = fs::File::open(corpus().join(format!("{}.ndjson.gz", scene["log"].as_str().unwrap()))).unwrap();
        let records = read_quiz_log(log).unwrap();
        let mut replay = QuizReplay::new(records, student(scene["locale"].as_str().unwrap()));
        let expected: Vec<Value> =
            serde_json::from_str(&fs::read_to_string(corpus().join(format!("expected/{name}.json"))).unwrap()).unwrap();
        for e in &expected {
            let t = e["t"].as_f64().unwrap();
            replay.seek(t);
            let got = norm(&view(&replay, t));
            assert_eq!(got, norm(e), "{name} at {t}");
            checked += 1;
        }
    }
    assert_eq!(checked, scenes.iter().map(|s| s["times"].as_array().unwrap().len()).sum::<usize>());
}

#[test]
fn seeking_back_starts_a_fresh_game() {
    let log = fs::File::open(corpus().join("mixed-race-then-quest.ndjson.gz")).unwrap();
    let mut replay = QuizReplay::new(read_quiz_log(log).unwrap(), "A student");
    replay.seek(54_000.0);
    assert_eq!(replay.mode, Mode::Quest);
    replay.seek(1_500.0);
    assert_eq!(replay.mode, Mode::Race);
    assert_eq!(replay.game.title, "Geography");
    assert_eq!(replay.quest.board.top.len(), 0);
}
