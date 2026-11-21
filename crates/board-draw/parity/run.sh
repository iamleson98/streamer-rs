#!/usr/bin/env bash
# The parity check: renders the corpus with the web player's code (headless Chromium) and with the
# Rust board, then compares them (see ../README.md). Needs bun, node, a Rust toolchain, the
# lesson-replay submodule and Playwright's Chromium (`bunx playwright-core install chromium`,
# with `--with-deps` on a bare Linux machine).
#
#   PARITY_OUT=DIR      where the frames, the diffs and the report go (default: ./out)
#   Writes report.md and report.html (self-contained, all pictures inline) there too.
#   BOARD_FALLBACK_FONTS=DIR  where fallback-fonts.sh puts the fallback fonts (default: target/fallback-fonts)
#   REPLAY_COMMIT=SHA   the lesson-replay commit shown in the report; a container without git is given it
#                       from the host: REPLAY_COMMIT="$(git -C lesson-replay rev-parse HEAD)"
#   COMPARE_ARGS="..."  thresholds for the compare tool (--max-layout, --min-ssim, --max-off-percent)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
crate="$(dirname "$here")"
root="$(cd "$crate/../.." && pwd)"
out="${PARITY_OUT:-$here/out}"
corpus="$crate/tests/corpus"
fonts="$root/lesson-replay/static/fonts"
frames="$out/frames"
fallback="$("$crate/fallback-fonts.sh" "${BOARD_FALLBACK_FONTS:-$root/target/fallback-fonts}")"

[ -d "$fonts" ] || { echo "lesson-replay is not checked out: git submodule update --init lesson-replay" >&2; exit 1; }
rm -rf "$frames"
mkdir -p "$frames"

(cd "$here" && bun install --frozen-lockfile && bun bundle.ts "$out/bundle.js")
cd "$root"
cargo run --locked -q -p board-draw --example render_frames -- --corpus "$corpus" --out "$frames" --fonts "$fonts" --fallback-fonts "$fallback"
(cd "$here" && node render.mjs "$corpus" "$out/bundle.js" "$frames" "$fonts" "$fallback")
# shellcheck disable=SC2086
cargo run --locked -q -p board-draw --example compare -- \
  --corpus "$corpus" --frames "$frames" --report "$out/report.md" --html "$out/report.html" \
  --replay-commit "${REPLAY_COMMIT:-$(git -C "$root/lesson-replay" rev-parse HEAD 2>/dev/null || echo unknown)}" ${COMPARE_ARGS:-}
