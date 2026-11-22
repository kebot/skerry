# ARCH-040: Skerry App Identity & Free macOS Release Pipeline

- **Status:** settled
- **Decision:**
  - **App identity:** Unify macOS bundle and runtime identity under **Skerry**: `Skerry.app`, `CFBundleIdentifier = dev.kebot.skerry`, app icon `assets/icons/Skerry.icns` (generated from `assets/icons/Skerry.svg` via `scripts/build-icon.sh`), and preferences directory `~/Library/Application Support/Skerry/` (with read fallback to legacy `~/Library/Application Support/SealSkin Native/`).
  - **Free ad-hoc release packaging:** `scripts/bundle.sh` produces an ad-hoc signed `dist/Skerry.app`, `dist/Skerry-<version>-<arch>.dmg` (compressed UDZO with `/Applications` symlink), `dist/Skerry-<version>-<arch>.zip` (`ditto` archive for Homebrew Cask), and `dist/SHA256SUMS`, preserving `Info.plist` TCC validation and macOS 13 symbol checks (`ARCH-038`). Optional environment-variable hooks (`CODESIGN_IDENTITY`, plus `NOTARYTOOL_PROFILE` or `APPLE_ID`/`APPLE_TEAM_ID`/`APPLE_APP_PASSWORD`) enable Developer ID signing and Apple notarization later without script rewrites.
  - **Distribution & CI:** `.github/workflows/release-macos.yml` (compatible with both GitHub Actions and Forgejo Actions) builds and publishes macOS `arm64` release artifacts on `v*` tags. `Casks/skerry.rb` (in-repo Homebrew tap) and `README.md` document both Homebrew (`brew tap kebot/skerry https://github.com/kebot/skerry && brew install --cask kebot/skerry/skerry`) and direct DMG (`xattr -cr /Applications/Skerry.app`) installation.
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/skerry.test.prose.md`
- **Affected Files:** `scripts/bundle.sh`, `scripts/build-icon.sh`, `assets/icons/Skerry.svg`, `assets/icons/Skerry.icns`, `Casks/skerry.rb`, `.github/workflows/release-macos.yml`, `README.md`, `crates/skerry-mac/src/{app,main,persist,settings,transport}.rs`

---
