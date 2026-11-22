#!/usr/bin/env bash
# Fetches the prebuilt Skia that skia-bindings would download, from the one release it is pinned to,
# and checks it against skia-binaries.sha256, so a build never runs a binary nobody has checked.
# skia-bindings has no checksum of its own; it takes a URL template in SKIA_BINARIES_URL, so this
# puts the checked file in DIR and prints the line that points the build at it:
#
#   eval "$(crates/board-draw/skia-binaries.sh "$RUNNER_TEMP/skia")"        # then cargo build ...
#
# In GitHub Actions the line is appended to $GITHUB_ENV as well. Raising skia-safe means raising
# SKIA_TAG and the file names (their key changes), and the checksums, together: the release's asset
# list shows each file's sha256 (https://github.com/rust-skia/skia-binaries/releases/tag/<tag>).
set -euo pipefail

SKIA_TAG=0.157.0
KEY=30669978c3f39a73e8a7
FEATURES=jpegd-jpege-pdf-textlayout # skia-safe's features in the workspace Cargo.toml

dir="${1:?usage: skia-binaries.sh DIR}"
here="$(cd "$(dirname "$0")" && pwd)"
# SKIA_TRIPLE names the build's target when it is not this machine (a cross build in an image).
case "${SKIA_TRIPLE:-$(uname -s)-$(uname -m)}" in
  x86_64-unknown-linux-gnu | Linux-x86_64) triple=x86_64-unknown-linux-gnu ;;
  aarch64-unknown-linux-gnu | Linux-aarch64 | Linux-arm64) triple=aarch64-unknown-linux-gnu ;;
  Darwin-arm64) triple=aarch64-apple-darwin ;;
  *) echo "no pinned Skia binary for ${SKIA_TRIPLE:-$(uname -s)-$(uname -m)}" >&2; exit 1 ;;
esac
file="skia-binaries-$KEY-$triple-$FEATURES.tar.gz"
want="$(awk -v f="$file" '$2 == f { print $1 }' "$here/skia-binaries.sha256")"
[ -n "$want" ] || { echo "$file has no checksum in skia-binaries.sha256" >&2; exit 1; }

mkdir -p "$dir"
curl --fail --silent --show-error --location --retry 3 \
  --output "$dir/$file" "https://github.com/rust-skia/skia-binaries/releases/download/$SKIA_TAG/$file" >&2
if command -v sha256sum >/dev/null; then got="$(sha256sum "$dir/$file" | cut -d' ' -f1)"; else got="$(shasum -a 256 "$dir/$file" | cut -d' ' -f1)"; fi
if [ "$got" != "$want" ]; then
  rm -f "$dir/$file"
  echo "checksum of $file is $got, expected $want" >&2
  exit 1
fi
# skia-bindings replaces {key} with its own key (the part of the file name between the prefix and .tar.gz).
line="SKIA_BINARIES_URL=file://$(cd "$dir" && pwd)/skia-binaries-{key}.tar.gz"
[ -z "${GITHUB_ENV:-}" ] || echo "$line" >> "$GITHUB_ENV"
echo "export $line"
