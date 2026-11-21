//! The quiz and Quest games of a recording, without drawing: the service's frames, the state they
//! make, and the small rules the screens read. Every rule is a port of the web player's
//! (`lesson-replay`, `src/lib/replay/quiz/` and `player/quiz-replay.svelte.ts`); the fixtures in
//! `quiz-draw/tests/corpus` (made from the player's own code by `gen.ts`) are what tell the two apart.

pub mod avatar;
pub mod fit;
pub mod format;
pub mod protocol;
pub mod quest;
pub mod race;
pub mod replay;

pub use replay::{Mode, QuizRecord, QuizReplay, Since, parse_quiz_log, read_quiz_log};
