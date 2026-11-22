//! The countdown card at the front of the video: how long, and what it says.

use stream_common::api::{RenderClaim, file_ms};

/// The lengths of countdown card the site offers (ms); anything else is drawn as no card.
const PREROLLS_MS: [u64; 5] = [0, 60_000, 120_000, 300_000, 600_000];
/// The most characters of the lesson's title and of the teacher's name the card shows (the site
/// bounds them too; the worker does not trust it).
const CARD_TITLE_CHARS: usize = 80;
const CARD_TEACHER_CHARS: usize = 60;

/// The countdown card at the front of the video: how long, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub preroll_ms: u64,
    pub title: String,
    pub teacher: String,
}

impl Card {
    pub fn from_claim(claim: &RenderClaim) -> Self {
        let preroll_ms = if PREROLLS_MS.contains(&claim.preroll_ms) {
            claim.preroll_ms
        } else {
            0
        };
        Self {
            preroll_ms,
            title: plain_text(&claim.card_title, CARD_TITLE_CHARS),
            teacher: plain_text(&claim.card_teacher, CARD_TEACHER_CHARS),
        }
    }

    /// The whole file's length: the card, then the lesson.
    pub fn file_ms(&self, duration_ms: i64) -> i64 {
        file_ms(duration_ms, self.preroll_ms)
    }
}

/// `text` without control characters, trimmed, at most `max` characters.
fn plain_text(text: &str, max: usize) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    clean
        .trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[cfg(test)]
pub(crate) fn claim_with(preroll_ms: u64, title: &str, teacher: &str) -> RenderClaim {
    serde_json::from_value(serde_json::json!({
        "request_id": "r1", "recording_id": "c1", "duration_ms": 60_000,
        "render_token": "t", "token_expires_at": 1, "lease_until": 1, "locale": "vi",
        "preroll_ms": preroll_ms, "card_title": title, "card_teacher": teacher,
    }))
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_offered_prerolls_draw_a_card() {
        for ok in [0, 60_000, 120_000, 300_000, 600_000] {
            assert_eq!(Card::from_claim(&claim_with(ok, "t", "n")).preroll_ms, ok);
        }
        for bad in [1, 59_999, 90_000, 600_001, 3_600_000, u64::MAX] {
            assert_eq!(
                Card::from_claim(&claim_with(bad, "t", "n")).preroll_ms,
                0,
                "{bad}"
            );
        }
        let card = Card::from_claim(&claim_with(120_000, "t", "n"));
        assert_eq!(card.file_ms(60_000), 180_000);
        assert_eq!(
            Card::from_claim(&claim_with(90_000, "t", "n")).file_ms(60_000),
            60_000
        );
    }
}
