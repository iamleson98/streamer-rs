//! A Quest game as the host's recording shows it (`quiz/quest/quest-game.svelte.ts`, fed by
//! `player/quest-replay.svelte.ts`): the lobby's head count, the board while it runs, the podium.
//! A player's own loop (question, verdict, chests, banners) is not on the host's stage and is not
//! kept here.

use serde::Serialize;

use super::protocol::{QuestFrame, Row, ScoreLine};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestPhase {
    None,
    Lobby,
    Running,
    Podium,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestBoard {
    pub top: Vec<Row>,
    pub players: i64,
    pub answers: i64,
    pub correct: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestPodium {
    pub top: Vec<ScoreLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<ScoreLine>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestGame {
    pub phase: QuestPhase,
    pub title: String,
    pub question_count: i64,
    /// The length of the game as the teacher chose it (seconds).
    pub seconds: i64,
    pub friendly: bool,
    pub players: i64,
    /// When the game ends, on the recording's clock (0 until it runs).
    pub ends_at: i64,
    pub ended_reason: Option<i64>,
    pub roster: Vec<RosterEntry>,
    pub board: QuestBoard,
    pub podium: Option<QuestPodium>,
    /// Every character seen by name (the roster, the board, the standings), in the order first seen.
    #[serde(skip)]
    characters: Vec<(String, i64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterEntry {
    pub name: String,
    pub character: i64,
}

impl Default for QuestGame {
    fn default() -> Self {
        Self {
            phase: QuestPhase::None,
            title: String::new(),
            question_count: 0,
            seconds: 0,
            friendly: false,
            players: 0,
            ends_at: 0,
            ended_reason: None,
            roster: vec![],
            board: QuestBoard {
                top: vec![],
                players: 0,
                answers: 0,
                correct: 0,
            },
            podium: None,
            characters: vec![],
        }
    }
}

impl QuestGame {
    /// The character a name has been seen with.
    pub fn character_of(&self, name: &str) -> Option<i64> {
        self.characters
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| *c)
    }

    fn remember(&mut self, name: &str, character: i64) {
        match self.characters.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = character,
            None => self.characters.push((name.to_owned(), character)),
        }
    }

    /// A Quest frame (the player's own frames change nothing here).
    pub fn apply_quest(&mut self, frame: &QuestFrame) {
        match frame {
            QuestFrame::Lobby {
                title,
                questions,
                players,
                seconds,
                friendly,
            } => {
                // A lobby seen again (a reconnect) keeps what was chosen in it.
                if self.phase != QuestPhase::Lobby {
                    self.clear();
                }
                self.ended_reason = None;
                self.phase = QuestPhase::Lobby;
                self.title = title.clone();
                self.question_count = *questions;
                self.players = *players;
                self.seconds = *seconds;
                self.friendly = *friendly;
            }
            QuestFrame::Roster { players } => {
                for (name, character) in players {
                    self.remember(name, *character);
                }
                self.roster = self
                    .characters
                    .iter()
                    .map(|(name, character)| RosterEntry {
                        name: name.clone(),
                        character: *character,
                    })
                    .collect();
            }
            QuestFrame::Running { ends_at } => {
                self.phase = QuestPhase::Running;
                self.ends_at = *ends_at;
            }
            QuestFrame::Standings { top } => {
                for r in top {
                    self.remember(&r.name, r.character);
                }
            }
            QuestFrame::Board {
                top,
                players,
                answers,
                correct,
            } => {
                for r in top {
                    self.remember(&r.name, r.character);
                }
                self.board = QuestBoard {
                    top: top.clone(),
                    players: *players,
                    answers: *answers,
                    correct: *correct,
                };
                self.players = *players;
            }
            QuestFrame::Other => {}
        }
    }

    pub fn count(&mut self, players: i64) {
        self.players = players;
    }

    pub fn podium(&mut self, top: &[ScoreLine], all: Option<&[ScoreLine]>) {
        self.podium = Some(QuestPodium {
            top: top.to_vec(),
            all: all.map(<[ScoreLine]>::to_vec),
        });
        self.phase = QuestPhase::Podium;
    }

    pub fn idle(&mut self) {
        self.clear();
    }

    pub fn ended(&mut self, reason: i64) {
        self.clear();
        self.ended_reason = Some(reason);
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}
