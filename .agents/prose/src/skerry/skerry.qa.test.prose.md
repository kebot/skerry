---
name: skerry-qa-feature-set
kind: test
subject: skerry
---

# Skerry Feature-Set QA Plan

End-to-end QA of the current feature set (Connect, Library, Launch Dialog, Session streaming, Session Controls, Stop Session, preview workbench), plus usability/design and performance. Run it after any feature merge; the package suite (`skerry.test.prose.md`) covers the automated checks.

### Fixtures

- `app`: `target/debug/skerry-mac` (or `--release` for performance cases) started with `HOME=<qa-home>`, where `<qa-home>/.config/skerry/config.toml` is a copy of the real one. Every credential and preference change stays inside `<qa-home>`, so the operator's real key and preferences are never touched.
- `preview`: `cargo run -p skerry-ui --example preview` (no network or credentials).
- `server`: the SealSkin ≥ 0.5.0 server named in the key. Sessions that already ran before QA started belong to the operator: QA may open and disconnect them, but never stops them. QA stops only sessions it launched itself.
- `driver`: the `qa` GUI helper (window list, window-only screenshot, window-relative click/scroll/key/type through CGEvent and System Events). Coordinates are in points relative to the window's top-left corner, title bar included; screenshots are at 2x.

### Expects

#### C — Connect and credentials (`ARCH-009`, `ARCH-011`)
- C1 With no saved key, Connect shows the key field and copy naming `~/.config/skerry/config.toml`. Return or the button submits the key.
- C2 An empty key, a key with no `@host`, a non-`ssk_` key, and an `http://` host each show a specific, human-readable error. No network request is made.
- C3 A valid key connects and loads the Library, then saves `application_key` to `config.toml` (`0600` file in a `0700` folder). Relaunching reconnects with no prompt.
- C4 **Disconnect Server** returns to Connect and removes only `application_key`; other lines in `config.toml` survive.
- C5 A malformed `config.toml` shows an error on Connect and is not overwritten until a key is submitted.
- C6 No macOS Keychain prompt appears at any point.

#### L — Library
- L1 Header shows the server host, **Refresh**, and **Disconnect Server**. Refresh reloads sessions and apps without losing the search text.
- L2 Running-session cards show a thumbnail, an app icon, the name, "Started … ago", GPU/storage pills, **Open**, and Stop.
- L3 Application tiles show icons, with a letter badge when an app has no icon. Category chips filter the tiles; **All** resets the filter.
- L4 Search filters apps and sessions case-insensitively. A search with no matches shows a helpful empty state. Pasting an `http(s)` address offers to open it (or clearly says this isn't supported yet).
- L5 Layout reflows at narrow (≈700 pt) and wide (≈1800 pt) window sizes with no clipping, overlap, or horizontal overflow, in both Light and Dark appearance.
- L6 Busy and loading states are visible, and errors (such as the network going away) are shown, not swallowed.

#### D — Launch Dialog
- D1 Clicking an app tile opens the Launch Dialog. Escape, the close button, and a backdrop click all dismiss it.
- D2 The native Select popovers (storage/home and the other options) open, select, and close; their values persist while the dialog stays open.
- D3 **Launch** shows progress, opens the session, and the new session appears under Running sessions after it returns to the Library.
- D4 Favorite, Hide, and Copy controls either work or are clearly disabled; none of them is silently a no-op.
- D5 The fields match SealSkin's web launcher for the same user and app (`ARCH-016`): Storage lists Auto, Cleanroom, and named homes; GPU lists "None" plus the GPUs that fit the app (e.g. `intel-gpu (i915)`); VPN and its endpoint appear only when the server offers them; Language filters as you type and shows Mac names ("German (Germany) · de_DE").
- D6 An Auto launch keeps its data: the session card's storage pill reads `auto-<app>`, and a file saved in the session is there after Stop and a new Auto launch. A GPU launch shows the GPU pill.
- D7 After a launch, opening the same app's dialog again shows the options last used; Copy link pasted into a browser opens the app with those options.

#### S — Session streaming and controls
- S1 **Open** on a session streams video within a few seconds, with a placeholder while waiting. Mouse, scroll, and keyboard reach the remote desktop.
- S2 `cmd-shift-m` toggles the Session Controls window; Escape and `cmd-w` close it. Its sections (Input, Clipboard, Screen, Video, Diagnostics) expand and collapse.
- S3 Switching the codec (JPEG ↔ H.264 ↔ H.265 ↔ AV1, as the server allows) keeps the stream alive. Diagnostics show the active codec, FPS, bitrate, decode time, and RTT, refreshed every second.
- S4 Display modes (**Match Window**, **Match Retina**, **Scale Locally**, fixed presets) change the remote resolution or scaling as labelled. Resizing the window re-negotiates the resolution when Match Window is on.
- S5 `ctrl-cmd-f` toggles fullscreen and back without a stuck black frame.
- S6 Each session opens in its own window. Pointing at its top-left corner shows the close button. Closing it, **Window → Disconnect Session**, or **Disconnect** closes only that window. The remote session keeps running, and the Library refreshes.
- S8 Multi-window (`ARCH-022`): open two sessions, one app launched twice included. Both stream side by side. Controls, codec switch, and resize in one leave the other untouched. **Resume** on an open session brings its window forward. Close the Library: sessions keep streaming, and `cmd-0` or a Dock click brings it back. Copy in window A, focus window B: it pastes once in B and doesn't come back to A.
- S7 Pause, mute, and Reset Video Settings behave as labelled.

#### B — Clipboard (`ARCH-017`)
- B1 Seamless (default): copy text in a Mac app, click into the session window, paste with Control+V (or ⌘V with **Command sends Control**). The text arrives, Unicode included (e.g. "日本語 — café"). Copy in the session, switch to a Mac app, ⌘V: it arrives.
- B2 Rich text and images: copied formatted text from Safari pastes formatted into a remote rich editor and as plain text into a remote terminal. A Mac screenshot copied to the clipboard (⌃⇧⌘4) pastes as a picture in the session. A picture copied in the session pastes into Preview on the Mac.
- B3 No echo: after a remote copy lands on the Mac, refocusing the session window sends nothing back (`clipboard.receive` is not followed by a `clipboard.send` of the same content in the server's debug log).
- B4 Session Controls → **Clipboard**: turning off **Send to Session** or **Receive from Session** stops that direction alone. Turning off **Seamless** stops both, and **Send Mac Clipboard** / **Copy Session Clipboard** then move content on demand. The switches persist for that server across reconnects.
- B5 Large text (≥ 1 MB) crosses in both directions intact.
- B6 A password copied by a password manager in the session lands on the Mac marked concealed (a clipboard manager such as Maccy or Raycast does not record it), and it is gone from the Mac clipboard after 60 s if nothing replaced it.

#### K — Stop Session
- K1 Stop on a QA-launched session asks for confirmation that explains data loss. Cancel keeps the session.
- K2 Confirming stops the session, and it leaves Running sessions after the Library refreshes.

#### P — Preview workbench
- P1 Every fixture (Connect default/busy/error, Library populated/empty/busy, Launch Dialog full and minimal, Session Controls H.265/JPEG/server-locked, Session Overlays) renders in Light, Dark, and System themes, with event feedback when controls are clicked.

#### U — Usability and design
- U1 Consistent spacing, type scale, alignment, and button hierarchy; one clear primary action per screen; hover and focus states on everything clickable; icon-only buttons are recognisable (the Stop square in particular).
- U2 Copy is specific and actionable, and errors say what to do next.
- U3 Keyboard reach: Tab order, Return to submit, Escape to dismiss.

#### F — Performance (release build)
- F1 Cold start to a populated Library in under 3 s on a warm network. The Library sits idle at under 2 % CPU with no busy redraw loop.
- F2 Library memory stays flat across 10 Refreshes (icons and thumbnails are not re-fetched or leaked without bound).
- F3 Streaming at 60 fps: client FPS near the server rate, decode under 5 ms with hardware codecs, main-thread CPU reasonable. Resident memory grows less than 50 MB across 2 minutes of streaming (no texture leak, `window.drop_image`).
- F4 Typing in search stays responsive with no visible lag.

### Expects Not

- Any panic, hang, or crash, or a dialog that blocks startup (Keychain or otherwise).
- QA stopping, or changing the settings of, a session it did not launch.
- QA modifying the operator's real `~/.config/skerry/config.toml` or real preferences.
