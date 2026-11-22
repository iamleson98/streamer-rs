//! The countdown card that can open a video: how the text is fitted, apart from the painting
//! (`paint.rs`). Port of `export/card.ts`.

use crate::clock::FPS;

/// The seconds the countdown shows on card frame `i` of `card_frames`: the time left, rounded up,
/// so the card opens on the whole length ("5:00") and its last frame still reads "0:01".
pub fn countdown_seconds(i: u64, card_frames: u64) -> u64 {
    (card_frames - i).div_ceil(u64::from(FPS))
}

/// `M:SS`.
pub fn format_countdown(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[derive(Debug, Clone, PartialEq)]
pub struct FittedTitle {
    pub px: u32,
    pub lines: Vec<String>,
}

/// Greedy word wrap of `text` into lines no wider than `width` at `px`; a word wider than a line is
/// cut.
pub fn wrap_lines(
    text: &str,
    width: f64,
    px: u32,
    measure: &dyn Fn(&str, u32) -> f64,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let joined = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if measure(&joined, px) <= width {
            line = joined;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        let chars: Vec<char> = word.chars().collect();
        let mut from = 0;
        let slice = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
        // A word wider than a line is cut where it stops fitting; what is left starts the next line.
        while measure(&slice(from, chars.len()), px) > width {
            let mut cut = chars.len() - 1;
            while cut > from + 1 && measure(&slice(from, cut), px) > width {
                cut -= 1;
            }
            lines.push(slice(from, cut));
            from = cut;
        }
        line = slice(from, chars.len());
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// `text` cut to fit `width` at `px`, ending in an ellipsis.
fn ellipsize(text: &str, width: f64, px: u32, measure: &dyn Fn(&str, u32) -> f64) -> String {
    let chars: Vec<char> = text.chars().collect();
    for n in (1..=chars.len()).rev() {
        let head: String = chars[..n].iter().collect();
        let shown = format!("{}…", head.trim_end());
        if measure(&shown, px) <= width {
            return shown;
        }
    }
    "…".to_owned()
}

#[derive(Debug, Clone, Copy)]
pub struct TitleRange {
    pub max_px: u32,
    pub min_px: u32,
    pub max_lines: usize,
}

/// The title in at most `max_lines` lines: the largest size from `max_px` down to `min_px` (steps
/// of 2) at which it fits; at `min_px` what still does not fit is cut with an ellipsis.
pub fn fit_title(
    title: &str,
    width: f64,
    measure: &dyn Fn(&str, u32) -> f64,
    range: &TitleRange,
) -> FittedTitle {
    let mut px = range.max_px;
    while px >= range.min_px {
        let lines = wrap_lines(title, width, px, measure);
        if lines.len() <= range.max_lines {
            return FittedTitle { px, lines };
        }
        px -= 2;
    }
    let lines = wrap_lines(title, width, range.min_px, measure);
    let mut kept: Vec<String> = lines.iter().take(range.max_lines).cloned().collect();
    if range.max_lines > 0 {
        let rest = lines[range.max_lines - 1..].join(" ");
        kept[range.max_lines - 1] = ellipsize(&rest, width, range.min_px, measure);
    }
    FittedTitle {
        px: range.min_px,
        lines: kept,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character is `px / 2` wide.
    fn measure(s: &str, px: u32) -> f64 {
        s.chars().count() as f64 * f64::from(px) / 2.0
    }

    #[test]
    fn the_countdown_opens_on_the_whole_length_and_ends_on_one_second() {
        let frames = 5 * 60 * 30;
        assert_eq!(format_countdown(countdown_seconds(0, frames)), "5:00");
        assert_eq!(countdown_seconds(1, frames), 300);
        assert_eq!(countdown_seconds(frames - 1, frames), 1);
        assert_eq!(format_countdown(61), "1:01");
        assert_eq!(format_countdown(0), "0:00");
    }

    #[test]
    fn words_wrap_greedily_and_a_long_word_is_cut() {
        assert_eq!(wrap_lines("aa bb cc", 5.0, 2, &measure), ["aa bb", "cc"]);
        assert_eq!(wrap_lines("abcdefgh", 4.0, 2, &measure), ["abcd", "efgh"]);
        assert!(wrap_lines("   ", 10.0, 2, &measure).is_empty());
    }

    #[test]
    fn a_title_takes_the_largest_size_that_fits_or_ends_in_an_ellipsis() {
        let range = TitleRange {
            max_px: 20,
            min_px: 10,
            max_lines: 2,
        };
        let fits = fit_title("hello world", 120.0, &measure, &range);
        assert_eq!((fits.px, fits.lines.len()), (20, 1));
        let smaller = fit_title("hello world again", 100.0, &measure, &range);
        assert!(smaller.px < 20 && smaller.lines.len() <= 2);
        let cut = fit_title(&"word ".repeat(40), 100.0, &measure, &range);
        assert_eq!(cut.px, 10);
        assert_eq!(cut.lines.len(), 2);
        assert!(cut.lines[1].ends_with('…'));
        assert!(measure(&cut.lines[1], 10) <= 100.0);
    }
}
