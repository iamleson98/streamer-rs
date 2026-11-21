//! The renderer, the painter and the layout dump, on small replays.

mod common;

use board_draw::{
    BoardPainter, BoardRenderer, BoardTheme, Canvas, DrawOptions, StaticPictures, decode_image,
    layout_dump,
};
use common::{canvas, pixel};
use replay_model::{BoardReplay, Viewport, parse_board_log};

const W: u32 = 200;
const H: u32 = 120;

fn replay(lines: &[&str]) -> BoardReplay {
    BoardReplay::new(parse_board_log(&lines.join("\n")))
}

fn el(id: &str, z: u32, data: &str) -> String {
    format!(
        r#"{{"id":"{id}","version":1,"version_nonce":1,"is_deleted":false,"data":{{"z":{z},{data}}}}}"#
    )
}

fn snapshot(t: u32, elements: &[String]) -> String {
    format!(
        r#"{{"t":{t},"k":"snapshot","elements":[{}]}}"#,
        elements.join(",")
    )
}

fn paint(replay: &BoardReplay, pictures: &StaticPictures<skia_safe::Image>) -> Vec<u8> {
    paint_sized(replay, pictures, W, H)
}

fn paint_sized(
    replay: &BoardReplay,
    pictures: &StaticPictures<skia_safe::Image>,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let mut target = canvas(w, h);
    let mut painter = BoardPainter::new(&target, w, h);
    painter.paint(&mut target, (0.0, 0.0), replay, pictures);
    target.rgba()
}

const LINE: &str = r#""type":"line","color":"red","size":8,"x1":20,"y1":60,"x2":180,"y2":60"#;

#[test]
fn an_empty_board_is_white_paper_with_a_dot_grid() {
    let mut r = replay(&[]);
    r.seek(0.0);
    let px = paint(&r, &StaticPictures::new());
    assert_eq!(pixel(&px, W, 3, 3), [255, 255, 255, 255]);
    // The grid's dots are at multiples of 32, a little darker than the paper.
    let dot = pixel(&px, W, 32, 32);
    assert!(dot[0] < 255 && dot[0] > 200, "{dot:?}");
}

#[test]
fn a_line_is_drawn_in_its_palette_colour_in_the_frame() {
    let mut r = replay(&[&snapshot(10, &[el("a", 1, LINE)])]);
    r.seek(10.0);
    let px = paint(&r, &StaticPictures::new());
    // The camera fits the line's box (zoom 1 here), centred: the line's middle is the frame's.
    assert_eq!(pixel(&px, W, 100, 60), [0xe0, 0x31, 0x31, 255]);
    assert_eq!(pixel(&px, W, 100, 30), [255, 255, 255, 255]);
}

#[test]
fn a_pointer_preview_is_drawn_over_the_board_and_goes_with_the_users_write() {
    let preview = r#"{"t":20,"k":"presence","u":"u1","p":{"preview":{"type":"rect","color":"blue","size":6,"z":9,"x1":60,"y1":20,"x2":140,"y2":100}}}"#;
    let write = format!(
        r#"{{"t":30,"k":"elements","u":"u1","elements":[{}]}}"#,
        el(
            "r",
            9,
            r#""type":"rect","color":"blue","size":6,"x1":60,"y1":20,"x2":140,"y2":100"#
        )
    );
    let mut r = replay(&[&snapshot(10, &[el("a", 1, LINE)]), preview, &write]);
    let none = StaticPictures::new();
    r.seek(10.0);
    let before = paint(&r, &none);
    r.seek(20.0);
    let during = paint(&r, &none);
    assert_ne!(before, during);
    r.seek(30.0);
    let after = paint(&r, &none);
    // The same rectangle, now committed: the frame looks as it did with the preview. (The camera
    // moved with the content, so compare on the pixel where both agree: the line's colour.)
    assert_eq!(pixel(&after, W, 100, 60)[3], 255);
}

#[test]
fn highlighters_are_translucent_and_the_dark_theme_has_its_own_inks() {
    let hl = r#""type":"stroke","color":"orange","size":8,"highlighter":true,"points":[20,60,0.5,180,60,0.5]"#;
    let elements = [snapshot(1, &[el("h", 1, hl)])];
    let mut r = replay(&[&elements[0]]);
    r.seek(1.0);
    let px = paint(&r, &StaticPictures::new());
    let mid = pixel(&px, W, 100, 60);
    // 35 % of orange over white.
    assert_eq!(mid[..3], [247, 197, 170], "{mid:?}");
    // Dark theme: drawn through the renderer directly.
    let scene = r.scene();
    let list = scene.ordered();
    let mut ctx = canvas(W, H);
    let measure = ctx.measurer();
    let mut renderer = BoardRenderer::new();
    renderer.draw(
        &mut ctx,
        &DrawOptions {
            elements: &list,
            revision: scene.revision(),
            overlays: &[],
            hidden: &[],
            viewport: Viewport::IDENTITY,
            width: f64::from(W),
            height: f64::from(H),
            dpr: 1.0,
            theme: BoardTheme::Dark,
            measure: &*measure,
        },
        &StaticPictures::<skia_safe::Image>::new(),
    );
    let dark = pixel(&ctx.rgba(), W, 100, 60);
    // ffa94d at 45 % over transparent: unpremultiplied it is the ink itself.
    for (got, want) in dark[..3].iter().zip([0xff, 0xa9, 0x4d]) {
        assert!((i32::from(*got) - want).abs() <= 1, "{dark:?}");
    }
    assert!((i32::from(dark[3]) - 115).abs() <= 2, "{dark:?}");
}

#[test]
fn hidden_elements_are_left_out_and_a_new_revision_redraws() {
    let mut r = replay(&[&snapshot(1, &[el("a", 1, LINE)])]);
    r.seek(1.0);
    let scene = r.scene();
    let list = scene.ordered();
    let mut ctx = canvas(W, H);
    let measure = ctx.measurer();
    let mut renderer = BoardRenderer::new();
    let mut draw = |ctx: &mut board_draw::SkiaCanvas, hidden: &[String], revision: u64| {
        renderer.draw(
            ctx,
            &DrawOptions {
                elements: &list,
                revision,
                overlays: &[],
                hidden,
                viewport: Viewport::IDENTITY,
                width: f64::from(W),
                height: f64::from(H),
                dpr: 1.0,
                theme: BoardTheme::Light,
                measure: &*measure,
            },
            &StaticPictures::<skia_safe::Image>::new(),
        );
        ctx.rgba()
    };
    assert_eq!(pixel(&draw(&mut ctx, &[], 1), W, 100, 60)[3], 255);
    // The layer is kept for an unchanged key, and redrawn when the hidden set changes.
    assert_eq!(pixel(&draw(&mut ctx, &[], 1), W, 100, 60)[3], 255);
    assert_eq!(pixel(&draw(&mut ctx, &["a".into()], 1), W, 100, 60)[3], 0);
}

#[test]
fn a_sheet_shows_its_deck_page_fitted_or_a_crossed_frame_when_it_is_missing() {
    let deck = "a".repeat(26);
    let page = |id: &str, n: u32| {
        el(
            id,
            0,
            &format!(
                r#""type":"page","color":"ink","size":2,"x":0,"y":0,"w":1280,"h":720,"deck":"{deck}","n":{n}"#
            ),
        )
    };
    let png = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/corpus/deck.png"
    ))
    .unwrap();
    let mut pictures = StaticPictures::new();
    pictures.insert(&deck, 1, decode_image(&png).unwrap());
    let (w, h) = (1300, 740);
    let mut ready = replay(&[&snapshot(1, &[page("s", 1)])]);
    ready.seek(1.0);
    let with = paint_sized(&ready, &pictures, w, h);
    let mut missing = replay(&[&snapshot(1, &[page("s", 2)])]);
    missing.seek(1.0);
    let without = paint_sized(&missing, &pictures, w, h);
    // Where the board point (20, 20), inside the sheet and its picture, is in the frame.
    let probe = canvas(8, 8);
    let dump = layout_dump(&ready, &*probe.measurer(), f64::from(w), f64::from(h));
    let v = &dump["viewport"];
    let at = |axis: &str| {
        ((20.0 - v[axis].as_f64().unwrap()) * v["zoom"].as_f64().unwrap()).round() as u32
    };
    let (fx, fy) = (at("x"), at("y"));
    // The deck page is cream; a missing one leaves white paper with a grey mark in the middle.
    assert_eq!(pixel(&with, w, fx, fy)[..3], [0xff, 0xf7, 0xe6]);
    assert_eq!(pixel(&without, w, fx, fy)[..3], [255, 255, 255]);
    assert_ne!(pixel(&without, w, w / 2, h / 2)[..3], [255, 255, 255]);
}

#[test]
fn the_layout_dump_has_the_cameras_bounds_and_the_measured_lines() {
    let text = r#""type":"text","color":"ink","size":20,"x":10,"y":10,"text":"Xin chào\nb""#;
    let mut r = replay(&[&snapshot(1, &[el("t", 1, text), el("l", 2, LINE)])]);
    r.seek(1.0);
    let target = canvas(W, H);
    let measure = target.measurer();
    let dump = layout_dump(&r, &*measure, f64::from(W), f64::from(H));
    let elements = dump["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 2);
    let t = &elements[0];
    assert_eq!(
        (t["id"].as_str(), t["type"].as_str()),
        (Some("t"), Some("text"))
    );
    let lines = t["lines"].as_array().unwrap();
    assert_eq!(lines[0]["text"], "Xin chào");
    assert_eq!(
        lines[0]["width"].as_f64().unwrap(),
        measure.width("Xin chào", 20.0)
    );
    assert_eq!(t["bounds"][3].as_f64().unwrap(), 10.0 + 2.0 * 25.0);
    assert!(elements[1].get("lines").is_none());
    assert!(dump["viewport"]["zoom"].as_f64().unwrap() > 0.0);
    assert!(dump["overlays"].as_array().unwrap().is_empty());
}
