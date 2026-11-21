//! The quiz service's frames as the recording keeps them (`quiz/protocol.ts` and
//! `quiz/quest/quest-protocol.ts` of the web player). A frame is a short JSON array whose first
//! element is its type; `decode_frame` turns one into a typed event, or `None` for what is not a
//! frame of the protocol (a wrong frame never breaks a game).

use serde::Serialize;
use serde_json::Value;

/// A number the way the player's `Number.isInteger` reads it.
fn int(v: &Value) -> Option<i64> {
    let f = v.as_f64()?;
    (f.is_finite() && f.fract() == 0.0).then_some(f as i64)
}

fn ints(v: &Value) -> Option<Vec<i64>> {
    v.as_array()?.iter().map(int).collect()
}

fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|s| s.as_str().map(str::to_owned))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoreLine {
    pub name: String,
    pub score: i64,
}

pub(crate) fn score_lines(v: &Value) -> Option<Vec<ScoreLine>> {
    v.as_array()?
        .iter()
        .map(|line| {
            let line = line.as_array()?;
            Some(ScoreLine {
                name: line.first()?.as_str()?.to_owned(),
                score: int(line.get(1)?)?,
            })
        })
        .collect()
}

/// A player's own result; the host's recording never has one, so it is only kept apart.
#[derive(Debug, Clone, PartialEq)]
pub struct Me {
    pub score: i64,
    pub rank: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerFrame {
    Idle,
    Lobby { title: String, questions: i64, players: i64 },
    Question {
        now: i64,
        q: i64,
        count: i64,
        ends_at: i64,
        kind: i64,
        text: String,
        options: Vec<String>,
    },
    Count { players: i64 },
    Progress {
        answered: i64,
        players: i64,
        counts: Option<Vec<i64>>,
        ranking: Option<Vec<ScoreLine>>,
    },
    Names { names: Vec<String> },
    Reveal {
        q: i64,
        correct: i64,
        counts: Vec<i64>,
        explanation: Option<String>,
        top: Vec<ScoreLine>,
    },
    Podium {
        top: Vec<ScoreLine>,
        me: Option<Me>,
        all: Option<Vec<ScoreLine>>,
    },
    Closed { q: i64 },
    Ended { reason: i64 },
    /// Only the numbers matter to the replay: the host's recording never shows an error.
    Error,
    Ack,
    /// A Quest frame (types 20 to 29), decoded by `decode_quest_frame` when a Quest game plays.
    Quest { code: i64, frame: Vec<Value> },
}

/// A text frame as an event, or `None` when it is not a frame of this protocol.
pub fn decode_frame(f: &[Value]) -> Option<ServerFrame> {
    let kind = int(f.first()?)?;
    let at = |i: usize| f.get(i).unwrap_or(&Value::Null);
    Some(match kind {
        0 => {
            int(at(1))?;
            ServerFrame::Idle
        }
        1 => {
            int(at(1))?;
            ServerFrame::Lobby {
                title: at(2).as_str()?.to_owned(),
                questions: int(at(3))?,
                players: int(at(4))?,
            }
        }
        2 => {
            let kind = int(at(5))?;
            if !(0..=2).contains(&kind) {
                return None;
            }
            ServerFrame::Question {
                now: int(at(1))?,
                q: int(at(2))?,
                count: int(at(3))?,
                ends_at: int(at(4))?,
                kind,
                text: at(6).as_str()?.to_owned(),
                options: strings(at(7))?,
            }
        }
        3 => ServerFrame::Count {
            players: int(at(1))?,
        },
        4 => {
            let (answered, players) = (int(at(1))?, int(at(2))?);
            if f.len() > 3 {
                let counts = ints(at(3))?;
                if f.len() == 4 {
                    ServerFrame::Progress {
                        answered,
                        players,
                        counts: Some(counts),
                        ranking: None,
                    }
                } else {
                    ServerFrame::Progress {
                        answered,
                        players,
                        counts: Some(counts),
                        ranking: Some(score_lines(at(4))?),
                    }
                }
            } else {
                ServerFrame::Progress {
                    answered,
                    players,
                    counts: None,
                    ranking: None,
                }
            }
        }
        5 => {
            let explanation = match at(4) {
                Value::Null => None,
                Value::String(s) => Some(s.clone()),
                _ => return None,
            };
            let frame = ServerFrame::Reveal {
                q: int(at(1))?,
                correct: int(at(2))?,
                counts: ints(at(3))?,
                explanation,
                top: score_lines(at(5))?,
            };
            // A player's frame carries a personal tail; it has to read, though the host never has one.
            if f.len() > 6 {
                let reads = (6..=10).all(|i| int(at(i)).is_some())
                    && (11..=12).all(|i| f.get(i).is_none() || int(at(i)).is_some());
                if !reads {
                    return None;
                }
            }
            frame
        }
        6 => {
            let top = score_lines(at(1))?;
            if at(2).is_array() {
                ServerFrame::Podium {
                    top,
                    me: None,
                    all: Some(score_lines(at(2))?),
                }
            } else {
                ServerFrame::Podium {
                    top,
                    me: Some(Me {
                        score: int(at(2))?,
                        rank: int(at(3))?,
                    }),
                    all: None,
                }
            }
        }
        7 => {
            let reason = int(at(1))?;
            if !(0..=4).contains(&reason) {
                return None;
            }
            ServerFrame::Ended { reason }
        }
        8 => {
            let code = int(at(1))?;
            if ![1, 2, 3, 4, 5, 6, 9].contains(&code) {
                return None;
            }
            int(at(2))?;
            ServerFrame::Error
        }
        9 => ServerFrame::Ack,
        10 => ServerFrame::Names {
            names: strings(at(1))?,
        },
        11 => ServerFrame::Closed { q: int(at(1))? },
        20..=29 => ServerFrame::Quest {
            code: kind,
            frame: f.to_vec(),
        },
        _ => return None,
    })
}

/// The frame with its deadline on the recording's clock (question `[2, now, q, count, endsAt, ...]`,
/// Quest running `[22, now, endsAt]`): `at + (endsAt - now)`, both from the same frame.
pub fn rebase(f: &[Value], at: f64) -> Vec<Value> {
    let Some(now) = f.get(1).and_then(Value::as_f64) else {
        return f.to_vec();
    };
    let end = match f.first().and_then(Value::as_f64) {
        Some(2.0) => 4,
        Some(22.0) => 2,
        _ => return f.to_vec(),
    };
    let Some(ends_at) = f.get(end).and_then(Value::as_f64) else {
        return f.to_vec();
    };
    let mut out = f.to_vec();
    out[1] = number(at);
    out[end] = number(at + (ends_at - now));
    out
}

/// A JSON number that reads back as the integer it is when it is one.
fn number(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
    }
}

// ───────────────────────── Quest ─────────────────────────

/// The twelve characters a player may choose, `0` to `11`.
pub const CHARACTERS: i64 = 12;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub name: String,
    pub character: i64,
    pub coins: i64,
}

fn rows(v: &Value) -> Option<Vec<Row>> {
    v.as_array()?
        .iter()
        .map(|r| {
            let r = r.as_array()?;
            let character = int(r.get(1)?)?;
            if !(0..CHARACTERS).contains(&character) {
                return None;
            }
            Some(Row {
                name: r.first()?.as_str()?.to_owned(),
                character,
                coins: int(r.get(2)?)?,
            })
        })
        .collect()
}

/// The Quest frames a host's recording shows the stage with. The player's own frames (23 to 27)
/// decode to `Other`: they change nothing the host's stage reads.
#[derive(Debug, Clone, PartialEq)]
pub enum QuestFrame {
    Lobby {
        title: String,
        questions: i64,
        players: i64,
        seconds: i64,
        friendly: bool,
    },
    Roster { players: Vec<(String, i64)> },
    Running { ends_at: i64 },
    Standings { top: Vec<Row> },
    Board {
        top: Vec<Row>,
        players: i64,
        answers: i64,
        correct: i64,
    },
    Other,
}

/// A Quest frame (an array whose first element is 20 to 29) as an event, or `None`.
pub fn decode_quest_frame(f: &[Value]) -> Option<QuestFrame> {
    let at = |i: usize| f.get(i).unwrap_or(&Value::Null);
    Some(match int(f.first()?)? {
        20 => {
            int(at(1))?;
            QuestFrame::Lobby {
                title: at(2).as_str()?.to_owned(),
                questions: int(at(3))?,
                players: int(at(4))?,
                seconds: int(at(5))?,
                friendly: int(at(6))? & 1 == 1,
            }
        }
        21 => {
            let players = at(1)
                .as_array()?
                .iter()
                .map(|p| {
                    let p = p.as_array()?;
                    let character = int(p.get(1)?)?;
                    if !(0..CHARACTERS).contains(&character) {
                        return None;
                    }
                    Some((p.first()?.as_str()?.to_owned(), character))
                })
                .collect::<Option<Vec<_>>>()?;
            QuestFrame::Roster { players }
        }
        22 => {
            int(at(1))?;
            QuestFrame::Running {
                ends_at: int(at(2))?,
            }
        }
        28 => {
            let top = rows(at(1))?;
            int(at(2))?;
            int(at(3))?;
            QuestFrame::Standings { top }
        }
        29 => {
            let top = rows(at(1))?;
            let [players, answers, correct, _] = [2, 3, 4, 5].map(|i| int(at(i)));
            int(at(5))?;
            QuestFrame::Board {
                top,
                players: players?,
                answers: answers?,
                correct: correct?,
            }
        }
        23..=27 => QuestFrame::Other,
        _ => return None,
    })
}
