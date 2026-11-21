//! The quiz game of a recording at any moment of it (`player/quiz-replay.svelte.ts` and
//! `player/quest-replay.svelte.ts`). The server logged the quiz service's host frames beside the
//! board (`k: "quiz"`, `t` on the recording's clock, `q` the frame), so the stage plays them back
//! through the same state the live room uses. Playing forward applies the frames since the last
//! position; seeking back starts again.
//!
//! The server blanks the name of anyone who is not a student of the class; those show as a
//! neutral name (`student`, "A student" in the player's strings).

use std::io::Read;

use flate2::read::GzDecoder;
use serde_json::Value;

use super::protocol::{
    QuestFrame, ScoreLine, ServerFrame, decode_frame, decode_quest_frame, rebase,
};
use super::quest::QuestGame;
use super::race::QuizGame;

#[derive(Debug, Clone, PartialEq)]
pub struct QuizRecord {
    pub t: f64,
    pub frame: ServerFrame,
}

/// The quiz frames of a board log (NDJSON text), decoded, in time order; what is not a frame of
/// the protocol is skipped.
pub fn parse_quiz_log(text: &str) -> Vec<QuizRecord> {
    let mut out: Vec<QuizRecord> = text
        .lines()
        .filter_map(|line| {
            let r: Value = serde_json::from_str(line).ok()?;
            let t = r.get("t")?.as_f64()?;
            if r.get("k")?.as_str()? != "quiz" {
                return None;
            }
            let q = r.get("q")?.as_array()?;
            Some(QuizRecord {
                t,
                frame: decode_frame(&rebase(q, t))?,
            })
        })
        .collect();
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// Unzips the board log (gzip, as served) and reads its quiz frames.
pub fn read_quiz_log(gz: impl Read) -> Result<Vec<QuizRecord>, std::io::Error> {
    let mut text = String::new();
    GzDecoder::new(gz).read_to_string(&mut text)?;
    Ok(parse_quiz_log(&text))
}

fn anonymize(lines: &[ScoreLine], student: &str) -> Vec<ScoreLine> {
    lines
        .iter()
        .map(|l| ScoreLine {
            name: if l.name.is_empty() {
                student.to_owned()
            } else {
                l.name.clone()
            },
            score: l.score,
        })
        .collect()
}

/// The frame with the names of players who are not in the recording made neutral.
fn named(frame: &ServerFrame, student: &str) -> ServerFrame {
    match frame {
        ServerFrame::Reveal {
            q,
            correct,
            counts,
            explanation,
            top,
        } => ServerFrame::Reveal {
            q: *q,
            correct: *correct,
            counts: counts.clone(),
            explanation: explanation.clone(),
            top: anonymize(top, student),
        },
        ServerFrame::Podium { top, me, all } => ServerFrame::Podium {
            top: anonymize(top, student),
            me: me.clone(),
            all: all.as_ref().map(|a| anonymize(a, student)),
        },
        ServerFrame::Progress {
            answered,
            players,
            counts,
            ranking: Some(ranking),
        } => ServerFrame::Progress {
            answered: *answered,
            players: *players,
            counts: counts.clone(),
            ranking: Some(anonymize(ranking, student)),
        },
        other => other.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Race,
    Quest,
}

/// A frame ends the game on the stage (the board comes back).
fn ends_stage(f: &ServerFrame) -> bool {
    matches!(f, ServerFrame::Idle | ServerFrame::Ended { .. })
}

/// A frame only a game sends: a lobby, a question, or Quest's lobby or running frame.
fn is_play(f: &ServerFrame) -> bool {
    matches!(
        f,
        ServerFrame::Lobby { .. }
            | ServerFrame::Question { .. }
            | ServerFrame::Quest { code: 20 | 22, .. }
    )
}

/// What a frame is compared with to see what it changed.
struct Seen {
    phase: super::race::Phase,
    question: Option<i64>,
    closing: bool,
    ranking: bool,
    players: i64,
    joined: usize,
}

impl Seen {
    fn of(g: &QuizGame) -> Self {
        Self {
            phase: g.phase,
            question: g.question.as_ref().map(|q| q.q),
            closing: g.closing,
            ranking: !g.ranking.is_empty(),
            players: g.players,
            joined: g.joined.len(),
        }
    }
}

/// When the race's screens last changed, for the animations the player runs on a change.
#[derive(Debug, Clone, Default)]
pub struct Since {
    /// The question that is open (its splash).
    pub question: Option<f64>,
    /// The reveal came up.
    pub reveal: Option<f64>,
    /// The podium came up (race or Quest).
    pub podium: Option<f64>,
    /// The question closed, the reveal on its way.
    pub closing: Option<f64>,
    /// The live ranking began.
    pub ranking: Option<f64>,
    /// The lobby's player count last changed.
    pub players: Option<f64>,
    /// When each name in the lobby's list joined.
    pub joined: Vec<f64>,
    /// The Quest board last changed, and what its rows were before.
    pub board: Option<f64>,
    pub board_before: Vec<super::protocol::Row>,
}

impl Since {
    fn update(&mut self, before: &Seen, game: &QuizGame, frame: &ServerFrame, at: f64) {
        use super::race::Phase;
        if game.phase == Phase::Question
            && (before.phase != Phase::Question
                || before.question != game.question.as_ref().map(|q| q.q))
        {
            self.question = Some(at);
        }
        if game.phase == Phase::Reveal && before.phase != Phase::Reveal {
            self.reveal = Some(at);
        }
        if game.closing && !before.closing {
            self.closing = Some(at);
        }
        if !game.ranking.is_empty() && !before.ranking {
            self.ranking = Some(at);
        }
        if game.players != before.players {
            self.players = Some(at);
        }
        if game.joined.len() > before.joined {
            self.joined.resize(game.joined.len(), at);
        } else if game.joined.len() < before.joined {
            self.joined.truncate(game.joined.len());
        }
        let _ = frame;
    }
}

#[derive(Debug, Clone)]
pub struct QuizReplay {
    records: Vec<QuizRecord>,
    student: String,
    pub game: QuizGame,
    pub quest: QuestGame,
    pub mode: Mode,
    /// A Quest game is on (from its first frame to the end frame).
    pub quest_on: bool,
    /// When each thing on the stage last changed (the recording's clock): the animations run from there.
    pub since: Since,
    next_race: usize,
    next_quest: usize,
    time: f64,
}

impl QuizReplay {
    pub fn new(records: Vec<QuizRecord>, student: impl Into<String>) -> Self {
        Self {
            records,
            student: student.into(),
            game: QuizGame::default(),
            quest: QuestGame::default(),
            mode: Mode::Race,
            quest_on: false,
            since: Since::default(),
            next_race: 0,
            next_quest: 0,
            time: -1.0,
        }
    }

    /// The recording has a game in it: the service also says "nothing running" to anyone who only
    /// opened the quiz page.
    pub fn has(&self) -> bool {
        self.records.iter().any(|r| is_play(&r.frame))
    }

    /// A game is on the stage now: it began and has not ended.
    pub fn on(&self) -> bool {
        match self.mode {
            Mode::Quest => self.quest_on,
            Mode::Race => self.game.active(),
        }
    }

    /// Shows the game as it was at `time` (ms into the recording); going back starts again.
    pub fn seek(&mut self, time: f64) {
        if time < self.time {
            self.game = QuizGame::default();
            self.quest = QuestGame::default();
            self.mode = Mode::Race;
            self.quest_on = false;
            self.since = Since::default();
            self.next_race = 0;
            self.next_quest = 0;
        }
        self.time = time;
        while self.next_race < self.records.len() && self.records[self.next_race].t <= time {
            let at = self.records[self.next_race].t;
            let frame = named(&self.records[self.next_race].frame, &self.student);
            self.next_race += 1;
            let before = Seen::of(&self.game);
            if matches!(frame, ServerFrame::Podium { .. }) && self.mode == Mode::Race {
                self.since.podium = Some(at);
            }
            self.apply_race(&frame);
            self.since.update(&before, &self.game, &frame, at);
        }
        while self.next_quest < self.records.len() && self.records[self.next_quest].t <= time {
            let at = self.records[self.next_quest].t;
            let frame = named(&self.records[self.next_quest].frame, &self.student);
            self.next_quest += 1;
            if matches!(frame, ServerFrame::Podium { .. }) && self.mode == Mode::Quest {
                self.since.podium = Some(at);
            }
            let before = self.quest.board.top.clone();
            self.apply_quest(&frame);
            if self.quest.board.top != before {
                self.since.board_before = before;
                self.since.board = Some(at);
            }
        }
    }

    /// The room's routing: Quest's frames make it Quest, the race's lobby or an end make it the race.
    fn apply_race(&mut self, frame: &ServerFrame) {
        if matches!(frame, ServerFrame::Quest { .. }) {
            self.mode = Mode::Quest;
            self.quest_on = true;
            return;
        }
        if matches!(frame, ServerFrame::Lobby { .. }) || ends_stage(frame) {
            self.mode = Mode::Race;
            self.quest_on = false;
        } else if self.mode == Mode::Quest {
            return;
        }
        self.game.apply(frame);
    }

    fn apply_quest(&mut self, frame: &ServerFrame) {
        match frame {
            ServerFrame::Quest { frame, .. } => {
                let Some(decoded) = decode_quest_frame(frame) else {
                    return;
                };
                let decoded = match decoded {
                    QuestFrame::Board {
                        top,
                        players,
                        answers,
                        correct,
                    } => {
                        let top = top
                            .into_iter()
                            .map(|mut r| {
                                if r.name.is_empty() {
                                    r.name = self.student.clone();
                                }
                                r
                            })
                            .collect();
                        QuestFrame::Board {
                            top,
                            players,
                            answers,
                            correct,
                        }
                    }
                    QuestFrame::Standings { top } => QuestFrame::Standings {
                        top: top
                            .into_iter()
                            .map(|mut r| {
                                if r.name.is_empty() {
                                    r.name = self.student.clone();
                                }
                                r
                            })
                            .collect(),
                    },
                    other => other,
                };
                self.quest.apply_quest(&decoded);
            }
            ServerFrame::Idle => self.quest.idle(),
            ServerFrame::Count { players } => self.quest.count(*players),
            ServerFrame::Podium { top, all, .. } => self.quest.podium(top, all.as_deref()),
            ServerFrame::Ended { reason } => self.quest.ended(*reason),
            _ => {}
        }
    }
}
