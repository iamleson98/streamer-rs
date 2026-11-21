#!/usr/bin/env bash
# Fetches the fallback fonts of the native renderer (the faces that come after the board's own six,
# see README.md "Fallback fonts") into DIR and checks every file against fallback-fonts.sha256;
# any mismatch removes the file and fails. They are not in git for their size (about 26 MB).
#
#   crates/board-draw/fallback-fonts.sh "$RUNNER_TEMP/fallback-fonts"
#
# The script prints DIR. CI caches DIR keyed on fallback-fonts.sha256. Raising a font means a new
# URL here and a new checksum there, together; the version of each is in the comment beside it.
set -euo pipefail

# notofonts.github.io at one commit: the per-script repositories publish their fonts only inside
# release archives (NotoSans alone is 117 MB), this repository carries the same release files.
NOTO=https://raw.githubusercontent.com/notofonts/notofonts.github.io/ab2c1e2157930e8c211d6e26f52f68e6d52f02ad/fonts
EMOJI=https://raw.githubusercontent.com/googlefonts/noto-emoji/v2026-09-24-unicode18_0/2D/fonts
CJK=https://github.com/notofonts/noto-cjk/raw/Sans2.004/Sans/OTC

dir="${1:?usage: fallback-fonts.sh DIR}"
here="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$dir"
sum() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

fetch() { # FILE URL
  local file="$1" url="$2" want got
  want="$(awk -v f="$file" '$2 == f { print $1 }' "$here/fallback-fonts.sha256")"
  [ -n "$want" ] || { echo "$file has no checksum in fallback-fonts.sha256" >&2; exit 1; }
  if [ ! -f "$dir/$file" ] || [ "$(sum "$dir/$file")" != "$want" ]; then
    curl --fail --silent --show-error --location --retry 3 --output "$dir/$file" "$url" >&2
  fi
  got="$(sum "$dir/$file")"
  if [ "$got" != "$want" ]; then
    rm -f "$dir/$file"
    echo "checksum of $file is $got, expected $want" >&2
    exit 1
  fi
}

fetch NotoSans-Regular.ttf "$NOTO/NotoSans/full/ttf/NotoSans-Regular.ttf" # 2.015
fetch NotoSansSymbols-Regular.ttf "$NOTO/NotoSansSymbols/full/ttf/NotoSansSymbols-Regular.ttf" # 2.003
fetch NotoSansCJK-Regular.ttc "$CJK/NotoSansCJK-Regular.ttc" # Sans2.004
fetch NotoSansThai-Regular.ttf "$NOTO/NotoSansThai/full/ttf/NotoSansThai-Regular.ttf" # 2.002
fetch NotoSansLao-Regular.ttf "$NOTO/NotoSansLao/full/ttf/NotoSansLao-Regular.ttf" # 2.003
fetch NotoSansKhmer-Regular.ttf "$NOTO/NotoSansKhmer/full/ttf/NotoSansKhmer-Regular.ttf" # 2.004
fetch NotoSansArabic-Regular.ttf "$NOTO/NotoSansArabic/full/ttf/NotoSansArabic-Regular.ttf" # 2.013
fetch NotoSansHebrew-Regular.ttf "$NOTO/NotoSansHebrew/full/ttf/NotoSansHebrew-Regular.ttf" # 3.001
fetch NotoSansDevanagari-Regular.ttf "$NOTO/NotoSansDevanagari/full/ttf/NotoSansDevanagari-Regular.ttf" # 2.007
fetch Noto-COLRv1.ttf "$EMOJI/Noto-COLRv1.ttf" # 2.057 (release v2026-09-24-unicode18_0)
cd "$dir" && pwd
