//! The Skia backend against the Canvas 2D behaviour the board relies on.

mod common;

use board_draw::{Canvas, LineCap, Path, Rgba};
use common::{canvas, pixel};

#[test]
fn fill_rect_and_clear_rect() {
    let mut c = canvas(20, 20);
    c.set_fill_style(Rgba::rgb(255, 0, 0));
    c.fill_rect(5.0, 5.0, 10.0, 10.0);
    assert_eq!(pixel(&c.rgba(), 20, 10, 10), [255, 0, 0, 255]);
    c.clear_rect(0.0, 0.0, 20.0, 20.0);
    assert_eq!(pixel(&c.rgba(), 20, 10, 10)[3], 0);
}

#[test]
fn global_alpha_scales_the_colour_and_restore_brings_back_the_state() {
    let mut c = canvas(10, 10);
    c.set_fill_style(Rgba::rgb(0, 0, 255));
    c.save();
    c.set_global_alpha(0.5);
    c.fill_rect(0.0, 0.0, 5.0, 10.0);
    c.restore();
    c.fill_rect(5.0, 0.0, 5.0, 10.0);
    let px = c.rgba();
    assert!((i32::from(pixel(&px, 10, 2, 5)[3]) - 128).abs() <= 1);
    assert_eq!(pixel(&px, 10, 7, 5), [0, 0, 255, 255]);
}

#[test]
fn set_transform_maps_board_units_to_pixels() {
    let mut c = canvas(40, 40);
    c.set_transform(2.0, 0.0, 0.0, 2.0, 10.0, 0.0);
    c.set_fill_style(Rgba::rgb(0, 0, 0));
    c.fill_rect(0.0, 0.0, 5.0, 5.0);
    let px = c.rgba();
    assert_eq!(pixel(&px, 40, 12, 2)[3], 255);
    assert_eq!(pixel(&px, 40, 8, 2)[3], 0);
    assert_eq!(pixel(&px, 40, 22, 2)[3], 0);
}

#[test]
fn clip_limits_later_drawing() {
    let mut c = canvas(20, 20);
    let mut clip = Path::new();
    clip.rect(0.0, 0.0, 10.0, 20.0);
    c.save();
    c.clip(&clip);
    c.set_fill_style(Rgba::rgb(0, 0, 0));
    c.fill_rect(0.0, 0.0, 20.0, 20.0);
    c.restore();
    let px = c.rgba();
    assert_eq!(pixel(&px, 20, 5, 5)[3], 255);
    assert_eq!(pixel(&px, 20, 15, 5)[3], 0);
}

#[test]
fn strokes_have_caps_and_dashes() {
    let line = |c: &mut board_draw::SkiaCanvas| {
        let mut p = Path::new();
        p.move_to(10.0, 10.0);
        p.line_to(40.0, 10.0);
        c.set_stroke_style(Rgba::rgb(0, 0, 0));
        c.set_line_width(6.0);
        c.stroke(&p);
    };
    let mut butt = canvas(50, 20);
    butt.set_line_cap(LineCap::Butt);
    line(&mut butt);
    let mut round = canvas(50, 20);
    round.set_line_cap(LineCap::Round);
    line(&mut round);
    // A round cap reaches past the end point, a butt cap does not.
    assert_eq!(pixel(&butt.rgba(), 50, 8, 10)[3], 0);
    assert_eq!(pixel(&round.rgba(), 50, 8, 10)[3], 255);
    let mut dashed = canvas(50, 20);
    dashed.set_line_dash(&[5.0, 5.0]);
    line(&mut dashed);
    let px = dashed.rgba();
    assert_eq!(pixel(&px, 50, 12, 10)[3], 255);
    assert_eq!(pixel(&px, 50, 17, 10)[3], 0);
}

#[test]
fn a_full_turn_ellipse_is_a_whole_ring_and_a_partial_arc_is_not() {
    let ring = |end: f64| {
        let mut c = canvas(60, 60);
        let mut p = Path::new();
        p.ellipse(30.0, 30.0, 20.0, 12.0, 0.0, end, false);
        c.set_stroke_style(Rgba::rgb(0, 0, 0));
        c.set_line_width(3.0);
        c.stroke(&p);
        c.rgba()
    };
    let full = ring(std::f64::consts::TAU);
    for (x, y) in [(50, 30), (10, 30), (30, 42), (30, 18)] {
        assert_eq!(pixel(&full, 60, x, y)[3], 255, "({x},{y})");
    }
    assert_eq!(pixel(&full, 60, 30, 30)[3], 0);
    let quarter = ring(std::f64::consts::FRAC_PI_2);
    assert_eq!(pixel(&quarter, 60, 30, 42)[3], 255);
    assert_eq!(pixel(&quarter, 60, 10, 30)[3], 0);
}

#[test]
fn round_rect_rounds_its_corners() {
    let mut c = canvas(40, 40);
    let mut p = Path::new();
    p.round_rect(5.0, 5.0, 30.0, 30.0, 10.0);
    c.set_fill_style(Rgba::rgb(0, 0, 0));
    c.fill(&p);
    let px = c.rgba();
    assert_eq!(pixel(&px, 40, 20, 20)[3], 255);
    assert_eq!(pixel(&px, 40, 6, 6)[3], 0);
}

#[test]
fn text_is_drawn_from_its_top_and_measured_like_it_is_drawn() {
    let mut c = canvas(300, 60);
    let m = c.measurer();
    let w = m.width("Hello world", 20.0);
    assert!(w > 80.0 && w < 140.0, "{w}");
    assert_eq!(m.width("", 20.0), 0.0);
    assert!(m.width("Hello world", 40.0) > 1.9 * w);
    c.set_fill_style(Rgba::rgb(0, 0, 0));
    c.set_font_size(20.0);
    c.fill_text("Hello world", 10.0, 10.0);
    let px = c.rgba();
    let inked = |x0: u32, x1: u32, y0: u32, y1: u32| {
        (y0..y1).any(|y| (x0..x1).any(|x| pixel(&px, 300, x, y)[3] > 0))
    };
    // Ink is inside the measured box, below the line's top, and nowhere to the right of it.
    assert!(inked(10, 10 + w.ceil() as u32 + 1, 10, 36));
    assert!(!inked(10 + w.ceil() as u32 + 3, 300, 0, 60));
    assert!(!inked(0, 300, 0, 9));
}

#[test]
fn vietnamese_letters_are_found_in_the_second_face() {
    let c = canvas(10, 10);
    let m = c.measurer();
    assert!(m.width("ạ", 20.0) > 5.0);
    assert!(m.width("ở", 20.0) > 5.0);
}

/// The widths the browser measured for these (parity corpus, Chromium 159, size 28), which the
/// text must have to the thousandth: a letter from another face never kerns with its neighbour,
/// and one of the second face is taken whole, not built out of the first face's base and mark.
#[test]
fn text_is_cut_at_the_faces_like_the_browser_does_it() {
    let c = canvas(8, 8);
    let m = c.measurer();
    let w = |s: &str| m.width(s, 28.0);
    // Same face: kerned.
    assert!((w("Ta") - 31.746).abs() < 0.001, "{}", w("Ta"));
    // 'ạ' and 'ẽ' come from the Vietnamese face: no kerning with the Latin letter before them.
    assert!((w("Tạ") - 33.797).abs() < 0.001, "{}", w("Tạ"));
    assert!((w("vẽ") - 32.061).abs() < 0.001, "{}", w("vẽ"));
    assert!((w("Lỹ") - 31.568).abs() < 0.001, "{}", w("Lỹ"));
    // A letter and its precomposed neighbour in one face are still kerned.
    assert!((w("ve") - 31.514).abs() < 0.001, "{}", w("ve"));
}

/// A combining mark stays with its base's face, so it attaches to it and adds no width.
#[test]
fn a_combining_mark_attaches_to_its_base_in_the_same_face() {
    let c = canvas(8, 8);
    let m = c.measurer();
    let (base, marked) = (m.width("a", 28.0), m.width("a\u{0300}", 28.0));
    assert!((marked - 15.723).abs() < 0.001, "{marked}");
    assert!((marked - base).abs() < 1.0, "{base} {marked}");
}

#[test]
fn a_character_no_face_has_is_drawn_the_same_every_time() {
    // U+263A is in none of the board's fonts: it is the first face's missing-glyph box.
    let draw = || {
        let mut c = canvas(60, 40);
        c.set_fill_style(Rgba::hex(0));
        c.set_font_size(28.0);
        c.fill_text("\u{263a}", 5.0, 5.0);
        c.rgba()
    };
    assert_eq!(draw(), draw());
    assert!(draw().iter().any(|&b| b != 0));
}

/// Canvas text turns tabs, form feeds and carriage returns into spaces; measured so by Chromium on
/// Linux and macOS alike (the text engine's own answer differs between platforms).
#[test]
fn tabs_form_feeds_and_carriage_returns_are_spaces() {
    let c = canvas(8, 8);
    let m = c.measurer();
    let space = m.width("a b", 28.0);
    assert!((space - 40.742).abs() < 0.001, "{space}");
    for ws in ["a\tb", "a\u{c}b", "a\rb"] {
        assert_eq!(m.width(ws, 28.0), space, "{ws:?}");
    }
    assert!((m.width("\ttab\tseparated", 28.0) - 190.777).abs() < 0.001);
}
