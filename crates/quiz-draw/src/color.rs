//! A colour as the player's CSS gives it: sRGB bytes and an alpha in 0..=1.

use skia_safe::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const WHITE: Self = Self::new(255, 255, 255, 1.0);
    pub const BLACK: Self = Self::new(0, 0, 0, 1.0);

    /// The same colour with its alpha replaced (`bg-white/15`).
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// As a Skia colour, its alpha times `opacity`, rounded to a byte as a browser does.
    pub fn skia(self, opacity: f32) -> Color {
        Color::from_argb(
            (self.a * opacity * 255.0).round().clamp(0.0, 255.0) as u8,
            self.r,
            self.g,
            self.b,
        )
    }
}
