# Skerry

Skerry is a native Mac client for [SealSkin](https://docs.sealskin.app/latest/) and [Selkies](https://github.com/selkies-project/selkies). It lists the applications and running sessions on a SealSkin server, launches or resumes one, and streams it into a Mac window over the Selkies WebSocket transport. It is written in Rust with [GPUI](https://gpui.rs/).

- **Video:** H.264, H.265 (HEVC) and AV1 decoded in hardware with VideoToolbox, or JPEG decoded in Rust.
- **Audio:** stereo Opus played through Core Audio.
- **Input:** keyboard, mouse, smooth trackpad scrolling, pinch to zoom, and optional Mac input method composition.
- **Screen:** the remote desktop follows the window size, Retina scaling and the Mac display's UI scale.
- **Session Controls:** a separate panel (⌘⇧M) for playback, screen, video codec and quality settings, and stream diagnostics.

Disconnecting returns to the launcher and leaves the remote session running.

## Install (macOS Apple Silicon, macOS 13+)

Prebuilt releases are **ad-hoc signed** (not notarized with a paid Apple Developer account), so macOS Gatekeeper will quarantine downloads unless bypassed as shown below.

### Option 1: Homebrew Cask

Install from the cask definition with `--no-quarantine`:

```sh
brew install --cask --no-quarantine ./packaging/homebrew/Casks/skerry.rb
```

Or, if you have configured a Homebrew tap pointing at this repository's `packaging/homebrew` directory:

```sh
brew install --cask --no-quarantine skerry
```

### Option 2: Direct DMG download

1. Download `Skerry-<version>-arm64.dmg` (and optionally `SHA256SUMS`) from the [Releases](https://github.com/kebot/skerry/releases) page.
2. (Optional) Verify the checksum:
   ```sh
   shasum -a 256 -c SHA256SUMS --ignore-missing
   ```
3. Open the `.dmg` and drag **Skerry.app** into `/Applications`.
4. Remove the macOS Gatekeeper quarantine attribute before first launch:
   ```sh
   xattr -cr /Applications/Skerry.app
   ```

## Quick start

1. Sign in to your SealSkin server's web app, open **Settings → Application Keys**, and create a key. Copy the whole value, including the server address after `@`.
2. Open **Skerry** and enter your application key. Skerry saves the key in `~/.config/skerry/config.toml` (`0600`) after it connects.

The server must run SealSkin **0.5.0** or newer, use a trusted HTTPS certificate, and allow the **Other** client type.

For development, Skerry also reads `SEALSKIN_APPLICATION_KEY` from the environment or from `.env` in the working directory. A key supplied this way is used for that run only and is not saved to `~/.config/skerry/config.toml`.

```sh
cargo run -p skerry-mac
scripts/bundle.sh   # produces dist/Skerry.app, .dmg, .zip, and SHA256SUMS (ad-hoc signed)
```

GPUI is built with the `runtime_shaders` feature, so you don't need the Xcode Metal toolchain.

## Release & packaging

`scripts/bundle.sh` builds a release binary and produces:

- `dist/Skerry.app` — macOS application bundle (`dev.kebot.skerry`, wired with `assets/icons/Skerry.icns`)
- `dist/Skerry-<version>-<arch>.dmg` — compressed UDZO disk image with an `/Applications` drop link
- `dist/Skerry-<version>-<arch>.zip` — `ditto` archive used by the Homebrew Cask
- `dist/SHA256SUMS` — SHA-256 checksums for the `.dmg` and `.zip`

Pass `--app-only` to build only `dist/Skerry.app`, or `--target <triple>` to target a specific Rust triple.

### Optional Developer ID signing & notarization

By default, `scripts/bundle.sh` performs free ad-hoc signing (`codesign --force --deep --sign -`). If you later obtain an Apple Developer ID certificate, set these environment variables (locally or as CI secrets) to sign and notarize automatically:

- `CODESIGN_IDENTITY` — e.g. `Developer ID Application: Your Name (TEAMID)`
- `NOTARYTOOL_PROFILE` — keychain profile created with `xcrun notarytool store-credentials` (or set `APPLE_ID`, `APPLE_TEAM_ID`, and `APPLE_APP_PASSWORD`)

Pushing a `v*` tag (matching `Cargo.toml`'s version) triggers `.github/workflows/release-macos.yml` (compatible with both GitHub Actions and Forgejo Actions) to verify the workspace, package the macOS `arm64` artifacts, and publish the release.

## Layout

| Path | Contents |
|------|----------|
| `crates/skerry-mac` | The Mac app: SealSkin API client, Selkies transport, video decoding, GPUI views, Core Audio playback |
| `crates/skerry-ui` | Pure `gpui-kit` UI library and standalone interactive preview workbench |
| `assets/icons/` | Vector app icon source (`Skerry.svg`) and compiled macOS icon (`Skerry.icns`) |
| `packaging/homebrew/` | Homebrew Cask definition (`Casks/skerry.rb`) |
| `.github/workflows/` | Tag-triggered macOS release workflow (`release-macos.yml`, GitHub/Forgejo Actions compatible) |
| `scripts/bundle.sh` | Builds `Skerry.app`, `.dmg`, `.zip`, and `SHA256SUMS` in `dist/` |
| `scripts/build-icon.sh` | Regenerates `assets/icons/Skerry.icns` from `assets/icons/Skerry.svg` |
| `.agents/prose/` | OpenProse responsibility contracts (`src/skerry/`) and architecture decision lattice (`architecture/`) |

## Develop

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
