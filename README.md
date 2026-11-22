# Skerry

Skerry is a native macOS app for [SealSkin](https://docs.sealskin.app/latest/) and [Selkies](https://github.com/selkies-project/selkies). Connect to your SealSkin server—or directly to a Selkies stream—to launch, resume, and use remote desktop apps in native Mac windows without a browser.

## Features

- **Hardware-accelerated video:** H.264, H.265 (HEVC), and AV1 decoded natively with Apple VideoToolbox, plus JPEG fallback.
- **Native Mac feel:** Retina/HiDPI scaling, smooth trackpad scrolling, pinch-to-zoom, Mac input method (IME) support, and Command-to-Control mapping.
- **Audio, mic, and camera:** Stereo Opus audio with selectable output devices, plus optional microphone and webcam forwarding.
- **Files & clipboard:** Two-way clipboard sync and a built-in **Files** window with upload, download, Quick Look, and drag-and-drop.
- **Multi-window & second display:** Run multiple remote sessions side by side, or extend a session onto a second display.
- **Gamepads, pointer capture, and printing:** Controller support, pointer lock (`⌃⌥⇧Z`) for 3D apps and games, and remote printing.

## Requirements

- **Mac:** Apple Silicon (M1 or newer) running **macOS 13 Ventura** or later.
- **Server:** SealSkin **0.5.0+** (over trusted HTTPS with the **Other** client type enabled), or a direct Selkies server.

---

## Install

> **Note on Gatekeeper:** Skerry releases are ad-hoc signed rather than notarized with a paid Apple Developer account. macOS will block the first launch unless you clear the quarantine attribute as shown below.

### Option 1: Download the DMG (recommended)

1. Download `Skerry-<version>-arm64.dmg` from the [Releases](https://github.com/kebot/skerry/releases) page.
2. Open the `.dmg` and drag **Skerry.app** into your **Applications** folder.
3. Open **Terminal** and run this command once before launching Skerry:
   ```sh
   xattr -cr /Applications/Skerry.app
   ```
4. Launch **Skerry** from Applications or Spotlight.

*(Optional)* To verify the download before installing, download `SHA256SUMS` into the same folder and run:
```sh
shasum -a 256 -c SHA256SUMS --ignore-missing
```

### Option 2: Install with Homebrew

```sh
brew tap kebot/skerry https://github.com/kebot/skerry
brew install --cask kebot/skerry/skerry
xattr -cr /Applications/Skerry.app
```

---

## Getting Started

When you open Skerry, choose either **SealSkin** or **Selkies** at the top of the Connect screen.

### Connect to a SealSkin server

1. Sign in to your SealSkin web dashboard.
2. Go to **Settings → Application Keys** and create a new key.
3. Copy the entire key (including `@your-server-address`).
4. Paste it into the **SealSkin** tab in Skerry and click **Connect**.

Skerry saves your key locally in `~/.config/skerry/config.toml` (readable only by your user) so you stay signed in next time.

### Connect directly to a Selkies server

1. Switch to the **Selkies** tab.
2. Enter your Selkies server URL (and credentials/token if your server requires them) and click **Connect**.

---

## Using Skerry

### Library & Sessions

- **Launch or resume:** Click an app in the Library to start a new session, or click a running session to reconnect to it. You can open multiple sessions in separate windows at the same time.
- **Launch options:** Configure per-app options such as named home storage, GPU, VPN, language, timezone, or collaborative mode before launching.
- **Open local files or links:** From the Library, use **File → Open… (`⌘O`)** or **File → Open Location… (`⌘L`)** to open a file or URL in a remote app.
- **Manage home storage:** Use **Window → Storage (`⌘⇧I`)** to view and manage your SealSkin home directories and shared files.
- **Disconnecting vs. Stopping:** Closing a session window (or choosing **Window → Disconnect Session**) disconnects your Mac while leaving the remote session running so you can resume later. To shut a session down completely, stop it from the Library.

### Session Controls (`⌘⇧M`)

Press `⌘⇧M` (or choose **View → Session Controls**) while in a session to open the floating controls panel. From there you can adjust:

- **Input:** Toggle Command-as-Control, remote cursor, smooth trackpad scrolling, Mac input method (IME), or send remote chords like `Alt+Tab` and `Ctrl+Alt+Delete`.
- **Shortcuts:** Switch between keeping Mac shortcuts active locally or passing them through to the remote session.
- **Gamepads:** View and manage connected controllers.
- **Clipboard:** Control clipboard sync between your Mac and the remote desktop.
- **Printing:** Configure remote printing when enabled by the server.
- **Screen:** Adjust scaling/HiDPI mode or open a **Second Display** window.
- **Video:** Switch codec (H.264, H.265, AV1, JPEG), quality, and framerate.
- **Audio:** Pick your Mac audio output device and control volume/mute.
- **Microphone & Camera:** Toggle microphone and webcam forwarding (macOS will ask for permission the first time you enable them).
- **Stats:** View live codec, framerate, bitrate, and latency stats, or toggle the overlay strip over the stream.

### Files (`⌘⇧F`)

Press `⌘⇧F` (or choose **View → Files**) in a session to open the session's **Files** window:

- Browse folders, create new folders (`⌘⇧N`), upload files (`⌘⇧U`), or download selected files.
- Preview remote files with **Quick Look** (`Space`), copy/paste (`⌘C` / `⌘V`), or drag files directly between Finder and the Files window.

---

## Keyboard Shortcuts

### In a Session Window

| Shortcut | Action |
|---|---|
| `⌘⇧M` | Open / close Session Controls |
| `⌃⌘F` | Toggle Full Screen |
| `⌘⇧F` | Open / close session Files window |
| `⌘0` | Show Library window |
| `⌃⌥⇧Z` | Toggle Capture Pointer (always stays local so you can always exit pointer lock) |
| `⌘Q` | Quit Skerry (always stays local) |

### In the Library Window

| Shortcut | Action |
|---|---|
| `⌘O` | Open a local file in a remote app |
| `⌘L` | Open a URL in a remote app |
| `⌘⇧I` | Open Storage |

---

## Troubleshooting

**"Skerry.app is damaged and can't be opened" or Gatekeeper blocks launch**
Because releases are ad-hoc signed, macOS quarantines downloaded apps. Run:
```sh
xattr -cr /Applications/Skerry.app
```

**Can't connect to SealSkin**
Make sure your server is running SealSkin **0.5.0 or newer**, uses a trusted HTTPS certificate, and has the **Other** client type enabled.

**Where does Skerry store my data?**
- Application keys and direct Selkies connections: `~/.config/skerry/config.toml`
- Per-server preferences: `~/Library/Application Support/Skerry/`

---

## Building from Source & Packaging

For development, Skerry also reads `SEALSKIN_APPLICATION_KEY` from the environment or a `.env` file in the working directory (used for that run only and not saved to `~/.config/skerry/config.toml`). GPUI is built with `runtime_shaders`, so the Xcode Metal toolchain is not required.

```sh
# Run locally
cargo run -p skerry-mac

# Run tests, lint, and format checks
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check

# Build ad-hoc signed dist/Skerry.app, .dmg, .zip, and SHA256SUMS
scripts/bundle.sh
```

`scripts/bundle.sh` supports `--app-only` and `--target <triple>`, as well as optional signing/notarization environment variables (`CODESIGN_IDENTITY`, plus `NOTARYTOOL_PROFILE` or `APPLE_ID`/`APPLE_TEAM_ID`/`APPLE_APP_PASSWORD`). Pushing a `v*` tag matching `Cargo.toml` triggers `.github/workflows/release-macos.yml` to verify, package, and publish the macOS `arm64` release.
