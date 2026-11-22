# AGENTS.md

Instructions for coding agents working in this repository. Humans start at `README.md`.

## What this is

A public Rust workspace (`crates/stream-common`, `stream-runner`, `stream-render`, the native renderer's
`replay-model`, `board-draw`, `stage-draw` and `quiz-draw`, and the measuring tools `render-compare`,
`fake-site` and `report-page`) and the two container images built from it: `stream-runner` (`Dockerfile`) and
`stream-render` (`render.Dockerfile`: the worker, fonts and ffmpeg, no browser; the board's fonts, the card's
strings and the logo come from the submodule `lesson-replay/`). Both are published, signed and released from one `vX.Y.Z` tag by
`.github/workflows/image.yml`. The private site that uses them is a different repository; nothing in this one may
name a secret, a host or a deployment of it.

## Rules

- Never edit `lesson-replay/`: it is a submodule of [iamleson98/lesson-replay](https://github.com/iamleson98/lesson-replay)
  (with a submodule of its own). Change the page there, merge it to its `main`, then move the pointer here. A release
  refuses a pin that is not on lesson-replay's `main`.
- Dependencies, toolchain, base images and actions are pinned exactly (images by digest, actions by commit SHA with the
  version in a comment). Raise `rust-toolchain.toml`, `Dockerfile` and `render.Dockerfile` together.
- `board-draw` is a port of the web player's board drawing: change the page in `lesson-replay` first, then the port,
  and add a scene to the parity corpus (`crates/board-draw/README.md`). Skia's prebuilt binary is fetched and
  checked by `crates/board-draw/skia-binaries.sh`; raise it together with `skia-safe`. The Noto fallback fonts (CJK, Thai, Arabic, emoji...) are fetched and
  checked by `crates/board-draw/fallback-fonts.sh` against `fallback-fonts.sha256`, not kept in git.
- Keep the release body's two `image: ghcr.io/<owner>/<name>@sha256:<digest>` lines as they are: the site's updater
  picks its image by them.
- Keep the job names `Test`, `Format and lints` and `Dependency audit` (the branch ruleset requires them).
- No build cache in release builds; secrets never go into the repository, logs or image.
- Conventional Commits (`type(scope): summary`, the body says why). Branch from `main`; open a pull request; never
  commit to `main`. Stage explicit paths.

## Verify before claiming done

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
crates/board-draw/parity/run.sh   # needs bun, Playwright's Chromium, the submodule (see board-draw's README)
actionlint .github/workflows/*.yml
git submodule update --init --recursive
docker buildx build -f render.Dockerfile --platform linux/amd64 --load -t stream-render:local .
docker build -t stream-runner:local .
```
