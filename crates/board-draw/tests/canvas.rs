//! The Skia backend against the Canvas 2D behaviour the board relies on.

mod common;

use board_draw::canvas::{TextAlign, TextBaseline};
use board_draw::{Canvas, LineCap, Path, Rgba};
use common::{canvas, canvas_with_fallbacks, fonts, fonts_with_fallbacks, pixel};

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

// The fallback fonts: the board's six faces are 0..6, then Noto Sans (6), Symbols (7), CJK (8),
// Thai (9), Lao (10), Khmer (11), Arabic (12), Hebrew (13), Devanagari (14) and the emoji (15).
const NOTO_SANS: usize = 6;
const CJK: usize = 8;
const THAI: usize = 9;
const ARABIC: usize = 12;
const HEBREW: usize = 13;
const EMOJI: usize = 15;

#[test]
fn runs_are_split_by_the_face_that_has_the_characters() {
    let f = fonts_with_fallbacks();
    assert_eq!(
        f.stretches("Hi 日本 สวัสดี мир"),
        vec![
            ("Hi ", Some(0)),
            ("日本", Some(CJK)),
            (" ", Some(0)),
            ("สวัสดี", Some(THAI)),
            (" ", Some(0)),
            ("мир", Some(NOTO_SANS)),
        ]
    );
}

#[test]
fn what_the_board_fonts_have_never_comes_from_a_fallback_face() {
    let with = fonts_with_fallbacks();
    let without = fonts();
    let text = "Việt Nam αβγ ∑ ★ ᴬ ạ\u{0301} x²";
    assert_eq!(with.stretches(text), without.stretches(text));
    assert!(
        with.stretches(text)
            .iter()
            .all(|(_, face)| face.is_some_and(|f| f < 6))
    );
    // The same pixels as without the fallbacks.
    let draw = |mut c: board_draw::SkiaCanvas| {
        c.set_fill_style(Rgba::rgb(0, 0, 0));
        c.set_font_size(28.0);
        c.fill_text(text, 4.0, 4.0);
        c.rgba()
    };
    assert_eq!(draw(canvas(500, 50)), draw(canvas_with_fallbacks(500, 50)));
}

#[test]
fn a_combining_mark_stays_with_its_base() {
    let f = fonts_with_fallbacks();
    // The base is Thai and so are its marks; a Latin letter with a mark the board fonts lack
    // (a ring above, U+030A) takes the mark from the face that has it, not from a stretch of its own.
    assert_eq!(
        f.stretches("ก\u{0e34}\u{0e49}"),
        vec![("ก\u{0e34}\u{0e49}", Some(THAI))]
    );
    let ring = f.stretches("a\u{030a}");
    assert_eq!(
        ring.iter().map(|(t, _)| *t).collect::<String>(),
        "a\u{030a}"
    );
    assert!(ring.iter().all(|(_, face)| face.is_some()));
}

#[test]
fn an_emoji_sequence_is_never_cut() {
    let f = fonts_with_fallbacks();
    for emoji in [
        "👨\u{200d}👩\u{200d}👧\u{200d}👦", // a family: ZWJ
        "👍🏽",                               // a skin tone
        "🇻🇳",                               // a flag: two regional indicators
        "1\u{fe0f}\u{20e3}",                // a keycap
        "🏴\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}", // a flag from tags
    ] {
        let text = format!("a{emoji}b");
        let runs = f.stretches(&text);
        assert_eq!(runs.iter().map(|(t, _)| *t).collect::<String>(), text);
        // Without the emoji face (macOS cannot draw it) a sequence is text no face has.
        if f.has_emoji_face() {
            assert!(
                runs.contains(&(emoji, Some(EMOJI))),
                "{emoji:?} is cut in {runs:?}"
            );
        }
    }
    // VS16 picks the emoji picture of a character the board's symbol font has; VS15 and nothing
    // leave it to the board's font.
    let heart = f.stretches("\u{2764}\u{fe0f} \u{2764}\u{fe0e} \u{2764}");
    if f.has_emoji_face() {
        assert_eq!(heart[0], ("\u{2764}\u{fe0f}", Some(EMOJI)));
    }
    assert!(
        heart[1..]
            .iter()
            .all(|(_, face)| face.is_some_and(|f| f < 6))
    );
}

#[test]
fn right_to_left_runs_are_ordered_inside_a_line() {
    let f = fonts_with_fallbacks();
    assert_eq!(
        f.stretches("a אב c"),
        vec![("a ", Some(0)), ("אב", Some(HEBREW)), (" c", Some(0))]
    );
    assert_eq!(f.stretches("مرحبا")[0].1, Some(ARABIC));
    // "a אב c" is drawn a, space, bet, alef, space, c: the Hebrew pair reversed, the rest not.
    let width = |s: &str| canvas_with_fallbacks(8, 8).measurer().width(s, 28.0);
    let draw = |parts: &[(&str, f64)]| {
        let mut c = canvas_with_fallbacks(300, 50);
        c.set_fill_style(Rgba::rgb(0, 0, 0));
        c.set_font_size(28.0);
        for (text, x) in parts {
            c.fill_text(text, *x, 4.0);
        }
        c.rgba()
    };
    let (a, bet) = (width("a "), width("ב"));
    let mut c = canvas_with_fallbacks(300, 50);
    c.set_fill_style(Rgba::rgb(0, 0, 0));
    c.set_font_size(28.0);
    c.fill_text("a אב c", 4.0, 4.0);
    let got = c.rgba();
    // How much of the line's ink lies where the letter, drawn alone at x, has its own.
    let overlap = |letter: &str, x: f64| -> u64 {
        got.chunks(4)
            .zip(draw(&[(letter, x)]).chunks(4))
            .map(|(p, q)| u64::from(p[3]) * u64::from(q[3]))
            .sum()
    };
    // The first letter of the pair is on the right, the second on the left: bet, then alef.
    assert!(overlap("ב", 4.0 + a) > 2 * overlap("א", 4.0 + a));
    assert!(overlap("א", 4.0 + a + bet) > 2 * overlap("ב", 4.0 + a + bet));
    // And the Arabic letters join: the word is not the isolated letters side by side.
    let joined = width("بيت");
    let isolated: f64 = ["ب", "ي", "ت"].iter().map(|l| width(l)).sum();
    assert!((joined - isolated).abs() > 1.0);
}

#[test]
fn glyphs_that_resolve_to_notdef_are_counted() {
    let with = fonts_with_fallbacks();
    let without = fonts();
    // Every script the fallbacks cover has a glyph with them and none without.
    for text in [
        "日本語",
        "한국어",
        "สวัสดี",
        "ສະບາຍດີ",
        "សួស្តី",
        "مرحبا",
        "שלום",
        "नमस्ते",
        "мир",
    ] {
        assert_eq!(with.missing_glyphs(text, 28.0), 0, "{text}");
    }
    assert!(without.missing_glyphs("日本語", 28.0) >= 3);
    // Text the board fonts have, joiners and variation selectors (hidden, not a box) and spaces
    // are not missing.
    assert_eq!(
        with.missing_glyphs("Việt Nam α ★ a\u{200d}b \u{fe0e}", 28.0),
        0
    );
    assert_eq!(without.missing_glyphs("Việt Nam α ★", 28.0), 0);
    // A private-use character is in no face.
    assert_eq!(with.missing_glyphs("a\u{e000}b\u{e001}", 28.0), 2);
    assert_eq!(with.missing_glyphs("", 28.0), 0);
    if with.has_emoji_face() {
        assert_eq!(
            with.missing_glyphs("😀 👨\u{200d}👩\u{200d}👧 🇻🇳 1\u{fe0f}\u{20e3}", 28.0),
            0
        );
    } else {
        assert!(with.missing_glyphs("😀", 28.0) > 0);
    }
}

#[test]
fn the_fallback_list_matches_the_checksums_and_the_parity_harness() {
    let dir = env!("CARGO_MANIFEST_DIR");
    let sums = std::fs::read_to_string(format!("{dir}/fallback-fonts.sha256")).unwrap();
    let parity: serde_json::Value = serde_json::from_slice(
        &std::fs::read(format!("{dir}/parity/fallback-fonts.json")).unwrap(),
    )
    .unwrap();
    let parity = parity.as_array().unwrap();
    assert_eq!(parity.len(), board_draw::FALLBACK_FONT_FILES.len());
    for (file, json) in board_draw::FALLBACK_FONT_FILES.iter().zip(parity) {
        assert_eq!(json["file"], file.name);
        assert_eq!(json["index"].as_u64().unwrap_or(0), u64::from(file.index));
        assert!(
            sums.lines()
                .any(|l| l.ends_with(&format!("  {}", file.name))),
            "{}",
            file.name
        );
    }
}

fn em28(text: &str) -> f64 {
    canvas_with_fallbacks(8, 8).measurer().width(text, 28.0)
}

#[track_caller]
fn assert_width(text: &str, expected: f64) {
    let got = em28(text);
    assert!(
        (got - expected).abs() < 0.01,
        "{text:?} is {got} wide at 28, Chromium measures {expected}"
    );
}

// The widths below are what Chromium 159 (headless shell) measures for the same text in a canvas
// with the same fonts, at 28 px; a full-width character is 28.
#[test]
fn adjacent_full_width_punctuation_is_trimmed_like_text_spacing_trim() {
    // A closing mark before another mark, or an opening mark after one, loses half its width.
    assert_width("。」", 42.0);
    assert_width("」、", 42.0);
    assert_width("（「", 42.0);
    assert_width("、「", 42.0);
    assert_width("。。", 42.0);
    assert_width("」・", 42.0);
    assert_width("・「", 42.0);
    assert_width("：」", 42.0);
    // A mark next to text is not trimmed, and neither is the start of the line.
    assert_width("「あ", 56.0);
    assert_width("あ」", 56.0);
    assert_width("（あ）", 84.0);
    assert_width("」あ「", 84.0);
    assert_width("、", 28.0);
    // A whole line: three junctions (」、 、『 』（), 14 px each.
    assert_width("「こんにちは」、『日本語』（括弧）", 434.0);
    // The same in Chinese quotes and book marks.
    assert_width("《汉字》、〈词〉", 28.0 * 8.0 - 28.0);
}

#[test]
fn marks_of_the_latin_font_are_never_trimmed_next_to_full_width_ones() {
    // Quotes and brackets of the board's own font are drawn from it, and Chromium does not look at
    // a neighbour of another font.
    let quote = em28("”");
    assert_width("」”", 28.0 + quote);
    assert_width("“「", 28.0 + quote);
    let paren = em28(")");
    assert_width("。)", 28.0 + paren);
}

#[test]
fn kana_are_kerned_only_where_chromium_kerns_them() {
    // Kana pairs are not kerned, a kana before a comma or full stop is (by the font's pair),
    // and so are the prolonged sound mark and the middle dot with the kana before them.
    assert_width("しています", 140.0);
    assert_width("ラーメン", 112.0);
    assert_width("スペース", 112.0);
    assert_width("ます。", 28.0 * 3.0 - 2.8);
    assert_width("う。", 28.0 * 2.0 - 1.68);
    assert_width("す、", 28.0 * 2.0 - 2.24);
    assert_width("日本語を勉強しています。漢字と仮名", 473.2);
    // A halved mark still kerns with the kana before it.
    assert_width("ア。。", 28.0 * 3.0 - 14.0 - 2.8);
}

/// The columns and rows that have ink: `(min x, max x, min y, max y)`.
fn ink_box(px: &[u8], width: u32, height: u32) -> (u32, u32, u32, u32) {
    let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0, u32::MAX, 0);
    for y in 0..height {
        for x in 0..width {
            if pixel(px, width, x, y)[3] > 64 {
                (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
            }
        }
    }
    (x0, x1, y0, y1)
}

fn text_box(
    align: TextAlign,
    baseline: TextBaseline,
    italic: bool,
    weight: u16,
) -> (u32, u32, u32, u32) {
    let mut c = canvas(200, 100);
    c.set_fill_style(Rgba::rgb(0, 0, 0));
    c.set_font(italic, weight, 24.0);
    c.set_text_align(align);
    c.set_text_baseline(baseline);
    c.fill_text("HH", 100.0, 50.0);
    ink_box(&c.rgba(), 200, 100)
}

#[test]
fn text_sits_on_the_side_and_line_the_alignment_and_baseline_say() {
    let (l0, l1, ..) = text_box(TextAlign::Left, TextBaseline::Top, false, 400);
    let (c0, c1, ..) = text_box(TextAlign::Center, TextBaseline::Top, false, 400);
    let (r0, r1, ..) = text_box(TextAlign::Right, TextBaseline::Top, false, 400);
    assert!((98..=104).contains(&l0), "{l0}");
    assert!(((c0 + c1) as i32 - 200).abs() <= 3, "{c0} {c1}");
    assert!((96..=101).contains(&r1), "{r1}");
    assert_eq!((l1 - l0, c1 - c0, r1 - r0), (l1 - l0, l1 - l0, l1 - l0));
    let (.., top0, top1) = text_box(TextAlign::Left, TextBaseline::Top, false, 400);
    let (.., mid0, mid1) = text_box(TextAlign::Left, TextBaseline::Middle, false, 400);
    let (.., bot0, bot1) = text_box(TextAlign::Left, TextBaseline::Bottom, false, 400);
    assert!(
        (50..=56).contains(&top0),
        "the top of the line is at y: {top0}"
    );
    assert!(mid0 < top0 && bot0 < mid0, "{top0} {mid0} {bot0}");
    assert!(
        (mid1 as i32 - top1 as i32 + 12).abs() <= 2 && (bot1 as i32) <= 50,
        "{mid1} {bot1}"
    );
}

#[test]
fn bold_is_heavier_and_italic_leans_right_about_the_baseline() {
    let weight = |w| {
        let mut c = canvas(200, 100);
        c.set_fill_style(Rgba::rgb(0, 0, 0));
        c.set_font(false, w, 24.0);
        c.fill_text("lll", 20.0, 20.0);
        c.rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u64::from(p[3]))
            .sum::<u64>()
    };
    assert!(weight(700) > weight(400) + weight(400) / 5);
    let (u0, u1, y0, y1) = text_box(TextAlign::Left, TextBaseline::Top, false, 400);
    let (i0, i1, j0, j1) = text_box(TextAlign::Left, TextBaseline::Top, true, 400);
    assert_eq!((y0, y1), (j0, j1), "a slant keeps the line where it was");
    assert!(
        i1 > u1 && i1 - i0 > u1 - u0,
        "the top of each letter leans out: {i0}-{i1} against {u0}-{u1}"
    );
}

#[test]
fn stroked_text_is_an_outline_wider_than_the_fill_and_scale_follows_the_transform() {
    let mut c = canvas(200, 60);
    c.set_font(false, 400, 24.0);
    c.set_text_baseline(TextBaseline::Top);
    c.set_stroke_style(Rgba::rgb(255, 0, 0));
    c.set_line_width(4.0);
    c.stroke_text("H", 20.0, 10.0);
    let stroked = ink_box(&c.rgba(), 200, 60);
    let mut f = canvas(200, 60);
    f.set_font(false, 400, 24.0);
    f.set_text_baseline(TextBaseline::Top);
    f.fill_text("H", 20.0, 10.0);
    let filled = ink_box(&f.rgba(), 200, 60);
    assert!(
        stroked.0 < filled.0 && stroked.2 < filled.2 && stroked.1 > filled.1,
        "{stroked:?} {filled:?}"
    );
    assert_eq!(pixel(&c.rgba(), 200, stroked.0, stroked.2)[0], 255);
    assert_eq!(c.scale(), 1.0);
    c.set_transform(2.0, 0.0, 0.0, 2.0, 5.0, 5.0);
    assert_eq!(c.scale(), 2.0);
    c.set_transform(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    assert_eq!(c.scale(), 1.0);
}
