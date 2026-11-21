//! Renders the parity corpus with the Rust board: one PNG and one layout dump per sampled time.
//!
//!   cargo run -p board-draw --example render_frames --locked -- --corpus DIR --out DIR [--fonts DIR]

mod support;

use std::path::PathBuf;

use board_draw::{BoardPainter, SkiaCanvas, StaticPictures, decode_image, layout_dump};
use replay_model::{BoardReplay, read_board_log};
use serde_json::Value;

fn main() {
    let corpus = PathBuf::from(support::required("--corpus"));
    let out = PathBuf::from(support::required("--out"));
    std::fs::create_dir_all(&out).unwrap();
    let index: Value =
        serde_json::from_slice(&std::fs::read(corpus.join("index.json")).unwrap()).unwrap();
    let (width, height) = (
        index["width"].as_u64().unwrap() as u32,
        index["height"].as_u64().unwrap() as u32,
    );
    let mut target = SkiaCanvas::new(width, height, support::fonts()).unwrap();
    let mut painter = BoardPainter::new(&target, width, height);
    let mut pictures = StaticPictures::new();
    let deck =
        decode_image(&std::fs::read(corpus.join("deck.png")).unwrap()).expect("deck.png decodes");
    pictures.insert(index["deck"].as_str().unwrap(), 1, deck);
    for scene in index["scenes"].as_array().unwrap() {
        let name = scene["name"].as_str().unwrap();
        let log = std::fs::File::open(corpus.join(format!("{name}.ndjson.gz"))).unwrap();
        let mut replay = BoardReplay::new(read_board_log(log).unwrap());
        for t in scene["times"].as_array().unwrap() {
            let t = t.as_u64().unwrap();
            replay.seek(t as f64);
            painter.paint(&mut target, (0.0, 0.0), &replay, &pictures);
            std::fs::write(out.join(format!("{name}-{t}.rust.png")), target.png()).unwrap();
            let dump = layout_dump(
                &replay,
                painter.measure(),
                f64::from(width),
                f64::from(height),
            );
            std::fs::write(
                out.join(format!("{name}-{t}.rust.json")),
                serde_json::to_vec_pretty(&dump).unwrap(),
            )
            .unwrap();
        }
    }
}
