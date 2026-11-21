#!/usr/bin/env bash
# The quiz parity check: draws the quiz corpus with the web player's own screens (headless Chromium, the way the
# render paints them) and with the Rust stage, then compares them with the board's compare tool (see ../README.md).
# Needs bun, node, a Rust toolchain, the lesson-replay submodule (with sui) and Playwright's Chromium
# (`bunx playwright-core install chromium` in this folder, with `--with-deps` on a bare Linux machine).
#
#   PARITY_OUT=DIR      where the frames, the diffs and the report go (default: ./out)
#   BOARD_FALLBACK_FONTS=DIR  where fallback-fonts.sh puts the fallback fonts (default: target/fallback-fonts)
#   EXTRA=DIR           another corpus to put in the same report (the board's, tests/corpus of board-draw)
#   COMPARE_ARGS="..."  thresholds for the compare tool (--max-layout, --min-ssim, --max-off-percent)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
crate="$(dirname "$here")"
root="$(cd "$crate/../.." && pwd)"
out="${PARITY_OUT:-$here/out}"
corpus="$crate/tests/corpus"
lesson="$root/lesson-replay"
fonts="$lesson/static/fonts"
frames="$out/frames"
fallback="$("$crate/../board-draw/fallback-fonts.sh" "${BOARD_FALLBACK_FONTS:-$root/target/fallback-fonts}")"

[ -f "$lesson/vendor/sui/package.json" ] || [ -d "$lesson/vendor/sui/src" ] || {
  echo "lesson-replay or its sui is not checked out: git submodule update --init --recursive lesson-replay" >&2
  exit 1
}
rm -rf "$frames" "$out/bundle"
mkdir -p "$frames" "$out/bundle"

(cd "$lesson" && bun install --frozen-lockfile)
(cd "$here" && bun install --frozen-lockfile)
# The generated files are the player's own: they must be what its code gives now.
(cd "$here" && bun gen.ts --check "$corpus")
(cd "$here/../../../lesson-replay" && OUT_DIR="$out/bundle" bun node_modules/vite/bin/vite.js build --config "$here/vite.config.mjs")
(cd "$here" && node tokens.mjs --check "$out/bundle" "$crate/src/tokens.rs" && bun assets.ts --check "$crate/src/art_data.rs")

cd "$root"
cargo run --locked -q -p quiz-draw --example render_frames -- --corpus "$corpus" --out "$frames" --fonts "$fonts" --fallback-fonts "$fallback"
(cd "$here" && node render.mjs "$corpus" "$out/bundle" "$frames" "$fonts" "$fallback")
extra=()
[ -z "${EXTRA:-}" ] || extra=(--extra-corpus "$EXTRA")
# shellcheck disable=SC2086
cargo run --locked -q -p board-draw --example compare -- \
  --corpus "$corpus" ${extra[@]+"${extra[@]}"} --frames "$frames" --report "$out/report.md" --html "$out/report.html" \
  --replay-commit "$(git -C "$lesson" rev-parse HEAD)" ${COMPARE_ARGS:-}
