//! The strip, the card, the screen and the camera tile drawn with the pinned board fonts.

use std::path::PathBuf;
use std::rc::Rc;

use board_draw::{BOARD_FONT_FILES, FontSet, SkiaCanvas};
use stage_draw::{CardText, Lang, StagePainter, Strings, blit_bgra};

fn fonts() -> Rc<FontSet> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lesson-replay/static/fonts");
    let files: Vec<Vec<u8>> = BOARD_FONT_FILES
        .iter()
        .map(|n| std::fs::read(dir.join(n)).unwrap())
        .collect();
    let faces: Vec<&[u8]> = files.iter().map(Vec::as_slice).collect();
    FontSet::new(&faces).unwrap()
}

fn frame(fonts: &Rc<FontSet>) -> (SkiaCanvas, Vec<u8>) {
    (
        SkiaCanvas::new(1280, 720, fonts.clone()).unwrap(),
        vec![0; 1280 * 720 * 4],
    )
}

fn pixel(buf: &[u8], x: usize, y: usize) -> [u8; 3] {
    let i = (y * 1280 + x) * 4;
    [buf[i + 2], buf[i + 1], buf[i]]
}

fn card(lang: Lang) -> CardText {
    CardText {
        title: "Nhập môn đại số: phương trình bậc hai".into(),
        teacher: "Cô Lan".into(),
        starts_in: Strings::of(lang)
            .get("replay.card.startsIn")
            .unwrap()
            .to_owned(),
    }
}

#[test]
fn the_strip_has_the_brand_and_the_card_covers_the_whole_frame() {
    let fonts = fonts();
    let painter = StagePainter::new(fonts.clone(), true, Some(&card(Lang::Vi))).unwrap();
    let (mut canvas, mut out) = frame(&fonts);
    painter.paint_card(canvas.sk_canvas(), 0, 1800);
    assert!(canvas.read_bgra(&mut out));
    // The strip's slate, the stage's gradient and the lit text of the title.
    assert_eq!(pixel(&out, 1200, 700), [0x0f, 0x17, 0x2a]);
    assert!(
        (0..624).any(|y| (0..1280).any(|x| pixel(&out, x, y)[0] > 200)),
        "no text drawn"
    );
    if std::env::var("STAGE_DRAW_PNG").is_ok() {
        std::fs::write(std::env::var("STAGE_DRAW_PNG").unwrap(), canvas.png()).unwrap();
    }
}

#[test]
fn the_countdown_changes_with_the_second_and_not_within_it() {
    let fonts = fonts();
    let painter = StagePainter::new(fonts.clone(), false, Some(&card(Lang::En))).unwrap();
    let draw = |i| {
        let (mut canvas, mut out) = frame(&fonts);
        painter.paint_card(canvas.sk_canvas(), i, 1800);
        assert!(canvas.read_bgra(&mut out));
        out
    };
    assert_eq!(draw(1), draw(29));
    assert_ne!(draw(0), draw(30));
}

#[test]
fn the_screen_is_black_without_a_picture_and_the_picture_otherwise() {
    let fonts = fonts();
    let painter = StagePainter::new(fonts.clone(), true, None).unwrap();
    let stage = painter.layout().stage;
    let red: Vec<u8> = [0u8, 0, 255, 255].repeat((stage.w * stage.h) as usize);
    let (mut canvas, mut out) = frame(&fonts);
    painter.paint_screen(canvas.sk_canvas(), None);
    painter.paint_strip(canvas.sk_canvas());
    canvas.read_bgra(&mut out);
    assert_eq!(pixel(&out, 10, 10), [0, 0, 0]);
    assert_eq!(pixel(&out, 1270, 700), [0x0f, 0x17, 0x2a]);
    painter.paint_screen(canvas.sk_canvas(), Some(&red));
    canvas.read_bgra(&mut out);
    assert_eq!(pixel(&out, 10, 10), [255, 0, 0]);
    let _ = blit_bgra;
}

#[test]
fn the_camera_is_a_rounded_tile_with_a_white_ring() {
    let fonts = fonts();
    let painter = StagePainter::new(fonts.clone(), true, None).unwrap();
    let tile = painter.layout().tile.unwrap();
    let green: Vec<u8> = [0u8, 255, 0, 255].repeat((tile.w * tile.h) as usize);
    let (mut canvas, mut out) = frame(&fonts);
    painter.paint_strip(canvas.sk_canvas());
    painter.paint_camera(canvas.sk_canvas(), &green);
    canvas.read_bgra(&mut out);
    let (cx, cy) = (
        (tile.x + tile.w / 2.0) as usize,
        (tile.y + tile.h / 2.0) as usize,
    );
    assert_eq!(pixel(&out, cx, cy), [0, 255, 0]);
    // The corner is outside the rounded clip: still the strip.
    assert_eq!(
        pixel(&out, tile.x as usize + 1, tile.y as usize + 1),
        [0x0f, 0x17, 0x2a]
    );
    assert_eq!(pixel(&out, cx, tile.y as usize), [255, 255, 255]);
}

#[test]
fn the_brand_name_is_heavier_than_regular_text() {
    let fonts = fonts();
    let (semi, regular) = (
        fonts.line_width("Học Vui", 40.0, 600.0),
        fonts.line_width("Học Vui", 40.0, 400.0),
    );
    assert!(semi > regular + 1.0, "{semi} vs {regular}");
}
