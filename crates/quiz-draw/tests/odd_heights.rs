//! The stage at heights the render never uses (the corpus is 624 and 664 px): the page's tree and the picture's
//! must correspond node for node whatever the media queries do (a short stage hides the title and the standings,
//! a tall one changes the tiles), so drawing and dumping neither panic nor leave rows out of step.

use std::{fs, path::PathBuf};

use board_draw::{BOARD_FONT_FILES, FontSet};
use quiz_draw::{Locale, QuizStage};
use replay_model::quiz::read_quiz_log;
use serde_json::Value;
use skia_safe::{Rect, surfaces};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

fn fonts() -> std::rc::Rc<FontSet> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lesson-replay/static/fonts");
    let files: Vec<Vec<u8>> = BOARD_FONT_FILES
        .iter()
        .map(|n| fs::read(dir.join(n)).unwrap_or_else(|e| panic!("{n}: {e}")))
        .collect();
    let faces: Vec<&[u8]> = files.iter().map(Vec::as_slice).collect();
    FontSet::new(&faces).expect("the fonts parse")
}

#[test]
fn every_scene_draws_and_dumps_at_any_stage_height() {
    let fonts = fonts();
    let index: Value =
        serde_json::from_str(&fs::read_to_string(corpus().join("index.json")).unwrap()).unwrap();
    let mut drawn = 0;
    for scene in index["scenes"].as_array().unwrap() {
        let log =
            fs::File::open(corpus().join(format!("{}.ndjson.gz", scene["log"].as_str().unwrap())))
                .unwrap();
        let records = read_quiz_log(log).unwrap();
        for height in [260, 459, 460, 560, 561, 620, 700, 701, 900] {
            let mut stage = QuizStage::new(records.clone(), Locale::En, fonts.clone());
            for t in scene["times"].as_array().unwrap() {
                let t = t.as_f64().unwrap();
                if !stage.on(t) {
                    continue;
                }
                let mut surface = surfaces::raster_n32_premul((1280, height)).unwrap();
                stage.draw(surface.canvas(), Rect::from_wh(1280.0, height as f32), t);
                let dump = stage.layout_dump(1280.0, height as f32, t);
                assert!(!dump.boxes.is_empty(), "{} at {height}", scene["name"]);
                drawn += 1;
            }
        }
    }
    assert!(drawn > 500, "{drawn} frames");
}
