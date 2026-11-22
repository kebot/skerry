# ARCH-022: Multi-Window Session Behaviour (`ARCH-GAP-LS-08`)

- **Amended by `ARCH-039`:** a session may now have a second connection, its Display 2 window. "Never a second connection" still holds for resuming an open session.

- **Status:** settled (decided and verified live 2026-11-22). Settles `ARCH-GAP-LS-08` and builds on `ARCH-010`.
- **Decision:**
  - **Window ownership:** a `SessionWindow` entity (`app/session.rs`) owns one session's stream, settings, stats, display mode, resize, policies, and Session Controls. The `Launcher` owns only the Library and a list of open session windows `(AnyWindowHandle, WeakEntity<SessionWindow>, session_id)`. Session Controls' `Owner` pairs the session window's handle with `Entity<SessionWindow>` (`ARCH-006`). Session windows open through `cx.defer` outside `Launcher` updates.
  - **Resume of an already open session:** brings its existing window to the front. Skerry never opens a second connection to a session it is already showing.
  - **Closing the Library:** the Library window closes and session windows keep streaming. **Window → Library** (`cmd-0`) or a Dock-icon click (app reopen) opens it again. Skerry quits only on `cmd-q` or when the last window closes. `cmd-q` disconnects every session and stops none.
  - **Preferences across windows:** codec, input, clipboard, and video changes are saved per server for the next session that opens. Other open windows keep what they have. Display mode, resolution, mute, and pause are per window, and each session plays its own audio.
  - **Closing a session window:** the title bar and window buttons stay hidden over the picture (`ARCH-014`). Moving the pointer into the window's top-left corner reveals the close / minimize / zoom buttons, and **Window → Disconnect Session** closes the key session window. `cmd-w` stays with the remote app.
  - **Teardown:** closing a session window (or the status banner's **Disconnect**) ends only that session's stream, releases its held keys, closes its Session Controls, and refreshes the Library. It never calls the stop API.
- **Affected Contracts:** `src/skerry/launcher-and-sessions.prose.md`, `src/skerry/index.prose.md`, `src/skerry/skerry.test.prose.md`, `src/skerry/skerry.qa.test.prose.md`
