//! Type sizes and tile columns that step with the length of the text (`quiz/fit.ts`). Sizes are
//! Tailwind steps; the screens map them to pixels.

/// A Tailwind type step, for the `sm:` (wide) variant the render always uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    Sm,
    Base,
    Lg,
    Xl,
    X2l,
    X3l,
    X4l,
}

impl Text {
    /// The Tailwind class.
    pub fn class(self) -> &'static str {
        match self {
            Self::Sm => "text-sm",
            Self::Base => "text-base",
            Self::Lg => "text-lg",
            Self::Xl => "text-xl",
            Self::X2l => "text-2xl",
            Self::X3l => "text-3xl",
            Self::X4l => "text-4xl",
        }
    }
}

/// The question's size class (wide variant) by text length (counted as JS counts: UTF-16 units).
pub fn question_size(length: usize) -> Text {
    if length > 260 {
        Text::Xl
    } else if length > 140 {
        Text::X2l
    } else if length > 70 {
        Text::X3l
    } else {
        Text::X4l
    }
}

/// The option text's size class (wide variant) by text length.
pub fn option_size(length: usize) -> Text {
    if length > 120 {
        Text::Sm
    } else if length > 60 {
        Text::Base
    } else if length > 30 {
        Text::Lg
    } else {
        Text::Xl
    }
}

const MAX_OPTION_LINES: usize = 3;
const TILE_CHROME: f64 = 92.0;
const CHAR_PX: f64 = 8.5;

/// How many columns the option tiles take in a grid `width` px wide: two, unless the longest
/// option would need more than three lines in a half-width tile.
pub fn tile_columns(width: f64, longest: usize) -> usize {
    if width <= 0.0 {
        return 2;
    }
    let text = (width - 8.0) / 2.0 - TILE_CHROME;
    let per_line = ((text / CHAR_PX).floor()).max(1.0) as usize;
    if longest.div_ceil(per_line) > MAX_OPTION_LINES {
        1
    } else {
        2
    }
}

/// JS string length: UTF-16 code units.
pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}
