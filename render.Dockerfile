# Image of the render worker (the Rust workspace's stream-render binary): the program that turns a
# recorded lesson into one video file, so it can be streamed. Published by .github/workflows/image.yml
# as ghcr.io/<owner>/stream-render on a version tag, together with the runner's image.
#
# Build from the repository root, with the submodules checked out (lesson-replay supplies the board's
# fonts, the countdown card's strings and the logo; its own nested submodule is not needed):
#   git submodule update --init
#   docker build -f render.Dockerfile -t stream-render .
#   docker buildx build -f render.Dockerfile --platform linux/amd64,linux/arm64 -t stream-render .
#
# What the image holds: the worker (Skia draws the frames in it), the fonts, and ffmpeg, which the
# worker runs as child processes to decode the recording's video, encode the result (x264) and mix
# the sound. There is no browser. Run it with a read-only root filesystem, no capabilities and
# `docker run --init`; it runs as the base image's unprivileged user and needs the work directory
# writable (a volume or a tmpfs: the render's files are the recording's videos, about the size of
# the recording). The render token is never in the image; mount it read-only at /secrets. The
# context is cut down by render.Dockerfile.dockerignore.

# ────────────────────────────── Fonts ──────────────────────────────
# The board's own faces come from lesson-replay (the page that draws the board in the browser); the
# fallback faces (CJK, Thai, Arabic, emoji...) are fetched and checked against fallback-fonts.sha256.
# The same fonts for every platform, so this stage runs on the machine's own.
FROM --platform=$BUILDPLATFORM ubuntu:24.04@sha256:9c7f200c07e3d4a03e7249d2c16bf66a51120fb876aa2242cf31451412543289 AS fonts
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY crates/board-draw/fallback-fonts.sh crates/board-draw/fallback-fonts.sha256 crates/board-draw/
RUN crates/board-draw/fallback-fonts.sh /fonts
COPY lesson-replay/static/fonts/*.woff2 /fonts/

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
# What the crates embed at compile time: the countdown card's strings and the logo.
COPY lesson-replay/src/lib/replay ./lesson-replay/src/lib/replay

# The build caches have ids of their own: the runner's Dockerfile builds other crates on Debian 13,
# and a cache shared by path would hand this build objects compiled against that glibc.
# Skia is the prebuilt binary of the one release skia-safe is pinned to, checked against
# skia-binaries.sha256 (the build never downloads one by itself); it links FreeType and fontconfig
# of the target's own, so their development files come for the target architecture.
RUN --mount=type=cache,id=render-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=render-cargo-target,target=/src/target \
    set -eux; \
    case "${TARGETARCH}" in \
      amd64) triple=x86_64-unknown-linux-gnu;  gcc_pkg=g++-x86-64-linux-gnu;  libc_pkg=libc6-dev-amd64-cross; gcc_bin=x86_64-linux-gnu-gcc; deb_arch=amd64 ;; \
      arm64) triple=aarch64-unknown-linux-gnu; gcc_pkg=g++-aarch64-linux-gnu; libc_pkg=libc6-dev-arm64-cross; gcc_bin=aarch64-linux-gnu-gcc; deb_arch=arm64 ;; \
      *) echo "unsupported TARGETARCH ${TARGETARCH}"; exit 1 ;; \
    esac; \
    rustup target add "${triple}"; \
    packages="libfreetype-dev:${deb_arch} libfontconfig-dev:${deb_arch}"; \
    if [ "$(uname -m)" != "${triple%%-*}" ]; then \
      dpkg --add-architecture "${deb_arch}"; \
      packages="${packages} ${gcc_pkg} ${libc_pkg}"; \
      upper="$(echo "${triple}" | tr 'a-z-' 'A-Z_')"; \
      export "CARGO_TARGET_${upper}_LINKER=${gcc_bin}" "CC_$(echo "${triple}" | tr '-' '_')=${gcc_bin}"; \
    fi; \
    apt-get update; \
    apt-get install -y --no-install-recommends ${packages}; \
    eval "$(SKIA_TRIPLE="${triple}" crates/board-draw/skia-binaries.sh /skia)"; \
    cargo build --release --locked -p stream-render --target "${triple}"; \
    cp "target/${triple}/release/stream-render" /stream-render

# ────────────────────────────── Runtime ──────────────────────────────
# Ubuntu, by tag and by the digest of its multi-platform index: the base never changes under a
# rebuild of the same commit.
FROM ubuntu:24.04@sha256:9c7f200c07e3d4a03e7249d2c16bf66a51120fb876aa2242cf31451412543289

# ffmpeg (and ffprobe, in the same package) from Ubuntu at the exact version the worker was
# measured with. Ubuntu removes an old package version from its mirror when it ships a security
# update, and then this build fails until the pin is raised after reading the changelog and
# re-running `render-compare`: that is intended. FreeType and fontconfig are what Skia links;
# CA certificates are what the worker checks the site's and the storage's certificates against.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg=7:6.1.1-3ubuntu5 libfreetype6 libfontconfig1 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && install -d -o 1001 -g 1001 /work

COPY --from=builder /stream-render /usr/local/bin/stream-render
COPY --from=fonts /fonts /fonts
# Every library the worker needs is in the image: a missing one fails the build, not a render.
RUN ! ldd /usr/local/bin/stream-render | grep 'not found'

USER 1001:1001
WORKDIR /work

# The files the unit mounts; the API address comes from the environment.
ENV RENDER_TOKEN_FILE=/secrets/render-token \
    RENDER_WORK_DIR=/work \
    RENDER_FONT_DIR=/fonts \
    RENDER_FFMPEG=/usr/bin/ffmpeg \
    RENDER_FFPROBE=/usr/bin/ffprobe \
    RUST_LOG=info

# The worker only calls out: nothing is exposed and there is nothing to health-check. Whatever runs
# the container restarts it when it exits.
ENTRYPOINT ["/usr/local/bin/stream-render"]
