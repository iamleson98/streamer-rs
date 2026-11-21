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
  that no face has is drawn as the first face's missing-glyph box, the same on every machine (a browser takes it
  from a system font, so the parity corpus only uses characters the fonts have).
- Pack elements (graphs, solids, symbols) decode but are not drawn yet.

## Parity with the web player

`parity/run.sh` renders a corpus of synthetic board logs (`tests/corpus/`, made by
`cargo run -p board-draw --example gen_corpus`) with the web player's own code, bundled from the submodule
and run in headless Chromium (Playwright 1.65.1), and with the Rust board, and compares them. The corpus has a
scene for everything the board can show, named for what it tests, in groups: strokes (colours, sizes,
highlighter, pressure, dots, 1,200 points, scribbles, sampling), shapes (every kind, size and direction,
zero and one pixel, the corner rule, short arrows), text (sizes, empty lines, 2,000 characters, Vietnamese,
every output of `typeset.ts`, all four fonts), sheets and decks (blank, landscape, portrait and square
pictures, loading and failed), viewport (zoom 0.2 to 5, fractional pans, the grid threshold, culling, the fitted
camera), scene rules (deletes, version and nonce conflicts, clear, lock, snapshots, stacking, invalid
elements, seeking backwards against fresh replays), presence (growing and finished strokes, shape previews,
two people, expiry, ignored cursors and names), both stage sizes (1280x624 and 1280x664) and a dense scene of
about 800 elements. `index.json` says how each scene is sampled (times, stage size, camera, whether the
pictures arrive, whether the web side draws every frame from a fresh replay).

- the layout dump (`layout_dump`, the same JSON written by `parity/harness.ts`): text lines, measured widths,
  element bounds and the camera, within 0.5 px;
- the pixels: SSIM, PSNR, and the share of pixels off by more than 8/255 overall and outside the text boxes.

It writes `report.md` and a self-contained `report.html` (grouped, with a pass/fail count per group, the
three pictures of every frame as thumbnails and at full size on tap for as many frames as `--max-mb` of
`compare` (14) leaves room for, the numbers and the mismatches; CI keeps it as the `render-parity-report`
artifact). Every board change must add a scene to the corpus. Chromium is started with
`--font-render-hinting=none --enable-font-subpixel-positioning`: on Linux at density 1 it rounds every glyph's
advance to whole pixels, which a text layout engine cannot be asked to copy, and those flags are what make its
text layout the same on every machine. The page loads every face of the board's stack before a scene is
measured, as the render page does. A character from none of the four fonts is left out of the corpus: a
browser takes it from a system font. The thresholds (0.5 px, 0.97, 0.5 %) are in `examples/compare.rs`.

Needs bun, node, Playwright's Chromium (`bunx playwright-core install chromium` in `parity/`) and the
submodule: `git submodule update --init lesson-replay`.

## Building Skia

`skia-safe` is pinned exactly and takes a prebuilt Skia, which skia-bindings downloads from the
rust-skia/skia-binaries release of the same version, with no checksum. `skia-binaries.sh DIR` fetches that
one file, checks it against `skia-binaries.sha256` and prints the `SKIA_BINARIES_URL` that makes the build
use it (CI runs it before every build that links Skia). Without it a plain `cargo build` still works (it
downloads the same release). Linux needs `libfreetype-dev` and `libfontconfig-dev`; Skia reads the woff2 files
through the system's FreeType (Ubuntu's reads woff2).
