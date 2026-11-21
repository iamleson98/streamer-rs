#!/usr/bin/env bash
# The parity check: renders the corpus with the web player's code (headless Chromium) and with the
# Rust board, then compares them (see ../README.md). Needs bun, node, a Rust toolchain, the
# lesson-replay submodule and Playwright's Chromium (`bunx playwright-core install chromium`,
# with `--with-deps` on a bare Linux machine).
#
#   PARITY_OUT=DIR      where the frames, the diffs and the report go (default: ./out)
#   Writes report.md and report.html (self-contained, all pictures inline) there too.
#   COMPARE_ARGS="..."  thresholds for the compare tool (--max-layout, --min-ssim, --max-off-percent)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
crate="$(dirname "$here")"
root="$(cd "$crate/../.." && pwd)"
out="${PARITY_OUT:-$here/out}"
corpus="$crate/tests/corpus"
fonts="$root/lesson-replay/static/fonts"
frames="$out/frames"

[ -d "$fonts" ] || { echo "lesson-replay is not checked out: git submodule update --init lesson-replay" >&2; exit 1; }
mkdir -p "$frames"
rm -f "$frames"/*

(cd "$here" && bun install --frozen-lockfile && bun bundle.ts "$out/bundle.js")
cd "$root"
cargo run --locked -q -p board-draw --example render_frames -- --corpus "$corpus" --out "$frames" --fonts "$fonts"
(cd "$here" && node render.mjs "$corpus" "$out/bundle.js" "$frames" "$fonts")
# shellcheck disable=SC2086
cargo run --locked -q -p board-draw --example compare -- \
  --corpus "$corpus" --frames "$frames" --report "$out/report.md" --html "$out/report.html" \
  --replay-commit "$(git -C "$root/lesson-replay" rev-parse HEAD)" ${COMPARE_ARGS:-}
