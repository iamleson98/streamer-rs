# board-draw

Draws a recorded lesson's whiteboard natively: the Rust side of the board that the web player
(`lesson-replay`, the submodule at the repository root) draws in a browser. It is the foundation of the
native render worker; nothing in the released images uses it yet.

- `replay-model` (a separate crate, no Skia) holds the board log, the scene merge, the replay clock, pointer
  previews and the camera. Every rule is a port of the web player's and is tested against its behaviour.
- `board-draw` has a small `Canvas` trait that mirrors the Canvas 2D calls the board makes, one backend on
  Skia (a CPU raster surface, text through `skia_safe::textlayout`), `BoardRenderer` (a port of
  `board/render.ts`: the cached layer, dot grid, sheets, overlays), `BoardPainter` and `layout_dump`.
- Fonts are an input: `FontSet::new(&[bytes...])` takes the woff2 files in the order of the board's font
  stack (`BOARD_FONT_FILES`, from the submodule's `static/fonts/`). There are no system fonts, so a glyph
  that no face has draws as nothing, the same on every machine.
- Pack elements (graphs, solids, symbols) decode but are not drawn yet.

## Parity with the web player

`parity/run.sh` renders a corpus of synthetic board logs (`tests/corpus/`, made by
`cargo run -p board-draw --example gen_corpus`) with the web player's own code, bundled from the submodule
and run in headless Chromium (Playwright 1.65.1), and with the Rust board, and compares them:

- the layout dump (`layout_dump`, the same JSON written by `parity/harness.ts`): text lines, measured widths,
  element bounds and the camera, within 0.5 px;
- the pixels: SSIM, PSNR, and the share of pixels off by more than 8/255 overall and outside the text boxes.

It writes `report.md` and a self-contained `report.html` (the three pictures of every frame, the numbers and the
mismatches; CI keeps it as the `render-parity-report` artifact). Every board change must add a scene to the
corpus. Chromium is started with `--font-render-hinting=none --enable-font-subpixel-positioning`: on Linux
at density 1 it rounds every glyph's advance to whole pixels, which a text layout engine cannot be asked to
copy, and those flags are what make its text layout the same on every machine. Calibrated on Linux (arm64,
Chromium 159): layout at most 0.33 px, SSIM at least 0.975, off-pixels outside text at most 0.22 %; the
thresholds (0.5 px, 0.97, 0.5 %) are in `examples/compare.rs`.

Needs bun, node, Playwright's Chromium (`bunx playwright-core install chromium` in `parity/`) and the
submodule: `git submodule update --init lesson-replay`.

## Building Skia

`skia-safe` is pinned exactly and takes a prebuilt Skia, which skia-bindings downloads from the
rust-skia/skia-binaries release of the same version, with no checksum. `skia-binaries.sh DIR` fetches that
one file, checks it against `skia-binaries.sha256` and prints the `SKIA_BINARIES_URL` that makes the build
use it (CI runs it before every build that links Skia). Without it a plain `cargo build` still works (it
downloads the same release). Linux needs `libfreetype-dev` and `libfontconfig-dev`; Skia reads the woff2 files
through the system's FreeType (Ubuntu's reads woff2).
