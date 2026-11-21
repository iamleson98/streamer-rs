//! The player's quiz strings (`i18n/en.ts` and `vi.ts`, generated into `strings.json` by `parity/gen.ts`),
//! with the same `{name}` filling as the player's `t()`.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    En,
    Vi,
}

impl Locale {
    fn key(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Vi => "vi",
        }
    }
}

type Dictionary = HashMap<String, String>;

fn dictionaries() -> &'static HashMap<String, Dictionary> {
    static ALL: OnceLock<HashMap<String, Dictionary>> = OnceLock::new();
    ALL.get_or_init(|| serde_json::from_str(include_str!("strings.json")).expect("strings.json"))
}

/// The text of `key`; the player's Vietnamese is the fallback, then the key itself.
pub fn t(locale: Locale, key: &str, params: &[(&str, String)]) -> String {
    let all = dictionaries();
    let text = all
        .get(locale.key())
        .and_then(|d| d.get(key))
        .or_else(|| all.get("vi").and_then(|d| d.get(key)))
        .map_or(key, String::as_str);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if after[..close].chars().all(|c| c.is_alphanumeric() || c == '_') && close > 0 => {
                match params.iter().find(|(k, _)| *k == &after[..close]) {
                    Some((_, v)) => out.push_str(v),
                    None => out.push_str(&rest[open..open + close + 2]),
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The student name for guests (`replay.quiz.student`).
pub fn student(locale: Locale) -> String {
    t(locale, "replay.quiz.student", &[])
}
