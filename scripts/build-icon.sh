#!/usr/bin/env bash
# Regenerate assets/icons/Skerry.icns from assets/icons/Skerry.svg using rsvg-convert and iconutil.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="assets/icons/Skerry.svg"
OUT="assets/icons/Skerry.icns"

if ! command -v rsvg-convert >/dev/null 2>&1; then
  echo "error: rsvg-convert is required (brew install librsvg)" >&2
  exit 1
fi
if ! command -v iconutil >/dev/null 2>&1; then
  echo "error: iconutil (macOS) is required" >&2
  exit 1
fi

TMPDIR_ICON="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_ICON"' EXIT
ICONSET="$TMPDIR_ICON/Skerry.iconset"
mkdir -p "$ICONSET"

for size in 16 32 128 256 512; do
  rsvg-convert -w "$size" -h "$size" "$SRC" -o "$ICONSET/icon_${size}x${size}.png"
  double=$((size * 2))
  rsvg-convert -w "$double" -h "$double" "$SRC" -o "$ICONSET/icon_${size}x${size}@2x.png"
done

iconutil -c icns "$ICONSET" -o "$OUT"
echo "built $OUT"
