# Image of the stream runner (the Rust workspace's stream-runner binary): the program that sends a
# rendered lesson to a live platform. Published by .github/workflows/image.yml as
# ghcr.io/<owner>/stream-runner on a version tag.
#
#   docker build -t stream-runner .
#   docker buildx build --platform linux/amd64,linux/arm64 -t stream-runner .
#
# The runtime image holds the runner and ffmpeg and nothing else that runs: it needs no writable
# root filesystem (run it with --read-only and a small tmpfs), and it runs as an unprivileged user.
# The secrets (the runner's token and private key) are never in the image; mount them read-only at
# /secrets.

# ────────────────────────────── Builder ──────────────────────────────
# The latest stable Rust when this was written, pinned by digest (dependabot moves tag and digest
# together; rust-toolchain.toml holds the same version, so raise both).
# The builder runs on the machine's own platform and cross-compiles for the one the image is for
# (BuildKit sets TARGETARCH from --platform): an arm64 laptop building the amd64 image needs no
# emulation, and on an amd64 runner nothing is cross-compiled. Debian 13, like the runtime image,
# so the binary meets the glibc it was linked against.
FROM --platform=$BUILDPLATFORM rust:1.99.1-trixie@sha256:8a4e90c7946b74626488b1446f712c0fe0ed1a596cbd3d871684bc9c5433ae07 AS builder
ARG TARGETARCH

WORKDIR /src
COPY . ./

# Layer-cache the registry downloads between builds; the build cache outlives the layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
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
    cargo build --release --locked -p stream-runner --target "${triple}"; \
    cp "target/${triple}/release/stream-runner" /stream-runner

# ────────────────────────────── Runtime ──────────────────────────────
# A dated tag and its digest (of the multi-platform index): the base never changes under a rebuild
# of the same commit. Raise both deliberately.
FROM debian:trixie-20261104-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2

# ffmpeg (and ffprobe, which ships in the same package) from the distribution, at the exact version
# the runner was tested with; ca-certificates: TLS to the site and the platform. Debian removes an
# old package version from its mirror when it ships a security update, and then this build fails
# until the pins are raised after reading the changelog: that is intended.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates=20250419+deb13u1 \
        ffmpeg=7:7.1.5-0+deb13u1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 2003 streamer \
    && useradd --system --no-create-home --uid 2003 --gid streamer --shell /usr/sbin/nologin streamer

COPY --from=builder /stream-runner /usr/local/bin/stream-runner

USER streamer
WORKDIR /

# The files are mounted; the API address comes from the environment.
ENV STREAM_RUNNER_TOKEN_FILE=/secrets/runner-token \
    STREAM_RUNNER_KEY_FILE=/secrets/runner-key.pem \
    STREAM_FFMPEG=/usr/bin/ffmpeg \
    STREAM_FFPROBE=/usr/bin/ffprobe \
    RUST_LOG=info

# The runner only calls out: nothing listens, so there is nothing to expose or to health-check.
# Whatever runs the container restarts it when it exits.
ENTRYPOINT ["/usr/local/bin/stream-runner"]
