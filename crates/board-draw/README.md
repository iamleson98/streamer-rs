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
  stack (`BOARD_FONT_FILES`, from the submodule's `static/fonts/`). There are no system fonts: behind the board's
  six faces come the fallback fonts below, and a glyph that no face has is drawn as the first face's
  missing-glyph box (counted by `FontSet::missing_glyphs`; the parity check fails any frame that has one).
- Pack elements (graphs, solids, symbols) decode but are not drawn yet.

## Parity with the web player

`parity/run.sh` renders a corpus of synthetic board logs (`tests/corpus/`, made by
`cargo run -p board-draw --example gen_corpus`) with the web player's own code, bundled from the submodule
and run in headless Chromium (Playwright 1.65.1), and with the Rust board, and compares them. The corpus has a
scene for everything the board can show, named for what it tests, in groups: strokes (colours, sizes,
highlighter, pressure, dots, 1,200 points, scribbles, sampling), shapes (every kind, size and direction,
zero and one pixel, the corner rule, short arrows), text (sizes, empty lines, 2,000 characters, Vietnamese,
every output of `typeset.ts`, all four fonts), other scripts (Chinese, Japanese, Korean, Thai, Lao, Khmer, Arabic
and Hebrew alone and mixed with Vietnamese, Hindi, emoji of every kind, a line of everything at three sizes), sheets and decks (blank, landscape, portrait and square
pictures, loading and failed), viewport (zoom 0.2 to 5, fractional pans, the grid threshold, culling, the fitted
camera), scene rules (deletes, version and nonce conflicts, clear, lock, snapshots, stacking, invalid
elements, seeking backwards against fresh replays), presence (growing and finished strokes, shape previews,
two people, expiry, ignored cursors and names), both stage sizes (1280x624 and 1280x664) and a dense scene of
about 800 elements. `index.json` says how each scene is sampled (times, stage size, camera, whether the
pictures arrive, whether the web side draws every frame from a fresh replay).

- the layout dump (`layout_dump`, the same JSON written by `parity/harness.ts`): text lines, measured widths,
  element bounds and the camera, within 0.5 px;
- the pixels: SSIM, PSNR, and the share of pixels off by more than 8/255 overall and outside the text boxes;
- no missing glyphs: the layout dump counts the glyphs that resolved to a missing-glyph box on the Rust side, and
  a frame with any fails.

It writes `report.md` and a self-contained `report.html` (grouped, with a pass/fail count per group, the
three pictures of every frame as thumbnails and at full size on tap for as many frames as `--max-mb` of
`compare` (14) leaves room for, the numbers and the mismatches; CI keeps it as the `render-parity-report`
artifact). Every board change must add a scene to the corpus. Chromium is started with
`--font-render-hinting=none --enable-font-subpixel-positioning`: on Linux at density 1 it rounds every glyph's
advance to whole pixels, which a text layout engine cannot be asked to copy, and those flags are what make its
text layout the same on every machine. The page loads every face of the board's stack before a scene is
measured, as the render page does. The "Other scripts" group gives Chromium the same fallback fonts, appended to
the board's stack as `@font-face` families in the harness only, so both sides draw with the same files. The thresholds (0.5 px, 0.97, 0.5 %) are in `examples/compare.rs`.

Needs bun, node, curl, Playwright's Chromium (`bunx playwright-core install chromium` in `parity/`) and the
submodule: `git submodule update --init lesson-replay`.

## Fallback fonts

The web keeps system fonts for what the board's fonts lack (CJK, Thai, Arabic, emoji...); the video has none, so
`FontSet::with_fallbacks` puts Noto behind the board's six faces (`FALLBACK_FONT_FILES`, in this order): Noto
Sans (full: Latin extended, Greek, Cyrillic) 2.015, Noto Sans Symbols 2.003, Noto Sans CJK Sans2.004 (the
`.ttc`, its Simplified Chinese face, index 2, for every Han character: it has the Han set of the others, and a
line carries no language to choose another by; kana and Hangul are in every face), Noto Sans Thai 2.002, Lao
2.003, Khmer 2.004, Arabic 2.013, Hebrew 3.001, Devanagari 2.007 and Noto Color Emoji 2.057 (the COLRv1 build).
A character comes from a fallback face only when no board face has it, so everything the board's fonts cover is
drawn as it is without them. A base and its combining marks, joiners and variation selectors are never cut
apart, an emoji sequence (ZWJ, skin tone, flag, keycap, VS16) is drawn whole from the emoji face, and
right-to-left runs are ordered inside a line by the text layout (Arabic joins, Hebrew points).

The files are not in git (about 26 MB). `fallback-fonts.sh DIR` downloads them (notofonts.github.io at one commit,
noto-cjk and noto-emoji at their release tags) and checks every file against `fallback-fonts.sha256`, failing on
any mismatch; tests and the parity job use it, and CI caches the directory by that file. The render image will run
it in the cutover slice. Raising a font means a new URL in the script and a new checksum, together.

What cannot match the web player: the player draws these scripts with the viewer's system font, so the video's
glyph shapes for them come from Noto and can look different from a given device (their placement follows the
same rules). Skia's CoreText font manager (macOS) cannot draw Noto Color Emoji (COLRv1 or bitmap), so on a Mac
the emoji face is left out and emoji count as missing glyphs; Linux (FreeType) draws it. CJK text is laid out as Chromium does it (read from Blink's `han_kerning.cc` and measured in Chromium 159, since Noto
Sans CJK has `halt` but no `chws`): `text-spacing-trim: normal` halves a full-width closing mark before another
mark and an opening mark after one (done with Skia `halt` font features on those characters only), and kana pairs
are kerned only where Chromium kerns them (a kana before a comma or full stop, the prolonged sound mark and the
middle dot), not the font's other kana pairs. Sweeps over every pair and every triple of kana, Han, Hangul and
punctuation, and 6,000 random lines, agree to 0.05 px except: the full-width semicolon next to another one or
a Latin quote or bracket (Chromium trims, the Rust side does not), a prolonged sound mark after a mark or a Latin
character followed by kana, and 0.28 px in a few katakana words (for example the pair in "スーパー"); they are in
the corpus ("scripts-chinese-punctuation") and the report shows them failing.

## Building Skia

`skia-safe` is pinned exactly and takes a prebuilt Skia, which skia-bindings downloads from the
rust-skia/skia-binaries release of the same version, with no checksum. `skia-binaries.sh DIR` fetches that
one file, checks it against `skia-binaries.sha256` and prints the `SKIA_BINARIES_URL` that makes the build
use it (CI runs it before every build that links Skia). Without it a plain `cargo build` still works (it
downloads the same release). Linux needs `libfreetype-dev` and `libfontconfig-dev`; Skia reads the woff2 files
through the system's FreeType (Ubuntu's reads woff2).
