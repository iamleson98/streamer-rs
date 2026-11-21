//! The painters of the parts of a frame that are not the board or a quiz: the strip under the stage
//! with the brand, the teacher's screen and camera (already scaled by the decoder, so they are
//! blitted), and the countdown card. Port of `export/painters.ts` and `export/card-painter.ts`.
//!
//! Everything draws on a Skia canvas of 1280 x 720 and uses one `FontSet`, which is not `Send`: a
//! painter lives on the thread that composes the frames.

use std::rc::Rc;

use board_draw::FontSet;
use skia_safe::gradient::{Colors, Gradient, Interpolation, shaders};
use skia_safe::{
    AlphaType, Canvas, ClipOp, Color, Color4f, ColorType, Data, Image, ImageInfo, Paint,
    PaintStyle, RRect, Rect, TileMode, images, surfaces,
};

use crate::card::{TitleRange, countdown_seconds, fit_title, format_countdown};
use crate::clock::{HEIGHT, WIDTH};
use crate::layout::{Boxf, StageLayout, stage_layout};
use crate::logo::{Logo, LogoError};

const STRIP_COLOR: u32 = 0xff0f_172a;
const BG_TOP: u32 = 0xff1e_293b;
const BG_BOTTOM: u32 = 0xff0f_172a;
const TEXT: u32 = 0xfff8_fafc;
const MUTED: u32 = 0xff94_a3b8;
const SOFT: u32 = 0xffcb_d5e1;
const MARGIN_X: f64 = 120.0;
const TITLE_RANGE: TitleRange = TitleRange {
    max_px: 56,
    min_px: 30,
    max_lines: 2,
};
const TEACHER_PX: f64 = 28.0;
const LABEL_PX: f64 = 32.0;
const TIME_PX: f64 = 132.0;

#[derive(Debug, thiserror::Error)]
pub enum PaintError {
    #[error(transparent)]
    Logo(#[from] LogoError),
    #[error("cannot make a surface")]
    Surface,
}

/// What the countdown card says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardText {
    pub title: String,
    pub teacher: String,
    /// "Live lesson starts in", in the lesson's language.
    pub starts_in: String,
}

#[derive(Debug)]
struct CardBase {
    image: Image,
    time_y: f64,
}

/// Paints the strip, the card and the blitted pictures of one video.
pub struct StagePainter {
    fonts: Rc<FontSet>,
    layout: StageLayout,
    strip: Image,
    card: Option<CardBase>,
}

impl std::fmt::Debug for StagePainter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagePainter")
            .field("layout", &self.layout)
            .finish()
    }
}

fn color(c: u32) -> Color {
    Color::from(c)
}

/// Draws one line at `x` (its left, centre or right per `align`) with its alphabetic baseline at `y`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
}

/// One line of text: what, how big, how heavy and what colour.
struct Run<'a> {
    s: &'a str,
    px: f64,
    weight: u16,
    rgb: u32,
}

fn text(canvas: &Canvas, fonts: &FontSet, run: &Run, x: f64, baseline: f64, align: Align) {
    let line = fonts.line(run.s, run.px, run.weight, color(run.rgb));
    let left = match align {
        Align::Left => x,
        Align::Center => x - f64::from(line.max_intrinsic_width()) / 2.0,
    };
    line.paint(
        canvas,
        (
            left as f32,
            (baseline - f64::from(line.alphabetic_baseline())) as f32,
        ),
    );
}

fn fill(canvas: &Canvas, b: Boxf, rgb: u32) {
    let mut paint = Paint::default();
    paint.set_color(color(rgb));
    canvas.draw_rect(
        Rect::from_xywh(b.x as f32, b.y as f32, b.w as f32, b.h as f32),
        &paint,
    );
}

/// The strip under the stage with the brand at its left.
fn draw_strip(canvas: &Canvas, fonts: &FontSet, logo: &Logo, layout: &StageLayout) {
    let (strip, brand) = (layout.strip, layout.brand);
    fill(canvas, strip, STRIP_COLOR);
    logo.draw(canvas, brand.x as f32, brand.y as f32, brand.h as f32);
    let text_x = brand.x + brand.h + 12.0;
    let mid_y = brand.y + brand.h / 2.0;
    let name_px = (brand.h * 0.62).round();
    let name = fonts.line("Học Vui", name_px, 600, color(TEXT));
    let name_w = f64::from(name.max_intrinsic_width());
    // 'middle' baseline: the paragraph's box centred on the strip's middle.
    let baseline = |p: &skia_safe::textlayout::Paragraph, centre: f64| {
        centre - f64::from(p.height()) / 2.0 + f64::from(p.alphabetic_baseline())
    };
    text(
        canvas,
        fonts,
        &Run {
            s: "Học Vui",
            px: name_px,
            weight: 600,
            rgb: TEXT,
        },
        text_x,
        baseline(&name, mid_y),
        Align::Left,
    );
    let domain_x = text_x + name_w + 14.0;
    let domain_px = (brand.h * 0.42).round();
    let domain = fonts.line("hocvui.org", domain_px, 500, color(MUTED));
    if domain_x + f64::from(domain.max_intrinsic_width()) <= brand.x + brand.w {
        text(
            canvas,
            fonts,
            &Run {
                s: "hocvui.org",
                px: domain_px,
                weight: 500,
                rgb: MUTED,
            },
            domain_x,
            baseline(&domain, mid_y + 1.0),
            Align::Left,
        );
    }
}

fn draw_card_base(
    canvas: &Canvas,
    fonts: &FontSet,
    logo: &Logo,
    layout: &StageLayout,
    card: &CardText,
) -> f64 {
    let stage = layout.stage;
    let cx = stage.x + stage.w / 2.0;
    let mut paint = Paint::default();
    let colors = [
        Color4f::from(color(BG_TOP)),
        Color4f::from(color(BG_BOTTOM)),
    ];
    paint.set_shader(shaders::linear_gradient(
        ((0.0, stage.y as f32), (0.0, (stage.y + stage.h) as f32)),
        &Gradient::new(
            Colors::new(&colors, None, TileMode::Clamp, None),
            Interpolation::default(),
        ),
        None,
    ));
    canvas.draw_rect(
        Rect::from_xywh(
            stage.x as f32,
            stage.y as f32,
            stage.w as f32,
            stage.h as f32,
        ),
        &paint,
    );
    draw_strip(canvas, fonts, logo, layout);

    let measure = |s: &str, px: u32| fonts.line_width(s, f64::from(px), 700);
    let title = fit_title(
        &card.title,
        f64::from(WIDTH) - 2.0 * MARGIN_X,
        &measure,
        &TITLE_RANGE,
    );
    let line_h = (f64::from(title.px) * 1.22).round();
    let head = title.lines.len() as f64 * line_h
        + if card.teacher.is_empty() {
            0.0
        } else {
            16.0 + TEACHER_PX
        };
    let gap = if head > 0.0 { 44.0 } else { 0.0 };
    let countdown_h = LABEL_PX + 6.0 + TIME_PX;
    let mut y = stage.y + 24.0_f64.max((stage.h - (head + gap + countdown_h)) / 2.0);
    for line in &title.lines {
        y += line_h;
        text(
            canvas,
            fonts,
            &Run {
                s: line,
                px: f64::from(title.px),
                weight: 700,
                rgb: TEXT,
            },
            cx,
            y - (f64::from(title.px) * 0.22).round(),
            Align::Center,
        );
    }
    if !card.teacher.is_empty() {
        y += 16.0 + TEACHER_PX;
        text(
            canvas,
            fonts,
            &Run {
                s: &card.teacher,
                px: TEACHER_PX,
                weight: 500,
                rgb: MUTED,
            },
            cx,
            y - 6.0,
            Align::Center,
        );
    }
    if head > 0.0 {
        let mut bar = Paint::default();
        bar.set_anti_alias(true);
        let stops = [
            Color4f::from(Color::from(0xff38_bdf8)),
            Color4f::from(Color::from(0xff4f_46e5)),
        ];
        bar.set_shader(shaders::linear_gradient(
            (((cx - 40.0) as f32, 0.0), ((cx + 40.0) as f32, 0.0)),
            &Gradient::new(
                Colors::new(&stops, None, TileMode::Clamp, None),
                Interpolation::default(),
            ),
            None,
        ));
        canvas.draw_rrect(
            RRect::new_rect_xy(
                Rect::from_xywh((cx - 40.0) as f32, (y + gap / 2.0 - 2.0) as f32, 80.0, 4.0),
                2.0,
                2.0,
            ),
            &bar,
        );
        y += gap;
    }
    y += LABEL_PX;
    text(
        canvas,
        fonts,
        &Run {
            s: &card.starts_in,
            px: LABEL_PX,
            weight: 500,
            rgb: SOFT,
        },
        cx,
        y - 8.0,
        Align::Center,
    );
    y + 6.0 + TIME_PX - 24.0
}

impl StagePainter {
    /// `has_camera`: whether the teacher's camera is on at any point (the strip is taller then).
    /// `card`: the text of the countdown card, when the video opens with one.
    pub fn new(
        fonts: Rc<FontSet>,
        has_camera: bool,
        card: Option<&CardText>,
    ) -> Result<Self, PaintError> {
        let logo = Logo::artwork()?;
        let layout = stage_layout(has_camera);
        let strip = {
            let mut surface = surfaces::raster_n32_premul((WIDTH as i32, layout.strip.h as i32))
                .ok_or(PaintError::Surface)?;
            let strip_only = StageLayout {
                strip: Boxf {
                    y: 0.0,
                    ..layout.strip
                },
                brand: Boxf {
                    y: layout.brand.y - layout.strip.y,
                    ..layout.brand
                },
                ..layout
            };
            draw_strip(surface.canvas(), &fonts, &logo, &strip_only);
            surface.image_snapshot()
        };
        let card = match card {
            Some(text) => {
                let mut surface = surfaces::raster_n32_premul((WIDTH as i32, HEIGHT as i32))
                    .ok_or(PaintError::Surface)?;
                let time_y = draw_card_base(surface.canvas(), &fonts, &logo, &layout, text);
                Some(CardBase {
                    image: surface.image_snapshot(),
                    time_y,
                })
            }
            None => None,
        };
        Ok(Self {
            fonts,
            layout,
            strip,
            card,
        })
    }

    pub fn layout(&self) -> &StageLayout {
        &self.layout
    }

    /// The strip under the stage: the brand.
    pub fn paint_strip(&self, canvas: &Canvas) {
        canvas.draw_image(&self.strip, (0.0, self.layout.strip.y as f32), None);
    }

    /// The card showing card frame `i` of `card_frames`. Panics are not possible: without a card
    /// nothing is drawn.
    pub fn paint_card(&self, canvas: &Canvas, i: u64, card_frames: u64) {
        let Some(card) = &self.card else { return };
        canvas.draw_image(&card.image, (0.0, 0.0), None);
        let shown = format_countdown(countdown_seconds(i, card_frames));
        text(
            canvas,
            &self.fonts,
            &Run {
                s: &shown,
                px: TIME_PX,
                weight: 700,
                rgb: TEXT,
            },
            f64::from(WIDTH) / 2.0,
            card.time_y,
            Align::Center,
        );
    }

    /// The teacher's screen on black inside the stage; `picture` is the decoder's frame, already
    /// fitted and centred on black at the stage's size (BGRA). With none the stage is black.
    pub fn paint_screen(&self, canvas: &Canvas, picture: Option<&[u8]>) {
        let stage = self.layout.stage;
        fill(canvas, stage, 0xff00_0000);
        if let Some(bgra) = picture {
            blit_bgra(canvas, stage, bgra);
        }
    }

    /// The camera as a rounded tile in the strip, black behind the picture and a white frame.
    pub fn paint_camera(&self, canvas: &Canvas, bgra: &[u8]) {
        let Some(tile) = self.layout.tile else { return };
        let rect = Rect::from_xywh(tile.x as f32, tile.y as f32, tile.w as f32, tile.h as f32);
        let round = RRect::new_rect_xy(rect, 12.0, 12.0);
        canvas.save();
        canvas.clip_rrect(round, ClipOp::Intersect, true);
        fill(canvas, tile, 0xff00_0000);
        blit_bgra(canvas, tile, bgra);
        canvas.restore();
        let mut ring = Paint::default();
        ring.set_anti_alias(true);
        ring.set_style(PaintStyle::Stroke);
        ring.set_stroke_width(3.0);
        ring.set_color(Color::WHITE);
        canvas.draw_rrect(round, &ring);
    }
}

/// Draws raw opaque BGRA pixels of exactly `b`'s size at its position.
pub fn blit_bgra(canvas: &Canvas, b: Boxf, bgra: &[u8]) {
    let (w, h) = (b.w as i32, b.h as i32);
    let info = ImageInfo::new((w, h), ColorType::BGRA8888, AlphaType::Opaque, None);
    if bgra.len() != (w * h * 4) as usize {
        return;
    }
    if let Some(image) = images::raster_from_data(&info, Data::new_copy(bgra), (w * 4) as usize) {
        canvas.draw_image(&image, (b.x as f32, b.y as f32), None);
    }
}
