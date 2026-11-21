//! Text as Chromium lays it out: Inter at the weights the screens use (variable-font instances of the
//! board's faces), a fixed line height with the leading split as CSS splits it, wrapping, `text-wrap:
//! balance` and one-line ellipsis. Faces are cut per character from the board's face order, as the
//! board's text is (`FontSet::stretches`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use board_draw::FontSet;
use skia_safe::font_arguments::{VariationPosition, variation_position::Coordinate};
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, StrutStyle, TextAlign, TextStyle,
    TypefaceFontProvider,
};
use skia_safe::{Font, FontArguments, FourByteTag, Typeface};

use crate::color::Rgba;

/// The weights the screens use (`font-medium` to `font-black`, and normal).
const WEIGHTS: [u16; 6] = [400, 500, 600, 700, 800, 900];

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub weight: u16,
    pub color: Rgba,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    pub spans: Vec<Span>,
    pub size: f32,
    /// The line height in px.
    pub line_height: f32,
    /// `tabular-nums`.
    pub tabular: bool,
    pub center: bool,
    /// `truncate` (one line, an ellipsis) when set to 1.
    pub max_lines: Option<usize>,
    /// `text-balance`.
    pub balance: bool,
}

impl TextSpec {
    pub fn plain(text: impl Into<String>, size: f32, line_height: f32, weight: u16, color: Rgba) -> Self {
        Self {
            spans: vec![Span { text: text.into(), weight, color }],
            size,
            line_height,
            tabular: false,
            center: false,
            max_lines: None,
            balance: false,
        }
    }

    pub fn tab(mut self) -> Self {
        self.tabular = true;
        self
    }
    pub fn center(mut self) -> Self {
        self.center = true;
        self
    }
    /// `truncate`.
    pub fn one_line(mut self) -> Self {
        self.max_lines = Some(1);
        self
    }
    pub fn balanced(mut self) -> Self {
        self.balance = true;
        self
    }

    fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// A paragraph laid out at one width.
pub struct Laid {
    pub paragraph: Paragraph,
    pub width: f32,
    pub height: f32,
    pub lines: usize,
    /// Where the first baseline is below the paragraph's top in Chromium, minus where Skia puts it.
    pub shift: f32,
    /// Chromium's content area of a line (its font's rounded ascent and descent) and where it starts in the line.
    pub content_h: f32,
    pub half_leading: f32,
}

impl std::fmt::Debug for Laid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Laid").field("width", &self.width).field("lines", &self.lines).finish()
    }
}

type Key = (String, [u32; 3], Vec<(u16, [u8; 4])>, u8, usize, u32);

pub struct TextEngine {
    fonts: Rc<FontSet>,
    collection: FontCollection,
    /// Per weight, the family alias of each face, in the board's face order.
    families: Vec<Vec<String>>,
    /// Chromium's ascent and descent of the first face at each weight (rounded as Blink does), by size.
    cache: RefCell<HashMap<Key, Rc<Laid>>>,
}

impl std::fmt::Debug for TextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEngine").finish_non_exhaustive()
    }
}

fn instance(face: &Typeface, weight: u16) -> Typeface {
    let coords = [Coordinate { axis: FourByteTag::from(('w', 'g', 'h', 't')), value: f32::from(weight) }];
    let position = VariationPosition { coordinates: &coords };
    let args = FontArguments::new().set_variation_design_position(position);
    face.clone_with_arguments(&args).unwrap_or_else(|| face.clone())
}

impl TextEngine {
    pub fn new(fonts: Rc<FontSet>) -> Rc<Self> {
        let mut provider = TypefaceFontProvider::new();
        let mut families = Vec::new();
        for weight in WEIGHTS {
            let mut names = Vec::new();
            for (i, face) in fonts.typefaces().iter().enumerate() {
                let alias = format!("quiz-{weight}-{i}");
                provider.register_typeface(instance(face, weight), alias.as_str());
                names.push(alias);
            }
            families.push(names);
        }
        let mut collection = FontCollection::new();
        collection.set_asset_font_manager(Some(provider.into()));
        collection.disable_font_fallback();
        Rc::new(Self { fonts, collection, families, cache: RefCell::new(HashMap::new()) })
    }

    fn weight_index(weight: u16) -> usize {
        WEIGHTS.iter().position(|w| *w == weight).unwrap_or_else(|| panic!("weight {weight}"))
    }

    /// Chromium's ascent and descent of a face at a size: each rounded to whole pixels.
    /// How many glyphs of `text` at `size` no face has.
    pub fn missing(&self, text: &str, size: f32) -> usize {
        self.fonts.missing_glyphs(text, f64::from(size))
    }

    fn metrics(&self, weight: u16, size: f32) -> (f32, f32) {
        let face = instance(&self.fonts.typefaces()[0], weight);
        let (_, m) = Font::from_typeface(face, size).metrics();
        ((-m.ascent).round(), m.descent.round())
    }

    fn build(&self, spec: &TextSpec) -> Paragraph {
        let first_weight = spec.spans.first().map_or(400, |s| s.weight);
        let style_for = |weight: u16, family: &[String], color: Rgba| {
            let mut style = TextStyle::new();
            style.set_font_size(spec.size);
            style.set_font_families(family);
            style.set_color(color.skia(1.0));
            style.set_height(spec.line_height / spec.size);
            style.set_height_override(true);
            style.set_half_leading(true);
            if spec.tabular {
                style.add_font_feature("tnum", 1);
            }
            let _ = weight;
            style
        };
        let all = &self.families[Self::weight_index(first_weight)];
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_style(&style_for(first_weight, all, Rgba::BLACK));
        paragraph_style.set_apply_rounding_hack(false);
        let mut strut = StrutStyle::new();
        strut.set_font_families(&all[..1]);
        strut.set_font_size(spec.size);
        strut.set_height_override(true);
        strut.set_height(spec.line_height / spec.size);
        strut.set_half_leading(true);
        strut.set_strut_enabled(true);
        strut.set_force_strut_height(true);
        paragraph_style.set_strut_style(strut);
        paragraph_style.set_text_align(if spec.center { TextAlign::Center } else { TextAlign::Left });
        if let Some(n) = spec.max_lines {
            paragraph_style.set_max_lines(n);
            paragraph_style.set_ellipsis("…");
        }
        let mut builder = ParagraphBuilder::new(&paragraph_style, &self.collection);
        for span in &spec.spans {
            let fam = &self.families[Self::weight_index(span.weight)];
            for (stretch, face) in self.fonts.stretches(&span.text) {
                let names = match face {
                    Some(f) => &fam[f..=f],
                    None => &fam[..],
                };
                builder.push_style(&style_for(span.weight, names, span.color));
                builder.add_text(stretch);
                builder.pop();
            }
        }
        builder.build()
    }

    fn lay_out(&self, spec: &TextSpec, width: f32) -> Laid {
        let mut paragraph = self.build(spec);
        paragraph.layout(width);
        if spec.balance && paragraph.line_number() > 1 && paragraph.line_number() <= 6 {
            // Chromium balances by taking the narrowest width that keeps the line count.
            let lines = paragraph.line_number();
            let (mut lo, mut hi) = (0.0f32, width);
            for _ in 0..24 {
                let mid = (lo + hi) / 2.0;
                paragraph.layout(mid);
                if paragraph.line_number() <= lines {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            paragraph.layout(hi.ceil().min(width));
            while paragraph.line_number() > lines {
                let w = (hi + 1.0).min(width);
                hi = w;
                paragraph.layout(w);
            }
        }
        let lines = paragraph.line_number();
        let (ascent, descent) = self.metrics(spec.spans.first().map_or(400, |s| s.weight), spec.size);
        let half = ((spec.line_height - (ascent + descent)) / 2.0).floor();
        let want = ascent + half;
        let have = paragraph.get_line_metrics().first().map_or(want, |l| l.baseline as f32);
        Laid {
            width: paragraph.longest_line(),
            height: lines as f32 * spec.line_height,
            lines,
            shift: want - have,
            content_h: ascent + descent,
            half_leading: half,
            paragraph,
        }
    }

    /// The text laid out in `width` (px), memoised.
    pub fn laid(&self, spec: &TextSpec, width: f32) -> Rc<Laid> {
        let key: Key = (
            spec.text(),
            [spec.size.to_bits(), spec.line_height.to_bits(), width.to_bits()],
            spec.spans.iter().map(|s| (s.weight, [s.color.r, s.color.g, s.color.b, (s.color.a * 255.0) as u8])).collect(),
            u8::from(spec.tabular) | u8::from(spec.center) << 1 | u8::from(spec.balance) << 2,
            spec.max_lines.unwrap_or(0),
            0,
        );
        if let Some(hit) = self.cache.borrow().get(&key) {
            return hit.clone();
        }
        let laid = Rc::new(self.lay_out(spec, width));
        let mut cache = self.cache.borrow_mut();
        if cache.len() > 4000 {
            cache.clear();
        }
        cache.insert(key, laid.clone());
        laid
    }

    /// The width of the text on one line, and the narrowest it can be set (its longest word).
    pub fn intrinsic(&self, spec: &TextSpec) -> (f32, f32) {
        let mut p = self.build(spec);
        p.layout(1.0e6);
        let max = p.max_intrinsic_width();
        let min = p.min_intrinsic_width();
        (max, min)
    }
}
