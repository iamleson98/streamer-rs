//! Renders the parity corpus with the Rust board: one PNG and one layout dump per sampled time.
//!
//!   cargo run -p board-draw --example render_frames --locked -- --corpus DIR --out DIR [--fonts DIR] [--fallback-fonts DIR]

mod support;

use std::path::PathBuf;

use board_draw::{
    BoardPainter, BoardTheme, PictureState, Pictures, SkiaCanvas, StaticPictures, decode_image,
    layout_dump_at, missing_glyphs,
};
use replay_model::{BoardReplay, Viewport, board_viewport, read_board_log};
use serde_json::Value;

/// Pictures that never arrive: every sheet shows its ring.
struct Loading;

impl Pictures<skia_safe::Image> for Loading {
    fn revision(&self) -> u64 {
        0
    }

    fn get(&self, _: &str, _: u32) -> PictureState<skia_safe::Image> {
        PictureState::Loading
    }
}

fn main() {
    let corpus = PathBuf::from(support::required("--corpus"));
    let out = PathBuf::from(support::required("--out"));
    std::fs::create_dir_all(&out).unwrap();
    let index: Value =
        serde_json::from_slice(&std::fs::read(corpus.join("index.json")).unwrap()).unwrap();
    let mut ready = StaticPictures::new();
    for (key, file) in index["pictures"].as_object().unwrap() {
        let (deck, n) = key.split_once('/').unwrap();
        let image = decode_image(&std::fs::read(corpus.join(file.as_str().unwrap())).unwrap())
            .expect("a deck picture decodes");
        ready.insert(deck, n.parse().unwrap(), image);
    }
    let fonts = support::fonts_with_fallbacks();
    for scene in index["scenes"].as_array().unwrap() {
        let name = scene["name"].as_str().unwrap();
        let (width, height) = (
            scene["width"].as_u64().unwrap() as u32,
            scene["height"].as_u64().unwrap() as u32,
        );
        let mut target = SkiaCanvas::new(width, height, fonts.clone()).unwrap();
        let mut painter = BoardPainter::new(&target, width, height);
        let log = std::fs::File::open(
            corpus.join(format!("{}.ndjson.gz", scene["log"].as_str().unwrap())),
        )
        .unwrap();
        let mut replay = BoardReplay::new(read_board_log(log).unwrap());
        let (w, h) = (f64::from(width), f64::from(height));
        for t in scene["times"].as_array().unwrap() {
            let t = t.as_u64().unwrap();
            replay.seek(t as f64);
            let viewport = match scene["view"].as_array() {
                Some(v) => Viewport {
                    x: v[0].as_f64().unwrap(),
                    y: v[1].as_f64().unwrap(),
                    zoom: v[2].as_f64().unwrap(),
                },
                None => board_viewport(replay.scene(), painter.measure(), w, h),
            };
            if scene["loading"] == true {
                painter.paint_with(&mut target, (0.0, 0.0), &replay, &Loading, viewport);
            } else {
                painter.paint_with(&mut target, (0.0, 0.0), &replay, &ready, viewport);
            }
            std::fs::write(out.join(format!("{name}-{t}.rust.png")), target.png()).unwrap();
            let mut dump = layout_dump_at(&replay, &target, BoardTheme::Light, w, h, viewport);
            dump["missing_glyphs"] =
                missing_glyphs(&replay, |line, size| fonts.missing_glyphs(line, size)).into();
            std::fs::write(
                out.join(format!("{name}-{t}.rust.json")),
                serde_json::to_vec_pretty(&dump).unwrap(),
            )
            .unwrap();
        }
    }
}
