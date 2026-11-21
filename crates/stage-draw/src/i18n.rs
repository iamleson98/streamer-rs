//! The replay's strings, read from lesson-replay's own `en.ts` and `vi.ts` (so there is one copy of
//! every string); a test fails when their keys differ.

use std::collections::HashMap;

static EN: &str = include_str!("../../../lesson-replay/src/lib/replay/i18n/en.ts");
static VI: &str = include_str!("../../../lesson-replay/src/lib/replay/i18n/vi.ts");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Vi,
}

impl Lang {
    /// `vi` or else English, as the render page does.
    pub fn of(locale: &str) -> Self {
        if locale == "vi" { Self::Vi } else { Self::En }
    }
}

/// The quoted strings of a locale file's table, as (key, value) pairs.
fn parse(source: &str) -> HashMap<String, String> {
    let body = source.split_once("= {").map_or("", |(_, rest)| rest);
    let mut strings = Vec::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\'' && c != '"' {
            continue;
        }
        let quote = c;
        let mut s = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('n') => s.push('\n'),
                    Some(other) => s.push(other),
                    None => break,
                },
                c if c == quote => break,
                other => s.push(other),
            }
        }
        strings.push(s);
    }
    strings
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[key, value]| (key.clone(), value.clone()))
        .collect()
}

/// A locale's table.
#[derive(Debug, Clone)]
pub struct Strings(HashMap<String, String>);

impl Strings {
    pub fn of(lang: Lang) -> Self {
        Self(parse(match lang {
            Lang::En => EN,
            Lang::Vi => VI,
        }))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_locales_have_the_card_text_and_the_same_keys() {
        let (en, vi) = (Strings::of(Lang::En), Strings::of(Lang::Vi));
        assert_eq!(
            en.get("replay.card.startsIn"),
            Some("Live lesson starts in")
        );
        assert_eq!(vi.get("replay.card.startsIn"), Some("Buổi học bắt đầu sau"));
        let mut a: Vec<_> = en.keys().collect();
        let mut b: Vec<_> = vi.keys().collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
        assert!(a.len() > 50);
    }

    #[test]
    fn a_value_on_the_next_line_and_escapes_are_read() {
        let got = parse("x = {\n\t'a.b':\n\t\t'it\\'s',\n\t'c': 'two\\nlines'\n};");
        assert_eq!(got["a.b"], "it's");
        assert_eq!(got["c"], "two\nlines");
    }

    #[test]
    fn the_locale_is_vi_or_english() {
        assert_eq!(Lang::of("vi"), Lang::Vi);
        assert_eq!(Lang::of("fr"), Lang::En);
    }
}
