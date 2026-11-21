//! Pieces of the player's quiz components that several screens use (the avatar discs, the rings, the
//! text steps), each a function that builds the nodes of the Svelte component's DOM with the same
//! Tailwind classes' values: sizes, spacing, radii and weights as the CSS gives them.

use std::collections::HashMap;
use std::rc::Rc;

use taffy::prelude::{AlignItems, JustifyContent, NodeId};

use crate::art::Art;
use crate::art_data;
use crate::color::Rgba;
use crate::i18n::{Locale, t};
use crate::text::TextSpec;
use crate::tokens;
use crate::ui::{Builder, Content, S, Visual};

/// The pictures, parsed once.
#[derive(Debug)]
pub struct Library {
    pub characters: Vec<Rc<Art>>,
    pub coin: Rc<Art>,
    icons: HashMap<&'static str, Rc<Art>>,
}

impl Library {
    pub fn new() -> Self {
        Self {
            characters: art_data::CHARACTERS
                .iter()
                .map(|s| Rc::new(Art::parse(s)))
                .collect(),
            coin: Rc::new(Art::parse(art_data::COIN)),
            icons: art_data::ICONS
                .iter()
                .map(|(n, s)| (*n, Rc::new(Art::parse(s))))
                .collect(),
        }
    }

    pub fn icon(&self, name: &str) -> Rc<Art> {
        self.icons
            .get(name)
            .unwrap_or_else(|| panic!("icon {name}"))
            .clone()
    }
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}

/// What a screen is built with.
#[derive(Debug)]
pub struct Cx<'a> {
    pub b: Builder,
    pub lib: &'a Library,
    pub locale: Locale,
}

impl Cx<'_> {
    pub fn t(&self, key: &str, params: &[(&str, String)]) -> String {
        t(self.locale, key, params)
    }
}

// Tailwind's type steps: size and line height in px.
pub const XS: (f32, f32) = (12.0, 16.0);
pub const SM: (f32, f32) = (14.0, 20.0);
pub const BASE: (f32, f32) = (16.0, 24.0);
pub const LG: (f32, f32) = (18.0, 28.0);
pub const XL: (f32, f32) = (20.0, 28.0);
pub const XL2: (f32, f32) = (24.0, 32.0);
pub const XL3: (f32, f32) = (30.0, 36.0);
pub const XL4: (f32, f32) = (36.0, 40.0);

// Weights.
pub const MEDIUM: u16 = 500;
pub const SEMIBOLD: u16 = 600;
pub const BOLD: u16 = 700;
pub const EXTRABOLD: u16 = 800;
pub const BLACK: u16 = 900;

// Radii of sui's theme (`--radius` is 0.625rem).
pub const R_LG: f32 = 10.0;
pub const R_XL: f32 = 14.0;
pub const R_2XL: f32 = 18.0;
pub const R_FULL: f32 = 9999.0;

pub fn spec(text: impl Into<String>, step: (f32, f32), weight: u16, color: Rgba) -> TextSpec {
    TextSpec::plain(text, step.0, step.1, weight, color)
}

/// A text step with an explicit line-height multiplier (`leading-tight`, `leading-none`, `leading-snug`).
pub fn leading(step: (f32, f32), mult: f32) -> (f32, f32) {
    (step.0, step.0 * mult)
}

pub fn disc_colours(i: usize) -> (Rgba, Rgba) {
    match i {
        0 => (tokens::DISC_0_BG, tokens::DISC_0_RING),
        1 => (tokens::DISC_1_BG, tokens::DISC_1_RING),
        2 => (tokens::DISC_2_BG, tokens::DISC_2_RING),
        3 => (tokens::DISC_3_BG, tokens::DISC_3_RING),
        4 => (tokens::DISC_4_BG, tokens::DISC_4_RING),
        5 => (tokens::DISC_5_BG, tokens::DISC_5_RING),
        6 => (tokens::DISC_6_BG, tokens::DISC_6_RING),
        _ => unreachable!("disc tone {i}"),
    }
}

pub fn character_ring(i: i64) -> Rgba {
    [
        tokens::CHARACTER_0_RING,
        tokens::CHARACTER_1_RING,
        tokens::CHARACTER_2_RING,
        tokens::CHARACTER_3_RING,
        tokens::CHARACTER_4_RING,
        tokens::CHARACTER_5_RING,
        tokens::CHARACTER_6_RING,
        tokens::CHARACTER_7_RING,
        tokens::CHARACTER_8_RING,
        tokens::CHARACTER_9_RING,
        tokens::CHARACTER_10_RING,
        tokens::CHARACTER_11_RING,
    ][i.rem_euclid(12) as usize]
}

pub fn tile_colours(i: usize) -> (Rgba, Rgba) {
    match i % 6 {
        0 => (tokens::TILE_0_FROM, tokens::TILE_0_TO),
        1 => (tokens::TILE_1_FROM, tokens::TILE_1_TO),
        2 => (tokens::TILE_2_FROM, tokens::TILE_2_TO),
        3 => (tokens::TILE_3_FROM, tokens::TILE_3_TO),
        4 => (tokens::TILE_4_FROM, tokens::TILE_4_TO),
        _ => (tokens::TILE_5_FROM, tokens::TILE_5_TO),
    }
}

/// An animal on a coloured disc (`quiz/components/avatar-disc.svelte`): `size` is the disc's px, `font` the
/// emoji's text step.
pub fn avatar_disc(c: &mut Cx, name: &str, size: f32, font: (f32, f32)) -> NodeId {
    let a = replay_model::quiz::avatar::avatar_of(name);
    let (bg, ring) = disc_colours(a.disc);
    let emoji = c.b.text(
        "span",
        S::block(),
        Visual::default(),
        spec(a.emoji, font, 400, tokens::FOREGROUND),
    );
    c.b.div(
        "span",
        S::row()
            .size(size)
            .shrink0()
            .items(AlignItems::CENTER)
            .justify(JustifyContent::CENTER),
        Visual::default().bg(bg).ring(2.0, ring).radius(9999.0),
        &[emoji],
    )
}

/// A Quest character on a round plate (`quest/components/character-disc.svelte`).
pub fn character_disc(c: &mut Cx, id: i64, size: f32) -> NodeId {
    let art = c.lib.characters[id.rem_euclid(12) as usize].clone();
    let picture = c.b.art(
        "svg",
        S::block().size(size * 0.84),
        Visual::default(),
        art,
        tokens::FOREGROUND,
    );
    c.b.div(
        "span",
        S::row()
            .size(size)
            .shrink0()
            .items(AlignItems::CENTER)
            .justify(JustifyContent::CENTER),
        Visual::default()
            .bg(tokens::SLATE_100)
            .ring(2.0, character_ring(id))
            .radius(9999.0),
        &[picture],
    )
}

/// The countdown ring (`quiz/components/ring.svelte`) of `size` px with `inner` in its middle.
pub fn ring(c: &mut Cx, size: f32, fraction: f32, tone: Rgba, inner: NodeId) -> NodeId {
    let svg = c.b.node(
        S::block().size(size),
        Visual {
            tag: "svg",
            ..Default::default()
        },
        Content::Ring {
            fraction,
            color: tone,
        },
        &[],
    );
    let middle = c.b.div(
        "span",
        S::row()
            .abs(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .items(AlignItems::CENTER)
            .justify(JustifyContent::CENTER),
        Visual::default(),
        &[inner],
    );
    c.b.div(
        "div",
        S::block().size(size).shrink0().relative(),
        Visual::default(),
        &[svg, middle],
    )
}

/// A text leaf of a ring's middle (`text-base font-extrabold tabular-nums sm:text-xl`, or a smaller inner span).
pub fn ring_text(c: &mut Cx, text: &str, step: (f32, f32)) -> NodeId {
    c.b.text(
        "span",
        S::block(),
        Visual::default(),
        spec(text, step, EXTRABOLD, tokens::FOREGROUND).tab(),
    )
}
