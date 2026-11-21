//! The race's state as the frames tell it (`quiz/game.svelte.ts`), for a recording: the host's view
//! of a replay (`staged: false`, so a reveal is whole at once). What only a player's screen reads
//! (their own answer, rank, streak, refusals) is left out: a recording is the host's socket.

use serde::Serialize;

use super::protocol::{ScoreLine, ServerFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    None,
    Lobby,
    Question,
    Reveal,
    Podium,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub q: i64,
    pub count: i64,
    pub ends_at: i64,
    pub kind: i64,
    pub text: String,
    pub options: Vec<String>,
    /// The time limit as first seen (ms): what the timer ring is a fraction of.
    pub limit_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reveal {
    pub q: i64,
    pub correct: i64,
    pub counts: Vec<i64>,
    pub explanation: Option<String>,
    pub top: Vec<ScoreLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Podium {
    pub top: Vec<ScoreLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<ScoreLine>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizGame {
    pub phase: Phase,
    pub closing: bool,
    pub title: String,
    pub question_count: i64,
    pub players: i64,
    pub answered: i64,
    pub counts: Vec<i64>,
    pub ranking: Vec<ScoreLine>,
    pub standings: Vec<ScoreLine>,
    pub joined: Vec<String>,
    pub prev_top: Vec<ScoreLine>,
    pub question: Option<Question>,
    pub reveal: Option<Reveal>,
    pub podium: Option<Podium>,
    pub ended_reason: Option<i64>,
    #[serde(skip)]
    last_top: Vec<ScoreLine>,
}

impl Default for QuizGame {
    fn default() -> Self {
        Self {
            phase: Phase::None,
            closing: false,
            title: String::new(),
            question_count: 0,
            players: 0,
            answered: 0,
            counts: vec![],
            ranking: vec![],
            standings: vec![],
            joined: vec![],
            prev_top: vec![],
            question: None,
            reveal: None,
            podium: None,
            ended_reason: None,
            last_top: vec![],
        }
    }
}

impl QuizGame {
    pub fn active(&self) -> bool {
        self.phase != Phase::None
    }

    pub fn apply(&mut self, frame: &ServerFrame) {
        match frame {
            ServerFrame::Idle => self.clear(),
            ServerFrame::Lobby {
                title,
                questions,
                players,
            } => {
                self.clear();
                self.phase = Phase::Lobby;
                self.title = title.clone();
                self.question_count = *questions;
                self.players = *players;
                self.ended_reason = None;
            }
            ServerFrame::Question {
                now,
                q,
                count,
                ends_at,
                kind,
                text,
                options,
            } => {
                let again = self.phase == Phase::Question
                    && self.question.as_ref().is_some_and(|p| p.q == *q);
                let limit_ms = match (&self.question, again) {
                    (Some(p), true) => p.limit_ms,
                    _ => ends_at - now,
                };
                self.question = Some(Question {
                    q: *q,
                    count: *count,
                    ends_at: *ends_at,
                    kind: *kind,
                    text: text.clone(),
                    options: options.clone(),
                    limit_ms,
                });
                if !again {
                    self.answered = 0;
                    self.counts = vec![];
                    self.ranking = vec![];
                }
                self.reveal = None;
                self.podium = None;
                self.phase = Phase::Question;
                self.closing = false;
                self.ended_reason = None;
            }
            ServerFrame::Count { players } => self.players = *players,
            ServerFrame::Progress {
                answered,
                players,
                counts,
                ranking,
            } => {
                self.answered = *answered;
                self.players = *players;
                if let Some(counts) = counts {
                    self.counts = counts.clone();
                }
                if let Some(ranking) = ranking {
                    self.ranking = ranking.clone();
                }
            }
            ServerFrame::Names { names } => self.joined.extend(names.iter().cloned()),
            ServerFrame::Closed { q } => {
                if self.phase == Phase::Question
                    && self.question.as_ref().is_some_and(|p| p.q == *q)
                {
                    self.closing = true;
                }
            }
            ServerFrame::Reveal {
                q,
                correct,
                counts,
                explanation,
                top,
            } => {
                self.prev_top = std::mem::replace(&mut self.last_top, top.clone());
                self.standings = top.clone();
                self.closing = false;
                self.reveal = Some(Reveal {
                    q: *q,
                    correct: *correct,
                    counts: counts.clone(),
                    explanation: explanation.clone(),
                    top: top.clone(),
                });
                self.phase = Phase::Reveal;
            }
            ServerFrame::Podium { top, all, .. } => {
                self.podium = Some(Podium {
                    top: top.clone(),
                    all: all.clone(),
                });
                self.phase = Phase::Podium;
            }
            ServerFrame::Ended { reason } => {
                self.clear();
                self.ended_reason = Some(*reason);
            }
            ServerFrame::Error | ServerFrame::Ack | ServerFrame::Quest { .. } => {}
        }
    }

    fn clear(&mut self) {
        let ended_reason = self.ended_reason;
        *self = Self::default();
        self.ended_reason = ended_reason;
    }
}
