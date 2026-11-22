#!/usr/bin/env bash
# Build a release binary and package Skerry.app, .dmg, .zip, and SHA256SUMS in dist/.
#
# Signing & notarization (optional env-var hooks):
#   Default (free): ad-hoc signed with `codesign --force --deep --sign -` (not notarized).
#   Developer ID:   set CODESIGN_IDENTITY="Developer ID Application: Name (TEAMID)"
#   Notarization:   with CODESIGN_IDENTITY set, also set either:
#                     NOTARYTOOL_PROFILE="<keychain-profile>"
#                   or all three of:
#                     APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#
# Usage:
#   scripts/bundle.sh [--app-only] [--target <rust-target-triple>]
set -euo pipefail
cd "$(dirname "$0")/.."

PACKAGE="skerry-mac"
APP_NAME="Skerry"
BUNDLE_ID="dev.kebot.skerry"
ICON_SRC="assets/icons/Skerry.icns"
APP_ONLY=0
TARGET_TRIPLE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app-only)
      APP_ONLY=1
      shift
      ;;
    --target)
      TARGET_TRIPLE="${2:?missing value for --target}"
      shift 2
      ;;
    skerry-mac)
      # Backward-compatible positional argument
      shift
      ;;
    -h|--help)
      sed -n '2,15p' "$0" | sed 's/^# {0,1}//'
      exit 0
      ;;
    *)
      echo "usage: $0 [--app-only] [--target <rust-target-triple>]" >&2
      exit 2
      ;;
  esac
done

VERSION="${SKERRY_VERSION:-$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == sys.argv[1]))' "$PACKAGE")}"

CARGO_ARGS=(build --release -p "$PACKAGE")
if [[ -n "$TARGET_TRIPLE" ]]; then
  CARGO_ARGS+=(--target "$TARGET_TRIPLE")
  BIN_PATH="target/$TARGET_TRIPLE/release/$PACKAGE"
  case "$TARGET_TRIPLE" in
    aarch64-apple-darwin) ARCH="arm64" ;;
    x86_64-apple-darwin) ARCH="x86_64" ;;
    *) ARCH="$TARGET_TRIPLE" ;;
  esac
else
  BIN_PATH="target/release/$PACKAGE"
  ARCH="$(uname -m)"
fi

cargo "${CARGO_ARGS[@]}"

mkdir -p dist
APP="dist/${APP_NAME}.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_PATH" "$APP/Contents/MacOS/$APP_NAME"
chmod +x "$APP/Contents/MacOS/$APP_NAME"

ICON_PLIST_ENTRY=""
if [[ -f "$ICON_SRC" ]]; then
  cp "$ICON_SRC" "$APP/Contents/Resources/${APP_NAME}.icns"
  ICON_PLIST_ENTRY="  <key>CFBundleIconFile</key><string>${APP_NAME}</string>"
else
  echo "warning: icon not found at $ICON_SRC; bundling without CFBundleIconFile" >&2
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key><string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>${APP_NAME}</string>
${ICON_PLIST_ENTRY}
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <!-- Without it, TCC ends the app at its first microphone access (ARCH-038). -->
  <key>NSMicrophoneUsageDescription</key><string>Skerry sends your microphone to a remote session only while you turn it on or the session asks for it.</string>
  <!-- The same for the camera; without the Continuity key, an iPhone camera is listed as a
       built-in one (ARCH-038). -->
  <key>NSCameraUsageDescription</key><string>Skerry sends your camera to a remote session only while you turn it on or the session asks for it.</string>
  <key>NSCameraUseContinuityCameraDeviceType</key><true/>
  <!-- Finder Open With and Dock drops for any file, never as the default app (ARCH-028). -->
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Any File</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>public.data</string></array>
    </dict>
  </array>
  <!-- skerry://open?url=<percent-encoded> hands a link to the Library's chooser. -->
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>${BUNDLE_ID}.open</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>CFBundleURLSchemes</key><array><string>skerry</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

plutil -lint "$APP/Contents/Info.plist" >/dev/null || { echo "error: Info.plist is invalid" >&2; exit 1; }
plutil -extract NSMicrophoneUsageDescription raw "$APP/Contents/Info.plist" >/dev/null \
  || { echo "error: Info.plist has no microphone usage string" >&2; exit 1; }
plutil -extract NSCameraUsageDescription raw "$APP/Contents/Info.plist" >/dev/null \
  || { echo "error: Info.plist has no camera usage string" >&2; exit 1; }
plutil -extract NSCameraUseContinuityCameraDeviceType raw "$APP/Contents/Info.plist" >/dev/null \
  || { echo "error: Info.plist has no Continuity Camera key" >&2; exit 1; }
# The floor is macOS 13 (LSMinimumSystemVersion): Rust links framework symbols strongly, so a
# reference to one added later stops the whole app from launching there. The camera resolves
# its macOS 14 constants at run time (ARCH-038); keep it that way.
if nm -u "$APP/Contents/MacOS/$APP_NAME" | grep -E '^_AVCaptureDeviceType'; then
  echo "error: the binary links an AVCaptureDeviceType constant strongly; resolve it at run time" >&2
  exit 1
fi

has_notary_creds() {
  [[ -n "${NOTARYTOOL_PROFILE:-}" ]] || {
    [[ -n "${APPLE_ID:-}" ]] && [[ -n "${APPLE_TEAM_ID:-}" ]] && [[ -n "${APPLE_APP_PASSWORD:-}" ]]
  }
}

notarize_artifact() {
  local artifact="$1"
  if [[ -n "${NOTARYTOOL_PROFILE:-}" ]]; then
    xcrun notarytool submit "$artifact" --keychain-profile "$NOTARYTOOL_PROFILE" --wait
  else
    xcrun notarytool submit "$artifact" \
      --apple-id "$APPLE_ID" \
      --team-id "$APPLE_TEAM_ID" \
      --password "$APPLE_APP_PASSWORD" \
      --wait
  fi
}

if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
  echo "signing $APP with Developer ID identity: $CODESIGN_IDENTITY"
  codesign --force --deep --options runtime --timestamp --sign "$CODESIGN_IDENTITY" "$APP"
else
  echo "ad-hoc signing $APP (no CODESIGN_IDENTITY set; not notarized)"
  codesign --force --deep --sign - "$APP"
fi
codesign --verify --deep --strict "$APP"

# Register at once so Open With and skerry: links work without a re-login when built locally.
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
if [[ -x "$LSREGISTER" ]]; then
  "$LSREGISTER" -f "$APP" >/dev/null 2>&1 || echo "warning: lsregister failed; Open With may need a re-login"
fi

echo "built $APP (version $VERSION, arch $ARCH)"

if [[ "$APP_ONLY" -eq 1 ]]; then
  exit 0
fi

ZIP="dist/${APP_NAME}-${VERSION}-${ARCH}.zip"
DMG="dist/${APP_NAME}-${VERSION}-${ARCH}.dmg"
SUMS="dist/SHA256SUMS"

rm -f "$ZIP" "$DMG" "$SUMS"

# Archive .app for Homebrew Cask and direct zip downloads.
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"

# Optional notarization for .app (when Developer ID + notary credentials are configured).
if [[ -n "${CODESIGN_IDENTITY:-}" ]] && has_notary_creds; then
  echo "notarizing $APP via $ZIP"
  notarize_artifact "$ZIP"
  xcrun stapler staple "$APP"
  rm -f "$ZIP"
  ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
fi

# Build compressed read-only DMG with Applications drop link.
STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGE_DIR"' EXIT
ditto "$APP" "$STAGE_DIR/${APP_NAME}.app"
ln -s /Applications "$STAGE_DIR/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE_DIR" -ov -format UDZO "$DMG" >/dev/null

if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
  codesign --force --timestamp --sign "$CODESIGN_IDENTITY" "$DMG"
  if has_notary_creds; then
    echo "notarizing $DMG"
    notarize_artifact "$DMG"
    xcrun stapler staple "$DMG"
  fi
fi

(
  cd dist
  shasum -a 256 "$(basename "$DMG")" "$(basename "$ZIP")" > SHA256SUMS
)

echo "built release artifacts:"
echo "  $APP"
echo "  $DMG"
echo "  $ZIP"
echo "  $SUMS"
if [[ -z "${CODESIGN_IDENTITY:-}" ]]; then
  echo "note: ad-hoc signed build; Gatekeeper users should install via Homebrew (brew tap kebot/skerry https://github.com/kebot/skerry && brew install --cask kebot/skerry/skerry) or run: xattr -cr /Applications/${APP_NAME}.app"
fi
