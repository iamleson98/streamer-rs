//! A player's avatar, worked out from their name alone (`quiz/avatar.ts`): an animal on a coloured
//! disc. And Quest's character for a name nobody has told us about (`quiz/quest/characters.ts`).

const ANIMALS: [&str; 18] = [
    "🦊", "🐼", "🐨", "🦁", "🐯", "🐸", "🐙", "🦉", "🐧", "🦄", "🐢", "🐬", "🦋", "🐝", "🦖", "🐳",
    "🦔", "🐰",
];

/// The number of disc tones; `Avatar::disc` indexes them in the order of `avatar.ts`
/// (red, blue, amber, green, violet, cyan, pink).
pub const DISCS: usize = 7;

/// JS `String.prototype.trim`: white space and the byte order mark.
fn trim(name: &str) -> &str {
    name.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}')
}

/// FNV-1a over the code points of the trimmed, lower-cased name.
fn hash(name: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for ch in trim(name).to_lowercase().chars() {
        h ^= ch as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Avatar {
    pub emoji: &'static str,
    pub disc: usize,
}

pub fn avatar_of(name: &str) -> Avatar {
    let h = hash(name);
    Avatar {
        emoji: ANIMALS[h as usize % ANIMALS.len()],
        disc: ((h >> 8) % DISCS as u32) as usize,
    }
}

/// A stable Quest character for a name nobody has told us about.
pub fn fallback_character(name: &str) -> i64 {
    i64::from(hash(name) % 12)
}
