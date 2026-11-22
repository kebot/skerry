#!/usr/bin/env bash
# Build a release binary and wrap it as a macOS app in dist/.
set -euo pipefail
cd "$(dirname "$0")/.."

case "${1:-skerry-mac}" in
  skerry-mac) package=skerry-mac; name='SealSkin Native'; identifier=dev.kebot.sealskin.native ;;
  *) echo "usage: $0 [skerry-mac]" >&2; exit 2 ;;
esac

VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == sys.argv[1]))' "$package")"
cargo build --release -p "$package"

APP="dist/$name.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/release/$package" "$APP/Contents/MacOS/$name"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>${name}</string>
  <key>CFBundleDisplayName</key><string>${name}</string>
  <key>CFBundleIdentifier</key><string>${identifier}</string>
  <key>CFBundleExecutable</key><string>${name}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <!-- Without it, TCC ends the app at its first microphone access (ARCH-038). -->
  <key>NSMicrophoneUsageDescription</key><string>SealSkin Native sends your microphone to a remote session only while you turn it on or the session asks for it.</string>
  <!-- The same for the camera; without the Continuity key, an iPhone camera is listed as a
       built-in one (ARCH-038). -->
  <key>NSCameraUsageDescription</key><string>SealSkin Native sends your camera to a remote session only while you turn it on or the session asks for it.</string>
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
      <key>CFBundleURLName</key><string>${identifier}.open</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>CFBundleURLSchemes</key><array><string>skerry</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

# Ad-hoc sign so macOS will launch it locally (not notarised).
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
if nm -u "$APP/Contents/MacOS/$name" | grep -E '^_AVCaptureDeviceType(External|ContinuityCamera|DeskViewCamera|Microphone)$'; then
  echo "error: the binary links a macOS 14+ AVFoundation symbol strongly" >&2
  exit 1
fi
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "warning: ad-hoc codesign failed"
# Register at once so Open With and skerry: links work without a re-login.
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
"$LSREGISTER" -f "$APP" >/dev/null 2>&1 || echo "warning: lsregister failed; Open With may need a re-login"
echo "built $APP (version $VERSION)"
