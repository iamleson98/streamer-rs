//! Renders the quiz corpus with the Rust stage: one PNG and one layout dump per sampled time at which a game is
//! on the stage (the others are not drawn as a game, as in the render).
//!
//!   cargo run -p quiz-draw --example render_frames --locked -- --corpus DIR --out DIR [--fonts DIR] [--fallback-fonts DIR]

mod support;

use std::path::PathBuf;

use quiz_draw::{Locale, QuizStage};
use replay_model::quiz::read_quiz_log;
use serde_json::Value;
use skia_safe::{AlphaType, ColorType, ImageInfo, Rect, surfaces};

fn main() {
    let corpus = PathBuf::from(support::required("--corpus"));
    let out = PathBuf::from(support::required("--out"));
    std::fs::create_dir_all(&out).unwrap();
    let only = support::opt("--only");
    let index: Value =
        serde_json::from_slice(&std::fs::read(corpus.join("index.json")).unwrap()).unwrap();
    let fonts = support::fonts_with_fallbacks();
    for scene in index["scenes"].as_array().unwrap() {
        let name = scene["name"].as_str().unwrap();
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        let (width, height) = (
            scene["width"].as_u64().unwrap() as i32,
            scene["height"].as_u64().unwrap() as i32,
        );
        let locale = if scene["locale"] == "vi" {
            Locale::Vi
        } else {
            Locale::En
        };
        let log = std::fs::File::open(
            corpus.join(format!("{}.ndjson.gz", scene["log"].as_str().unwrap())),
        )
        .unwrap();
        let mut stage = QuizStage::new(read_quiz_log(log).unwrap(), locale, fonts.clone());
        for t in scene["times"].as_array().unwrap() {
            let t = t.as_u64().unwrap();
            if !stage.on(t as f64) {
                continue;
            }
            let mut surface = surfaces::raster_n32_premul((width, height)).unwrap();
            surface.canvas().clear(skia_safe::Color::TRANSPARENT);
            stage.draw(
                surface.canvas(),
                Rect::from_wh(width as f32, height as f32),
                t as f64,
            );
            let info = ImageInfo::new(
                (width, height),
                ColorType::RGBA8888,
                AlphaType::Unpremul,
                None,
            );
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            assert!(surface.read_pixels(&info, &mut pixels, (width * 4) as usize, (0, 0)));
            std::fs::write(
                out.join(format!("{name}-{t}.rust.png")),
                board_draw::encode_png(width as u32, height as u32, &pixels).unwrap(),
            )
            .unwrap();
            if support::opt("--tree").is_some() {
                std::fs::write(
                    out.join(format!("{name}-{t}.rust.tree")),
                    stage.debug_tree(width as f32, height as f32, t as f64),
                )
                .unwrap();
            }
            let dump = stage.layout_dump(width as f32, height as f32, t as f64);
            std::fs::write(
                out.join(format!("{name}-{t}.rust.json")),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "texts": dump.texts, "boxes": dump.boxes, "missing_glyphs": dump.missing,
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }
}
