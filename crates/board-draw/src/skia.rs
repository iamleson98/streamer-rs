//! The Canvas backend on Skia: a CPU raster surface, text through `skia_safe::textlayout` with
//! the font bytes the caller passes in. There are no system fonts: a glyph the given faces do not
//! have is drawn as the first face's missing-glyph box, on every machine alike (a browser would
//! take it from a system font, which differs from machine to machine). Behind the board's own faces
//! come the fallback fonts (`FALLBACK_FONT_FILES`), so a character the board fonts lack still has
//! a glyph.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use replay_model::MeasureText;
use skia_safe::font_arguments::{VariationPosition, variation_position::Coordinate};
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextStyle, TypefaceFontProvider,
};
use skia_safe::{
    AlphaType, ClipOp, Color, ColorType, Data, EncodedImageFormat, FilterMode, Font, FontArguments,
    FontMgr, FourByteTag, Image, ImageInfo, MipmapMode, Paint, PaintCap, PaintJoin, PaintStyle,
    PathBuilder, PathDirection, PathEffect, RRect, Rect, SamplingOptions, Surface, Typeface,
    surfaces,
};

use unicode_segmentation::UnicodeSegmentation;

use crate::canvas::{Canvas, LineCap, LineJoin, Path, PathOp, Rgba, TextAlign, TextBaseline};

#[derive(Debug, thiserror::Error)]
pub enum SkiaError {
    #[error("font face {0} did not parse")]
    Font(usize),
    #[error("no font faces were given")]
    NoFonts,
    #[error("a {0}x{1} surface could not be made")]
    Surface(u32, u32),
}

/// The board's font files in the order of its font stack (`"Inter Variable", "Noto Sans",
/// "Noto Sans Math", "Noto Sans Symbols 2"`), as they are in the lesson-replay submodule's
/// `static/fonts/`: pass their bytes to [`FontSet::new`] in this order, so that each character is
/// drawn from the face the web player takes it from.
pub const BOARD_FONT_FILES: [&str; 6] = [
    "inter-5.3.0-latin-var-wght.woff2",
    "inter-5.3.0-vietnamese-var-wght.woff2",
    "inter-4.1-board-var-wght.woff2",
    "noto-sans-2.015-superscripts.woff2",
    "noto-sans-math-3.000-symbols.woff2",
    "noto-sans-symbols-2-2.008-symbols.woff2",
];

/// Face `index` of a font collection as a font file of its own (the tables are shared in the
/// collection; here they are copied behind a fresh table directory), or the bytes as they are when
/// they are no collection. Not every platform's font manager can open a face of a collection.
fn single_face(bytes: &[u8], index: u32) -> Option<Vec<u8>> {
    let be32 = |at: usize| -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    if bytes.get(..4)? != b"ttcf" {
        return (index == 0).then(|| bytes.to_vec());
    }
    if index >= be32(8)? {
        return None;
    }
    let dir = be32(12 + 4 * index as usize)? as usize;
    let count = u16::from_be_bytes(bytes.get(dir + 4..dir + 6)?.try_into().ok()?) as usize;
    let head = 12 + 16 * count;
    let mut out = bytes.get(dir..dir + 12)?.to_vec();
    let mut tables = Vec::new();
    for i in 0..count {
        let record = bytes.get(dir + 12 + 16 * i..dir + 28 + 16 * i)?;
        let at = u32::from_be_bytes(record[8..12].try_into().ok()?) as usize;
        let len = u32::from_be_bytes(record[12..16].try_into().ok()?) as usize;
        tables.push((record.to_vec(), bytes.get(at..at + len)?));
    }
    let mut offset = head;
    let mut body = Vec::new();
    for (record, table) in &mut tables {
        record[8..12].copy_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(record);
        body.extend_from_slice(table);
        let pad = (4 - table.len() % 4) % 4;
        body.extend(std::iter::repeat_n(0u8, pad));
        offset += table.len() + pad;
    }
    out.extend(body);
    Some(out)
}

/// Whether the face draws a visible glyph for U+1F600 (a grinning face).
fn draws_something(typeface: &Typeface) -> bool {
    let Some(mut surface) = surfaces::raster_n32_premul((64, 64)) else {
        return false;
    };
    let font = Font::from_typeface(typeface.clone(), 48.0);
    surface
        .canvas()
        .draw_str("\u{1f600}", (8, 52), &font, &Paint::default());
    let info = ImageInfo::new((64, 64), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let mut pixels = vec![0u8; 64 * 64 * 4];
    surface.read_pixels(&info, &mut pixels, 64 * 4, (0, 0)) && pixels.chunks(4).any(|p| p[3] != 0)
}

/// What the CSS `text-spacing-classes` call a character (`text-spacing-trim`), as far as it matters
/// for text drawn from the CJK face: Chromium looks at the neighbours inside one stretch of one
/// font only (a quote or a bracket of the Latin font next to a full-width mark is never trimmed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Han {
    Other,
    Open,
    Close,
    Middle,
    /// Comma, full stop and colon: where their glyph sits in the em box depends on the font
    /// ([`HanData`]).
    Dot,
    Colon,
}

fn han_class(c: char) -> Han {
    match c {
        '\u{3001}' | '\u{3002}' | '\u{ff0c}' | '\u{ff0e}' => Han::Dot,
        '\u{ff1a}' => Han::Colon,
        '\u{30fb}' | '\u{ff65}' => Han::Middle,
        '\u{2329}' | '\u{3008}' | '\u{300a}' | '\u{300c}' | '\u{300e}' | '\u{3010}'
        | '\u{3014}' | '\u{3016}' | '\u{3018}' | '\u{301a}' | '\u{301d}' | '\u{ff08}'
        | '\u{ff3b}' | '\u{ff5b}' | '\u{ff5f}' => Han::Open,
        '\u{232a}' | '\u{3009}' | '\u{300b}' | '\u{300d}' | '\u{300f}' | '\u{3011}'
        | '\u{3015}' | '\u{3017}' | '\u{3019}' | '\u{301b}' | '\u{301e}' | '\u{301f}'
        | '\u{ff09}' | '\u{ff3d}' | '\u{ff5d}' | '\u{ff60}' => Han::Close,
        _ => Han::Other,
    }
}

/// Chromium's rules for trimming the space of adjacent full-width punctuation
/// (`text-spacing-trim: normal`, `HanKerning` in Blink, which emulates the OpenType `chws` feature
/// with `halt` for a font without it, as Noto Sans CJK is: it has `halt`, `palt` and `kern` only):
/// an opening mark after another mark, and a closing mark before another, lose half their width.
fn han_kern_open(class: Han, last: Han) -> bool {
    class == Han::Open && matches!(last, Han::Open | Han::Middle | Han::Close)
}

fn han_kern_close(class: Han, last: Han) -> bool {
    last == Han::Close && matches!(class, Han::Close | Han::Middle)
}

/// How a CJK face draws the punctuation whose place in the em box depends on the font and the
/// language of its glyphs (Simplified Chinese: comma, full stop and colon at the left), read from
/// the glyph bounds as Blink does. The full-width semicolon is not read: measured in Chromium 159,
/// it never takes part, though its glyph also sits at the left.
#[derive(Clone, Copy, Debug)]
struct HanData {
    dot: Han,
    colon: Han,
}

impl HanData {
    fn of(typeface: &Typeface) -> Self {
        let font = Font::from_typeface(typeface.clone(), 1000.0);
        // The place of each glyph in its advance: closing (left half), opening (right half) or
        // middle (a narrow mark in the middle).
        let place = |chars: &[char]| {
            let glyphs: Vec<u16> = chars
                .iter()
                .map(|c| typeface.unichar_to_glyph(*c as i32))
                .collect();
            let mut widths = vec![0f32; glyphs.len()];
            let mut bounds = vec![Rect::default(); glyphs.len()];
            font.get_widths_bounds(&glyphs, Some(&mut widths), Some(&mut bounds), None);
            let mut found: Option<(f32, Han)> = None;
            for ((glyph, width), b) in glyphs.iter().zip(&widths).zip(&bounds) {
                if *glyph == 0 {
                    continue;
                }
                let half = width / 2.0;
                let class = if b.right <= half {
                    Han::Close
                } else if b.left >= half {
                    Han::Open
                } else if b.width() <= half && b.left >= half / 2.0 {
                    Han::Middle
                } else {
                    Han::Other
                };
                match found {
                    None => found = Some((*width, class)),
                    Some((w, c)) if w == *width && c == class => {}
                    Some(_) => return Han::Other,
                }
            }
            found.map_or(Han::Other, |(_, class)| class)
        };
        Self {
            dot: place(&['\u{3001}', '\u{3002}', '\u{ff0c}', '\u{ff0e}']),
            colon: place(&['\u{ff1a}']),
        }
    }

    fn class(&self, c: char) -> Han {
        match han_class(c) {
            Han::Dot => self.dot,
            Han::Colon => self.colon,
            other => other,
        }
    }
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' | '\u{ff66}'..='\u{ff9f}')
}

/// Whether Chromium kerns the pair (a browser draws the board's canvas text with kerning off for
/// the shaping of CJK, one character at a time, and joins only the marks that cannot start a
/// line: comma, full stop, the prolonged sound mark and the middle dot, which stay with the
/// character before them). Measured in Chromium 159 for every pair of kana, Han, Hangul and
/// punctuation: a kana pair is never kerned, a kana before one of those marks is, and so is the
/// mark with the kana after it only when the mark starts the line.
fn han_joined(a: char, b: char, line_start: bool) -> bool {
    matches!(
        b,
        '\u{3001}' | '\u{3002}' | '\u{ff0c}' | '\u{ff0e}' | '\u{30fc}' | '\u{30fb}'
    ) || (line_start && matches!(a, '\u{30fc}' | '\u{30fb}'))
}

/// A CJK stretch cut into pieces that are shaped apart: where the font would kern a kana pair that
/// Chromium does not, and around the characters that get `halt` (half width), each with the font
/// features to shape it with. A single character is given `kern` 0 so that it is not merged back
/// into its neighbour's text run (Skia joins neighbours of the same style).
fn han_pieces<'t>(
    stretch: &'t str,
    from: usize,
    halts: &std::collections::HashSet<usize>,
) -> Vec<(&'t str, &'static [(&'static str, i32)])> {
    let chars: Vec<(usize, char)> = stretch.char_indices().collect();
    let halt = |i: usize| halts.contains(&(from + chars[i].0));
    let mut out = Vec::new();
    let mut start = 0;
    let mut halted = false;
    for i in 0..chars.len() {
        halted |= halt(i);
        let end = chars.get(i + 1).map_or(stretch.len(), |c| c.0);
        let cut = match chars.get(i + 1) {
            None => true,
            Some((_, next)) => {
                let (a, b) = (chars[i].1, *next);
                let line_start = from + chars[i].0 == 0;
                let kana = is_kana(a) || is_kana(b);
                match (halt(i), halt(i + 1)) {
                    (true, true) => false,
                    // A mark that is halved still kerns with the kana before it.
                    (false, true) | (true, false) => {
                        let plain = if halt(i) { b } else { a };
                        !(is_kana(plain) && plain != '\u{30fb}' && han_joined(a, b, line_start))
                    }
                    (false, false) => kana && !han_joined(a, b, line_start),
                }
            }
        };
        if cut {
            let features: &[(&str, i32)] = match (start == i, halted) {
                (_, true) => &[("halt", 1)],
                (true, false) => &[("kern", 0)],
                (false, false) => &[],
            };
            out.push((&stretch[chars[start].0..end], features));
            start = i + 1;
            halted = false;
        }
    }
    out
}

/// One fallback font file (from `fallback-fonts.sh`), in the order they follow the board's faces.
#[derive(Debug)]
pub struct FallbackFile {
    pub name: &'static str,
    /// The face of a font collection (`.ttc`); 0 for a plain font file.
    pub index: u32,
    /// The colour emoji face: sequences (ZWJ, skin tones, flags, keycaps, emoji presentation) are
    /// kept whole in it.
    pub emoji: bool,
    /// A CJK face, laid out as Chromium does (`han`).
    pub han: bool,
}

const fn file(name: &'static str) -> FallbackFile {
    FallbackFile {
        name,
        index: 0,
        emoji: false,
        han: false,
    }
}

/// The fonts behind the board's six, in the order they are tried (versions: `fallback-fonts.sh`).
/// Noto Sans CJK is a collection of five faces (JP, KR, SC, TC, HK); the Simplified Chinese one
/// (index 2) is used for every Han character: it has the Han set of the others, the most readers
/// know its shapes, and a line cannot say which language it is in. Kana and Hangul are in every
/// face.
pub const FALLBACK_FONT_FILES: [FallbackFile; 10] = [
    file("NotoSans-Regular.ttf"),
    file("NotoSansSymbols-Regular.ttf"),
    FallbackFile {
        name: "NotoSansCJK-Regular.ttc",
        index: 2,
        emoji: false,
        han: true,
    },
    file("NotoSansThai-Regular.ttf"),
    file("NotoSansLao-Regular.ttf"),
    file("NotoSansKhmer-Regular.ttf"),
    file("NotoSansArabic-Regular.ttf"),
    file("NotoSansHebrew-Regular.ttf"),
    file("NotoSansDevanagari-Regular.ttf"),
    FallbackFile {
        name: "Noto-COLRv1.ttf",
        index: 0,
        emoji: true,
        han: false,
    },
];

/// A fallback face's bytes (see [`FALLBACK_FONT_FILES`]).
pub struct FallbackFace<'a> {
    pub bytes: &'a [u8],
    pub index: u32,
    pub emoji: bool,
    pub han: bool,
}

impl std::fmt::Debug for FallbackFace<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackFace")
            .field("bytes", &self.bytes.len())
            .field("index", &self.index)
            .field("emoji", &self.emoji)
            .field("han", &self.han)
            .finish()
    }
}

/// The fonts the board's text is drawn with: the faces in the order they are tried for each glyph.
pub struct FontSet {
    collection: FontCollection,
    /// The same faces at other weights (the board's are variable in weight), made when asked for.
    weighted: RefCell<HashMap<u16, FontCollection>>,
    families: Vec<String>,
    typefaces: Vec<Typeface>,
    first: Typeface,
    /// The colour emoji face, if there is one.
    emoji: Option<usize>,
    /// The CJK face and what its glyphs say about its punctuation.
    han: Option<(usize, HanData)>,
}

/// Characters a text engine hides when the font has no glyph for them (joiners, variation
/// selectors, bidi controls, tags): they are never a missing glyph and stay with their base.
fn default_ignorable(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{115f}'..='\u{1160}' | '\u{17b4}'..='\u{17b5}'
        | '\u{180b}'..='\u{180f}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{206f}' | '\u{3164}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}'
        | '\u{ffa0}' | '\u{fff0}'..='\u{fff8}' | '\u{e0000}'..='\u{e0fff}')
}

/// What makes a cluster an emoji sequence rather than text: a joiner, emoji presentation, a skin
/// tone, a keycap, a tag, or a pair of regional indicators.
fn emoji_marker(c: char) -> bool {
    matches!(c, '\u{200d}' | '\u{fe0f}' | '\u{20e3}' | '\u{1f3fb}'..='\u{1f3ff}' | '\u{e0020}'..='\u{e007f}'
        | '\u{1f1e6}'..='\u{1f1ff}')
}

impl std::fmt::Debug for FontSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontSet")
            .field("families", &self.families)
            .finish()
    }
}

impl FontSet {
    /// `faces` are font files (woff2, TrueType or OpenType, possibly variable), most preferred
    /// first. Each is its own family: a glyph is taken from the first face that has it. The board's
    /// subsets hold only the characters their `unicode-range` gives them, so the order alone makes
    /// the same choice as the browser. Nothing else is ever consulted (no system fonts).
    pub fn new(faces: &[&[u8]]) -> Result<Rc<Self>, SkiaError> {
        Self::with_fallbacks(faces, &[])
    }

    /// [`FontSet::new`] with the fallback faces after the board's: a character comes from a
    /// fallback face only when no board face has it, so everything the board fonts cover is drawn
    /// as it is without them.
    pub fn with_fallbacks(
        faces: &[&[u8]],
        fallbacks: &[FallbackFace],
    ) -> Result<Rc<Self>, SkiaError> {
        let mgr = FontMgr::default();
        let mut provider = TypefaceFontProvider::new();
        let mut families = Vec::new();
        let mut typefaces = Vec::new();
        let mut first = None;
        let mut emoji = None;
        let mut han = None;
        let all = faces.iter().map(|bytes| (*bytes, None, false, true)).chain(
            fallbacks
                .iter()
                .map(|f| (f.bytes, Some(f.index), f.emoji, f.han)),
        );
        for (i, (bytes, index, is_emoji, is_han)) in all.enumerate() {
            let typeface = single_face(bytes, index.unwrap_or(0))
                .and_then(|face| mgr.new_from_data(Data::new_copy(&face), None))
                .ok_or(SkiaError::Font(i))?;
            // Colour emoji are COLRv1 (or bitmaps), which only a FreeType font manager draws:
            // macOS's opens the file and draws nothing. A face that draws nothing is left out, so
            // that its characters are counted as missing glyphs instead of vanishing.
            if is_emoji && !draws_something(&typeface) {
                continue;
            }
            if is_emoji {
                emoji = Some(typefaces.len());
            }
            if is_han {
                han = Some((typefaces.len(), HanData::of(&typeface)));
            }
            let alias = format!("board-face-{}", typefaces.len());
            provider.register_typeface(typeface.clone(), alias.as_str());
            families.push(alias);
            typefaces.push(typeface.clone());
            first.get_or_insert(typeface);
        }
        let first = first.ok_or(SkiaError::NoFonts)?;
        let mut collection = FontCollection::new();
        collection.set_asset_font_manager(Some(provider.into()));
        collection.disable_font_fallback();
        Ok(Rc::new(Self {
            collection,
            weighted: RefCell::default(),
            families,
            typefaces,
            first,
            emoji,
            han,
        }))
    }

    /// The text cut into stretches that each come from one face, as the browser cuts it: a
    /// character comes from the first face whose `unicode-range` has it (here: whose character map
    /// has it, the subsets hold exactly their ranges), and kerning, ligatures and mark attachment
    /// stop at the cut. Shaping the whole line with the first face and falling back for what it
    /// lacks would not cut there: HarfBuzz builds a precomposed letter the face lacks out of its
    /// base and mark when the face has both (the browser would take the whole letter from the next
    /// face), and kerns it with its neighbours. A cluster (a base with its combining marks, joiners
    /// and variation selectors) is never cut: its marks stay with the base's face when that face has
    /// them, and an emoji sequence (a joiner, emoji presentation, a skin tone, a keycap, a flag)
    /// comes whole from the emoji face when it has the first character. A character no face has
    /// stays in a stretch of its own with the marks that follow it, which tries every face (and
    /// ends up as the first face's missing-glyph box). Each stretch is its text and the index of its face in the order the
    /// faces were given (the board's, then the fallbacks), or `None` for such a character.
    pub fn stretches<'t>(&self, text: &'t str) -> Vec<(&'t str, Option<usize>)> {
        let has = |face: usize, c: char| self.typefaces[face].unichar_to_glyph(c as i32) != 0;
        let find = |c: char| (0..self.typefaces.len()).find(|&f| has(f, c));
        let mut out: Vec<(&str, Option<usize>)> = Vec::new();
        let mut start = 0;
        let mut current: Option<Option<usize>> = None;
        let mut cut = |at: usize, face: Option<usize>, out: &mut Vec<(&'t str, Option<usize>)>| {
            if current != Some(face) {
                if let Some(face_before) = current {
                    out.push((&text[start..at], face_before));
                }
                start = at;
                current = Some(face);
            }
        };
        for (g, cluster) in text.grapheme_indices(true) {
            let mut chars = cluster.char_indices();
            let Some((_, first)) = chars.next() else {
                continue;
            };
            let sequence = cluster.chars().nth(1).is_some()
                && cluster.chars().any(emoji_marker)
                && self.emoji.is_some_and(|e| has(e, first));
            let base = if sequence { self.emoji } else { find(first) };
            cut(g, base, &mut out);
            for (i, c) in chars {
                let face = match base {
                    Some(f) if sequence || has(f, c) || default_ignorable(c) => Some(f),
                    Some(_) => find(c),
                    None => None,
                };
                cut(g + i, face, &mut out);
            }
        }
        if let Some(face) = current {
            out.push((&text[start..], face));
        }
        out
    }

    /// The characters (byte offsets) of CJK stretches Chromium draws at half width, with `halt`:
    /// an opening mark after another mark, a closing mark before another (`HanKerning` for a font
    /// without `chws`, as Noto Sans CJK is). Only neighbours inside the stretch count, and the start
    /// of the line is not trimmed.
    fn halts(
        &self,
        text: &str,
        stretches: &[(&str, Option<usize>)],
    ) -> std::collections::HashSet<usize> {
        let mut halts = std::collections::HashSet::new();
        let Some((han, data)) = &self.han else {
            return halts;
        };
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let kind = |i: usize| data.class(chars[i].1);
        let at = |byte: usize| chars.partition_point(|c| c.0 < byte);
        for (stretch, face) in stretches {
            if *face != Some(*han) {
                continue;
            }
            let from = stretch.as_ptr() as usize - text.as_ptr() as usize;
            let (start, end) = (at(from), at(from + stretch.len()));
            let mut last = kind(start);
            for i in start + 1..end {
                let class = kind(i);
                if han_kern_close(class, last) {
                    halts.insert(chars[i - 1].0);
                } else if han_kern_open(class, last) {
                    halts.insert(chars[i].0);
                }
                last = class;
            }
        }
        halts
    }

    /// The faces at `weight`: a variable face is set to it on its weight axis, any other stays as
    /// it is.
    fn collection_at(&self, weight: u16) -> FontCollection {
        if weight == REGULAR {
            return self.collection.clone();
        }
        self.weighted
            .borrow_mut()
            .entry(weight)
            .or_insert_with(|| {
                let coordinates = [Coordinate {
                    axis: FourByteTag::from_chars('w', 'g', 'h', 't'),
                    value: f32::from(weight),
                }];
                let arguments =
                    FontArguments::new().set_variation_design_position(VariationPosition {
                        coordinates: &coordinates,
                    });
                let mut provider = TypefaceFontProvider::new();
                for (typeface, family) in self.typefaces.iter().zip(&self.families) {
                    let face = typeface
                        .clone_with_arguments(&arguments)
                        .unwrap_or_else(|| typeface.clone());
                    provider.register_typeface(face, family.as_str());
                }
                let mut collection = FontCollection::new();
                collection.set_asset_font_manager(Some(provider.into()));
                collection.disable_font_fallback();
                collection
            })
            .clone()
    }

    /// A laid-out line. `foreground` paints the glyphs instead of `color` (an outline).
    fn paragraph(
        &self,
        text: &str,
        size: f64,
        weight: u16,
        color: Color,
        foreground: Option<&Paint>,
    ) -> Paragraph {
        // Canvas text replaces tabs, form feeds and carriage returns with a space before it is
        // measured or drawn; no face has a glyph for them, and what a text engine does then (the
        // missing-glyph box on one platform, a space on another) is not the board's.
        let text = text.replace(['\t', '\u{c}', '\r'], " ");
        let text = text.as_str();
        let style = |families: &[String], features: &[(&str, i32)]| {
            let mut style = TextStyle::new();
            for (tag, value) in features {
                style.add_font_feature(tag, *value);
            }
            style.set_font_size(size as f32);
            style.set_font_families(families);
            style.set_color(color);
            if let Some(paint) = foreground {
                style.set_foreground_paint(paint);
            }
            style
        };
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_style(&style(&self.families, &[]));
        paragraph_style.set_apply_rounding_hack(false);
        let mut builder = ParagraphBuilder::new(&paragraph_style, self.collection_at(weight));
        let stretches = self.stretches(text);
        let halts = self.halts(text, &stretches);
        for (stretch, face) in stretches {
            match face {
                Some(f) if self.han.as_ref().is_some_and(|(h, _)| *h == f) => {
                    let from = stretch.as_ptr() as usize - text.as_ptr() as usize;
                    for (piece, features) in han_pieces(stretch, from, &halts) {
                        builder.push_style(&style(&self.families[f..=f], features));
                        builder.add_text(piece);
                        builder.pop();
                    }
                    continue;
                }
                Some(f) => builder.push_style(&style(&self.families[f..=f], &[])),
                None => builder.push_style(&style(&self.families, &[])),
            };
            builder.add_text(stretch);
            builder.pop();
        }
        let mut paragraph = builder.build();
        // Never wrapped: a line of the board ends where its text does.
        paragraph.layout(1.0e7);
        paragraph
    }

    /// Whether the colour emoji face is in use: it is left out where the font manager cannot
    /// draw it (macOS), and then emoji are missing glyphs.
    pub fn has_emoji_face(&self) -> bool {
        self.emoji.is_some()
    }

    /// How many glyphs of `text` at `size` are a face's missing-glyph box (glyph 0): what the
    /// reader would see as an empty rectangle.
    pub fn missing_glyphs(&self, text: &str, size: f64) -> usize {
        let mut paragraph = self.paragraph(text, size, REGULAR, Color::BLACK, None);
        let mut missing = 0;
        paragraph.visit(|_, info| {
            if let Some(info) = info {
                missing += info.glyphs().iter().filter(|g| **g == 0).count();
            }
        });
        missing
    }

    /// How far the em box reaches above and below the baseline: the font's ascent and descent
    /// scaled so that they make up one em (what `measureText` calls `emHeightAscent` and
    /// `emHeightDescent`). The top of a `textBaseline = 'top'` line is the first.
    fn em_extents(&self, size: f64) -> (f64, f64) {
        let (_, m) = Font::from_typeface(&self.first, size as f32).metrics();
        let (ascent, descent) = (f64::from(-m.ascent), f64::from(m.descent));
        (
            size * ascent / (ascent + descent),
            size * descent / (ascent + descent),
        )
    }
}

/// The weight of text the board sets with `font = "{size}px ..."`.
const REGULAR: u16 = 400;

/// What a browser does to a face that has no italic: slants it a quarter, about the baseline.
const FAKE_ITALIC_SKEW: f32 = -0.25;

/// Where the glyphs of a laid-out line sit below its top: the text layout rounds the line's ascent
/// to whole pixels when it places the baseline, so `alphabetic_baseline` as it is would put the
/// text up to half a pixel off.
fn paragraph_baseline(paragraph: &Paragraph) -> f64 {
    f64::from(paragraph.alphabetic_baseline()).round()
}

/// `measureText` memoized per (size, line).
struct SkiaMeasure {
    fonts: Rc<FontSet>,
    cache: RefCell<HashMap<(u64, String), f64>>,
}

impl MeasureText for SkiaMeasure {
    fn width(&self, line: &str, font_size: f64) -> f64 {
        let key = (font_size.to_bits(), line.to_owned());
        if let Some(w) = self.cache.borrow().get(&key) {
            return *w;
        }
        let w = f64::from(
            self.fonts
                .paragraph(line, font_size, REGULAR, Color::BLACK, None)
                .max_intrinsic_width(),
        );
        let mut cache = self.cache.borrow_mut();
        if cache.len() > 5000 {
            cache.clear();
        }
        cache.insert(key, w);
        w
    }
}

#[derive(Clone)]
struct Style {
    fill: Rgba,
    stroke: Rgba,
    line_width: f64,
    cap: LineCap,
    join: LineJoin,
    dash: Vec<f64>,
    alpha: f64,
    font_size: f64,
    weight: u16,
    italic: bool,
    align: TextAlign,
    baseline: TextBaseline,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fill: Rgba::rgb(0, 0, 0),
            stroke: Rgba::rgb(0, 0, 0),
            line_width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            dash: Vec::new(),
            alpha: 1.0,
            font_size: 10.0,
            weight: REGULAR,
            italic: false,
            align: TextAlign::Left,
            baseline: TextBaseline::Top,
        }
    }
}

/// A CPU raster canvas.
pub struct SkiaCanvas {
    surface: Surface,
    fonts: Rc<FontSet>,
    measure: Rc<SkiaMeasure>,
    style: Style,
    saved: Vec<Style>,
    text_log: Option<Vec<TextBox>>,
}

/// Where a line of text was drawn: its text and its box in canvas units (the em box, from the
/// left of the line to its right, widened for the slant of an italic), what `measureText` and the
/// text's alignment and baseline give on the web side.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBox {
    pub text: String,
    pub bounds: [f64; 4],
}

impl std::fmt::Debug for SkiaCanvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkiaCanvas")
            .field("size", &(self.surface.width(), self.surface.height()))
            .finish()
    }
}

fn color_of(c: Rgba, alpha: f64) -> Color {
    let a = (f64::from(c.a) * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::from_argb(a, c.r, c.g, c.b)
}

impl SkiaCanvas {
    /// A transparent canvas of `width` x `height` device pixels.
    pub fn new(width: u32, height: u32, fonts: Rc<FontSet>) -> Result<Self, SkiaError> {
        let surface = surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or(SkiaError::Surface(width, height))?;
        let measure = Rc::new(SkiaMeasure {
            fonts: Rc::clone(&fonts),
            cache: RefCell::default(),
        });
        Ok(Self {
            surface,
            fonts,
            measure,
            style: Style::default(),
            saved: Vec::new(),
            text_log: None,
        })
    }

    /// Starts writing down the box of every line filled from now on (see [`TextBox`]).
    pub fn log_text(&mut self) {
        self.text_log = Some(Vec::new());
    }

    /// The boxes written down since `log_text` or the last call.
    pub fn take_text_boxes(&mut self) -> Vec<TextBox> {
        self.text_log
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// The pixels as 8-bit RGBA (not premultiplied), row by row.
    pub fn rgba(&mut self) -> Vec<u8> {
        let (w, h) = (self.surface.width(), self.surface.height());
        let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let ok = self
            .surface
            .read_pixels(&info, &mut pixels, (w * 4) as usize, (0, 0));
        assert!(ok, "reading the surface's pixels");
        pixels
    }

    /// The canvas as a PNG file.
    pub fn png(&mut self) -> Vec<u8> {
        let image = self.surface.image_snapshot();
        image
            .encode(None, EncodedImageFormat::PNG, None)
            .expect("PNG encoding of a raster image")
            .as_bytes()
            .to_vec()
    }

    fn paint(&self, style: PaintStyle) -> Paint {
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_style(style);
        match style {
            PaintStyle::Stroke => {
                p.set_color(color_of(self.style.stroke, self.style.alpha));
                p.set_stroke_width(self.style.line_width as f32);
                p.set_stroke_cap(match self.style.cap {
                    LineCap::Butt => PaintCap::Butt,
                    LineCap::Round => PaintCap::Round,
                    LineCap::Square => PaintCap::Square,
                });
                p.set_stroke_join(match self.style.join {
                    LineJoin::Miter => PaintJoin::Miter,
                    LineJoin::Round => PaintJoin::Round,
                    LineJoin::Bevel => PaintJoin::Bevel,
                });
                if !self.style.dash.is_empty() {
                    let mut intervals: Vec<f32> =
                        self.style.dash.iter().map(|d| *d as f32).collect();
                    // An odd list is repeated, as the web API does.
                    if intervals.len() % 2 == 1 {
                        intervals.extend_from_within(..);
                    }
                    p.set_path_effect(PathEffect::dash(&intervals, 0.0));
                }
            }
            _ => {
                p.set_color(color_of(self.style.fill, self.style.alpha));
            }
        }
        p
    }

    /// `fillText` and `strokeText`: the line placed by the text's alignment and baseline,
    /// slanted when the font is italic (the board's faces have no italic, so a browser slants
    /// them), painted in `color` or, for an outline, with `foreground`.
    fn draw_text(&mut self, text: &str, x: f64, y: f64, color: Color, foreground: Option<&Paint>) {
        // Canvas 2D draws nothing at a place that is not a number.
        if text.is_empty() || !x.is_finite() || !y.is_finite() {
            return;
        }
        let size = self.style.font_size;
        let paragraph = self
            .fonts
            .paragraph(text, size, self.style.weight, color, foreground);
        let width = f64::from(paragraph.max_intrinsic_width());
        let left = match self.style.align {
            TextAlign::Left => x,
            TextAlign::Center => x - width / 2.0,
            TextAlign::Right => x - width,
        };
        let (ascent, descent) = self.fonts.em_extents(size);
        let baseline = y + match self.style.baseline {
            TextBaseline::Top => ascent,
            TextBaseline::Middle => (ascent - descent) / 2.0,
            TextBaseline::Bottom => -descent,
        };
        let top = baseline - paragraph_baseline(&paragraph);
        // A stroke is the halo of a line that is filled too: only the fill is a box.
        if let (None, Some(log)) = (foreground, &mut self.text_log) {
            let slant = if self.style.italic { size * 0.25 } else { 0.0 };
            log.push(TextBox {
                text: text.to_owned(),
                bounds: [
                    left,
                    baseline - ascent,
                    left + width + slant,
                    baseline + descent,
                ],
            });
        }
        let canvas = self.surface.canvas();
        if self.style.italic {
            canvas.save();
            canvas.concat(&skia_safe::Matrix::new_all(
                1.0,
                FAKE_ITALIC_SKEW,
                -FAKE_ITALIC_SKEW * baseline as f32,
                0.0,
                1.0,
                0.0,
                0.0,
                0.0,
                1.0,
            ));
        }
        paragraph.paint(canvas, (left as f32, top as f32));
        if self.style.italic {
            canvas.restore();
        }
    }

    fn rect_path(x: f64, y: f64, w: f64, h: f64) -> skia_safe::Path {
        let mut b = PathBuilder::new();
        b.add_rect(
            Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
            None,
            None,
        );
        b.detach()
    }
}

/// A full turn or more is drawn as two half turns (Skia treats a sweep of 360 degrees as none).
fn add_ellipse(b: &mut PathBuilder, c: (f64, f64), r: (f64, f64), start: f64, end: f64, ccw: bool) {
    use std::f64::consts::TAU;
    let oval = Rect::new(
        (c.0 - r.0) as f32,
        (c.1 - r.1) as f32,
        (c.0 + r.0) as f32,
        (c.1 + r.1) as f32,
    );
    let delta = end - start;
    let sweep = if !ccw && delta >= TAU {
        TAU
    } else if ccw && -delta >= TAU {
        -TAU
    } else if !ccw {
        delta.rem_euclid(TAU)
    } else {
        -(-delta).rem_euclid(TAU)
    };
    let start_deg = start.to_degrees() as f32;
    if sweep.abs() >= TAU {
        let half = (sweep.signum() * 180.0) as f32;
        b.arc_to(oval, start_deg, half, false);
        b.arc_to(oval, start_deg + half, half, false);
    } else {
        b.arc_to(oval, start_deg, sweep.to_degrees() as f32, false);
    }
}

fn to_skia(path: &Path) -> skia_safe::Path {
    let mut b = PathBuilder::new();
    for op in path.ops() {
        match *op {
            PathOp::MoveTo(x, y) => {
                b.move_to((x as f32, y as f32));
            }
            PathOp::LineTo(x, y) => {
                b.line_to((x as f32, y as f32));
            }
            PathOp::QuadraticCurveTo { cx, cy, x, y } => {
                b.quad_to((cx as f32, cy as f32), (x as f32, y as f32));
            }
            PathOp::Ellipse {
                cx,
                cy,
                rx,
                ry,
                start,
                end,
                ccw,
            } => {
                add_ellipse(&mut b, (cx, cy), (rx, ry), start, end, ccw);
            }
            PathOp::RoundRect { x, y, w, h, radius } => {
                let rect = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32);
                b.add_rrect(
                    RRect::new_rect_xy(rect, radius as f32, radius as f32),
                    PathDirection::CW,
                    None,
                );
            }
            PathOp::Rect { x, y, w, h } => {
                b.add_rect(
                    Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
                    None,
                    None,
                );
            }
            PathOp::ClosePath => {
                b.close();
            }
        }
    }
    b.detach()
}

/// A picture, decoded from an encoded file (PNG, JPEG), for a canvas of this backend.
pub fn decode_image(bytes: &[u8]) -> Option<Image> {
    Image::from_encoded(Data::new_copy(bytes))
}

/// A PNG (or other encoded picture) as 8-bit RGBA, not premultiplied: width, height, pixels.
pub fn decode_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let image = decode_image(bytes)?;
    let (w, h) = (image.width(), image.height());
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    image
        .read_pixels(
            &info,
            &mut pixels,
            (w * 4) as usize,
            (0, 0),
            skia_safe::image::CachingHint::Allow,
        )
        .then_some((w as u32, h as u32, pixels))
}

/// 8-bit RGBA pixels (not premultiplied) as a PNG file.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let image =
        skia_safe::images::raster_from_data(&info, Data::new_copy(rgba), (width * 4) as usize)?;
    image
        .encode(None, EncodedImageFormat::PNG, None)
        .map(|d| d.as_bytes().to_vec())
}

impl Canvas for SkiaCanvas {
    type Image = Image;

    fn size(&self) -> (u32, u32) {
        (self.surface.width() as u32, self.surface.height() as u32)
    }

    fn offscreen(&self, width: u32, height: u32) -> Self {
        Self::new(width, height, Rc::clone(&self.fonts)).expect("an offscreen surface")
    }

    fn image(&mut self) -> Image {
        self.surface.image_snapshot()
    }

    fn image_size(image: &Image) -> (f64, f64) {
        (f64::from(image.width()), f64::from(image.height()))
    }

    fn measurer(&self) -> Rc<dyn MeasureText> {
        Rc::clone(&self.measure) as Rc<dyn MeasureText>
    }

    fn save(&mut self) {
        self.surface.canvas().save();
        self.saved.push(self.style.clone());
    }

    fn restore(&mut self) {
        if let Some(style) = self.saved.pop() {
            self.style = style;
            self.surface.canvas().restore();
        }
    }

    fn set_transform(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        let m = skia_safe::Matrix::new_all(
            a as f32, c as f32, e as f32, b as f32, d as f32, f as f32, 0.0, 0.0, 1.0,
        );
        let canvas = self.surface.canvas();
        canvas.reset_matrix();
        canvas.concat(&m);
    }

    fn clip(&mut self, path: &Path) {
        self.surface
            .canvas()
            .clip_path(&to_skia(path), ClipOp::Intersect, true);
    }

    fn set_fill_style(&mut self, color: Rgba) {
        self.style.fill = color;
    }

    fn set_stroke_style(&mut self, color: Rgba) {
        self.style.stroke = color;
    }

    fn set_line_width(&mut self, width: f64) {
        self.style.line_width = width;
    }

    fn set_line_cap(&mut self, cap: LineCap) {
        self.style.cap = cap;
    }

    fn set_line_join(&mut self, join: LineJoin) {
        self.style.join = join;
    }

    fn set_line_dash(&mut self, dash: &[f64]) {
        self.style.dash = dash.to_vec();
    }

    fn set_global_alpha(&mut self, alpha: f64) {
        self.style.alpha = alpha;
    }

    fn set_font(&mut self, italic: bool, weight: u16, size: f64) {
        self.style.italic = italic;
        self.style.weight = weight;
        self.style.font_size = size;
    }

    fn set_text_align(&mut self, align: TextAlign) {
        self.style.align = align;
    }

    fn set_text_baseline(&mut self, baseline: TextBaseline) {
        self.style.baseline = baseline;
    }

    fn scale(&mut self) -> f64 {
        let m = self.surface.canvas().local_to_device_as_3x3();
        let scale = f64::from(m.scale_x()).hypot(f64::from(m.skew_y()));
        if scale == 0.0 { 1.0 } else { scale }
    }

    fn fill(&mut self, path: &Path) {
        let paint = self.paint(PaintStyle::Fill);
        self.surface.canvas().draw_path(&to_skia(path), &paint);
    }

    fn stroke(&mut self, path: &Path) {
        let paint = self.paint(PaintStyle::Stroke);
        self.surface.canvas().draw_path(&to_skia(path), &paint);
    }

    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let paint = self.paint(PaintStyle::Fill);
        self.surface
            .canvas()
            .draw_path(&Self::rect_path(x, y, w, h), &paint);
    }

    fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let paint = self.paint(PaintStyle::Stroke);
        self.surface
            .canvas()
            .draw_path(&Self::rect_path(x, y, w, h), &paint);
    }

    fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let mut paint = Paint::default();
        paint.set_blend_mode(skia_safe::BlendMode::Clear);
        self.surface.canvas().draw_rect(
            Rect::from_xywh(x as f32, y as f32, w as f32, h as f32),
            &paint,
        );
    }

    fn draw_image(&mut self, image: &Image, x: f64, y: f64, w: f64, h: f64) {
        let mut paint = Paint::default();
        paint.set_alpha_f(self.style.alpha as f32);
        let dst = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32);
        self.surface.canvas().draw_image_rect_with_sampling_options(
            image,
            None,
            dst,
            SamplingOptions::new(FilterMode::Linear, MipmapMode::None),
            &paint,
        );
    }

    fn fill_text(&mut self, text: &str, x: f64, y: f64) {
        let color = color_of(self.style.fill, self.style.alpha);
        self.draw_text(text, x, y, color, None);
    }

    fn stroke_text(&mut self, text: &str, x: f64, y: f64) {
        let paint = self.paint(PaintStyle::Stroke);
        self.draw_text(text, x, y, paint.color(), Some(&paint));
    }
}
