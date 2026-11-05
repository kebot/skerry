#!/usr/bin/env bash
# Build a release binary and wrap it in dist/rb.app (macOS).
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print([p for p in json.load(sys.stdin)["packages"] if p["name"]=="rb-app"][0]["version"])')"
cargo build --release -p rb-app

APP="dist/rb.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/rb-app "$APP/Contents/MacOS/rb"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>rb</string>
  <key>CFBundleDisplayName</key><string>rb</string>
  <key>CFBundleIdentifier</key><string>dev.kebot.rb</string>
  <key>CFBundleExecutable</key><string>rb</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST

# Ad-hoc sign so macOS will launch it locally (not notarised).
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "warning: ad-hoc codesign failed"
echo "built $APP (version $VERSION)"
