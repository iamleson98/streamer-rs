# Image of the render worker (the Rust workspace's stream-render binary and the render page of
# lesson-replay): the program that turns a recorded lesson into one video file, with a headless
# browser, so it can be streamed. Published by .github/workflows/image.yml as
# ghcr.io/<owner>/stream-render on a version tag, together with the runner's image.
#
# Build from the repository root, with the submodules checked out (lesson-replay has one of its own,
# vendor/sui):
#   git submodule update --init --recursive
#   docker build -f render.Dockerfile -t stream-render .
#   docker buildx build -f render.Dockerfile --platform linux/amd64,linux/arm64 -t stream-render .
#
# What the image holds: the worker, the page it opens (lesson-replay's static app; the worker serves
# it to the browser itself, nothing of it is served to anyone else), the headless Chromium shell and
# fonts of the Playwright base image, and ffmpeg. Run it with a read-only root filesystem, no
# capabilities but CAP_SYS_CHROOT and a seccomp profile that allows user namespaces (the browser's
# own sandbox needs them); it runs as the base image's unprivileged user. The render token is never
# in the image; mount it read-only at /secrets. The context is cut down by render.Dockerfile.dockerignore.

# ────────────────────────────── Page ──────────────────────────────
# bun, pinned by tag and by the digest of its multi-platform index. The page is static, so the build
# runs on the machine's own platform whatever the image is for. No build cache outlives the build: a
# release is made from scratch, from the lockfile, so nothing left over can reach the signed image.
FROM --platform=$BUILDPLATFORM oven/bun:1.4.4-alpine@sha256:4cf82f5686b196504fe119db3fcd9185b191f82ece82c40680e597228ec6070a AS web

WORKDIR /web
COPY lesson-replay/package.json lesson-replay/bun.lock lesson-replay/.npmrc ./
RUN bun install --frozen-lockfile
COPY lesson-replay/ ./
# The worker serves the files as they are; precompressed copies would only be dead weight.
RUN bun run build \
    && find build -type f \( -name '*.br' -o -name '*.gz' \) -delete \
    && test -f build/200.html

# ────────────────────────────── Worker ──────────────────────────────
# Cross-compiled like the runner's Dockerfile, but on Debian 12 (glibc 2.36): the runtime image is
# Ubuntu 24.04 (glibc 2.39), and a binary linked on Debian 13 (glibc 2.41) would not start there.
# Pinned by digest (dependabot moves tag and digest together; rust-toolchain.toml holds the same
# version, so raise both).
FROM --platform=$BUILDPLATFORM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS builder
ARG TARGETARCH

WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates

# The build caches have ids of their own: the runner's Dockerfile builds the same crates on Debian 13,
# and a cache shared by path would hand this build objects compiled against that glibc (the link then
# fails on the symbols Debian 12 lacks).
RUN --mount=type=cache,id=render-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=render-cargo-target,target=/src/target \
    set -eux; \
    case "${TARGETARCH}" in \
      amd64) triple=x86_64-unknown-linux-gnu;  gcc_pkg=gcc-x86-64-linux-gnu;  libc_pkg=libc6-dev-amd64-cross; gcc_bin=x86_64-linux-gnu-gcc ;; \
      arm64) triple=aarch64-unknown-linux-gnu; gcc_pkg=gcc-aarch64-linux-gnu; libc_pkg=libc6-dev-arm64-cross; gcc_bin=aarch64-linux-gnu-gcc ;; \
      *) echo "unsupported TARGETARCH ${TARGETARCH}"; exit 1 ;; \
    esac; \
    rustup target add "${triple}"; \
    if [ "$(uname -m)" != "${triple%%-*}" ]; then \
      apt-get update; \
      apt-get install -y --no-install-recommends "${gcc_pkg}" "${libc_pkg}"; \
      upper="$(echo "${triple}" | tr 'a-z-' 'A-Z_')"; \
      export "CARGO_TARGET_${upper}_LINKER=${gcc_bin}" "CC_$(echo "${triple}" | tr '-' '_')=${gcc_bin}"; \
    fi; \
    cargo build --release --locked -p stream-render --target "${triple}"; \
    cp "target/${triple}/release/stream-render" /stream-render

# ────────────────────────────── Runtime ──────────────────────────────
# Playwright's own image (Ubuntu 24.04 with the browsers' system libraries, fonts and the headless
# Chromium shell), by tag and by the digest of its multi-platform index: the base never changes under
# a rebuild of the same commit. Its version is the one the render page was measured with; raise the
# tag and the digest together, and run the real-browser test (crates/stream-render/tests/real_browser.rs)
# against the new shell before shipping.
FROM mcr.microsoft.com/playwright:v1.65.1-noble@sha256:1507cbfc550a63a6b7396791ca4f00df17beb56342d2447b3f106c306f525579

# ffmpeg (and ffprobe, in the same package) from Ubuntu at the exact version the worker was tested
# with. Ubuntu removes an old package version from its mirror when it ships a security update, and
# then this build fails until the pin is raised after reading the changelog: that is intended.
# The headless shell is linked under one name whatever the architecture's directory is called.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg=7:6.1.1-3ubuntu5 \
    && rm -rf /var/lib/apt/lists/* \
    && shell="$(ls -d /ms-playwright/chromium_headless_shell-*/chrome-headless-shell-linux*/chrome-headless-shell)" \
    && [ "$(echo "${shell}" | wc -l)" = 1 ] \
    && ln -s "${shell}" /usr/local/bin/headless-shell \
    && install -d -o pwuser -g pwuser /work

COPY --from=builder /stream-render /usr/local/bin/stream-render
COPY --from=web /web/build /srv/web

USER pwuser
WORKDIR /work

# The files the unit mounts; the API address comes from the environment.
ENV RENDER_TOKEN_FILE=/secrets/render-token \
    RENDER_WEB_DIR=/srv/web \
    RENDER_WORK_DIR=/work \
    RENDER_BROWSER=/usr/local/bin/headless-shell \
    RENDER_FFMPEG=/usr/bin/ffmpeg \
    RENDER_FFPROBE=/usr/bin/ffprobe \
    RUST_LOG=info

# The worker only calls out (its local server is on 127.0.0.1 inside the container): nothing is
# exposed and there is nothing to health-check. Whatever runs the container restarts it when it exits.
ENTRYPOINT ["/usr/local/bin/stream-render"]
