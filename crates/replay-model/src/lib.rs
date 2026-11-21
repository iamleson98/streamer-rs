//! The recorded board of a lesson, without drawing: the log's records, the board elements and how
//! they are validated, the scene they merge into, the replay clock, what other people's pointers
//! show, and where the board is looked at.
//!
//! Every rule here is a port of the web player's (`lesson-replay`, `src/lib/replay/board/` and
//! `player/board-*.ts`), which stays the source of truth: when it changes, this changes with it,
//! and the parity corpus in `board-draw` is what tells the two apart.

pub mod element;
pub mod follow;
pub mod log;
pub mod pages;
pub mod presence;
pub mod quiz;
pub mod replay;
pub mod scene;
pub mod viewport;

pub use element::{BoardElement, ColorToken, ElementKind, MeasureText, Point, Rect};
pub use follow::board_viewport;
pub use log::{BoardRecord, RecordKind, parse_board_log, read_board_log};
pub use presence::{PRESENCE_TTL_MS, PresencePayload, RemotePresence, apply_presence};
pub use replay::BoardReplay;
pub use scene::{BoardScene, Envelope, OrderedElement, wins};
pub use viewport::Viewport;
