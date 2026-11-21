//! The quiz and Quest stages of a recorded lesson, drawn natively as the web player shows them in a recording.

pub mod art;
pub mod art_data;
pub mod color;
pub mod dump;
pub mod i18n;
pub mod kit;
pub mod podium;
pub mod quest;
pub mod race;
pub mod stage;
pub mod text;
pub mod tokens;
pub mod ui;

pub use i18n::Locale;
pub use stage::QuizStage;
