# quiz-draw

Draws the quiz (race) and Quest stages of a recorded lesson natively, as the web player shows them in a recording
(`lesson-replay`, the submodule): the native render's replacement for the player's Svelte screens, which the
Chromium render paints through an SVG `foreignObject` today.

- `replay-model::quiz` (no Skia) holds the game: the quiz service's frames, the race and Quest state at any time of
  a recording (names of guests made neutral, as the player does), the avatar hash, the size steps, the countdown.
- `quiz-draw` lays each screen out with `taffy` (one tree per frame mirroring the component's DOM, with the values
  of its Tailwind classes), measures text with Skia's text layout (Inter at the player's weights, a fixed line height
  split as CSS splits it, `text-wrap: balance`, ellipsis) and paints it. Colours (`src/tokens.rs`) and pictures
  (`src/art_data.rs`) are generated from the player's CSS and SVG (`parity/tokens.mjs`, `parity/assets.ts`), the
  strings (`src/strings.json`) from its i18n, never typed in.
- The render's `cutPartialRows` (what a scrolling box would cut is left out of the video) is ported as it is, with the
  two layouts it works with: the page's (a 720 px viewport) decides what is hidden, the picture is made at the
  stage's own height. The reduced-motion render is what is drawn (no splash, no confetti, still rows).
- Stage seam: `QuizStage::new(records, locale, fonts)`, `on(t_ms)`, `draw(canvas, rect, t_ms)`; time moves forward.
  It holds `Rc`s (the fonts are `Rc<FontSet>`), so it is built on the thread that draws.

## Checks

- `tests/reducers.rs`: the reducers against lesson-replay's own, on the 37 made-up games of `tests/corpus`
  (`parity/gen.ts` writes them; `bun gen.ts --check` fails when the player now gives something else).
- `parity/run.sh`: the screens against the player's, in headless Chromium, the way the render paints them
  (`createQuizDom` + `DomPainter`, reduced motion), with the board's compare tool (groups "Quiz" and "Quest";
  text lines and painted boxes within 0.5 px, SSIM, pixels). `EXTRA=` puts another corpus in the same report. The
  frames of a game are dumped in the layout the image has (the stage's height), not the page's.

Known differences: on macOS Skia cannot draw Noto Color Emoji, so avatars are missing glyphs there and every frame
with one fails (Linux draws them). A tile with a growing bar is a composited layer in Chromium, whose text
anti-aliasing differs from ours by a few pixels. With about ten live standings rows the player's cut hides the whole
question (a defect of the player today, reproduced here; fix it in lesson-replay, then here).
