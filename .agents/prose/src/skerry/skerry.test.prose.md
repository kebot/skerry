---
name: skerry-package-test
kind: test
subject: skerry
---

# Skerry Package Verification Suite

### Fixtures

- `workspace`: `crates/skerry-mac` Rust workspace and `.agents/prose/src/skerry/*.prose.md` contracts

### Expects

- The root responsibility `skerry` and domain responsibilities (`video-and-screen`, `launcher-and-sessions`, `desktop-and-collaboration`) have explicit `Requires`, `Maintains`, `Postconditions`, and `Continuity`.
- Every unsettled product feature from the former backlog is preserved as an explicit `<!-- compose:unresolved id=ARCH-GAP-... -->` marker and constraint entry in its owning domain responsibility.
- Every settled Milestone 1 and hardware codec feature is recorded as a settled `Maintains` clause with verification postconditions.
- `sealskin::tests::sealskin_0_5_0_or_newer_is_supported` accepts `0.5.0`, `0.5.1`, `0.6.0`, `0.10.0`, `1.0.0`, `0.5.1-rc1`, and `0.5.0+build7`, and refuses `0.4.2`, `0.4.99`, `0.5.0-rc1`, `v0.5.0`, `0.5`, `0.5.0.1`, and empty or non-numeric versions; `Api::library` calls `Api::check_version` before any other companion API request (`ARCH-009`).
- `sealskin::tests::saved_key_round_trips_through_a_private_config_toml`, `saving_the_key_keeps_other_settings_and_comments`, and `malformed_saved_keys_are_reported_not_ignored` pass: the key is saved as `application_key` in `~/.config/skerry/config.toml` (`0600` file, `0700` folder), other settings and comments survive save and forget, and malformed values are reported (`ARCH-011`).
- `video::tests::predicted_frames_follow_chained_or_unchained_references` passes: H.264 P-frames need `reference == last`, and H.265/AV1 P-frames with `reference == id` need only a previous decoded frame (`ARCH-012`).
- `transport::tests::only_startup_statuses_are_retried` passes: only 404/502/503/504 (and I/O errors) are retried while a session starts (`ARCH-014`).
- `launch_options::tests::*` pass: fields offer and hide what the web launcher does (storage, GPU by app support, VPN and endpoints), defaults then remembered options where they still hold, Auto names and creates `auto-<app>`, GPU/VPN are sent only when chosen, Copy link names only non-default options, and the default language is the locale nearest the Mac's (`ARCH-016`).
- `sealskin::tests::decodes_the_status_launch_rights_gpus_and_vpns`, `launch_body_carries_only_the_chosen_options`, and `deep_link_carries_the_launch_options` pass (`ARCH-016`).
- `clipboard::tests::*` pass: text, Unicode, and LF line ends go as one `cw` frame; payloads of 16383 bytes or more go multipart (`cws`/`cwd`/`cwe`, `cbs`/`cbd`/`cbe`) with their declared size; rich text rides the `application/x-selkies-clipboard-flavours` envelope; `Receiver` reassembles `clipboard_start`/`data`/`finish`, carries `clipboard_secret` and `clipboard_reply,cr`, drops truncated or superseded payloads; and server policy bounds the person's switches, with a locked Seamless winning (`ARCH-017`).
- `pasteboard::tests::contents_round_trip_and_secrets_are_concealed` passes on a private `NSPasteboard`: text, HTML with its text, and PNG round-trip, every write bumps the change count, and a secret carries `org.nspasteboard.ConcealedType` (`ARCH-017`).
- Session Controls follows the Selkies web sidebar (`ARCH-018`): sections are separate cards 10px apart; Encoder, Rate Control, UI Scaling, and Preset are dropdowns; numeric video settings are sliders that request a value only on release; on/off settings are switches.
- `stats::tests::*` pass: rows warn only on a hardware fallback, a readback where zero-copy was available, or throttling; tiles hold a time for 5 s then read idle; Video rate warns over 110% of its CBR target; meters appear only when the server reports them; a history longer than 120 points is bucketed by maximum (`ARCH-019`).
- The Stats section subscribes with `_stats,1` only while it is open, and re-subscribes after a reconnect (`ARCH-019`).
- A 60 fps H.264/H.265/AV1 stream reads about 60 Client FPS in Stats, not 30 (`ARCH-021`).
- Two sessions (the same app twice included) stream in separate windows at once. Closing one leaves the other streaming and the Library open. Session Controls and resizing act only on their own window. Resuming an open session focuses its window (`ARCH-022`).
- `selkies::tests::*` pass: a file at or under 64 MiB uploads as one POST with only `X-Upload-Path` (URL-encoded); a larger file goes as sequential 64 MiB slices whose `X-Upload-Id` is constant, `X-Upload-Offset` advances by the bytes sent, `X-Upload-Total` is the file size, and only the last carries `X-Upload-Final: 1`; browse JSON decodes into entries; a resumed download asks for `Range: bytes=<have>-`; no request carries an `Origin` header; upload/download availability follows `file_transfers`; `selkies::tests::end_to_end::*` round-trip a plain upload, a ~131 MiB sliced upload, browse, mkdir, a download, and a resumed download (206, and 416 restarting from 0) against an in-process fake Selkies; and the list's keys never fire while typing in the New Folder field (`ARCH-023`).
- `files_window::tests::deletes_map_into_the_home_desktop_only_when_the_folders_match` passes: a Selkies path maps to `/Desktop/<path>` in the home, and Delete is offered only when the Selkies root and the home's `Desktop` list the same names, staging files aside (`ARCH-024`). `pasteboard::tests::file_urls_round_trip` resolves Finder's `file:///.file/id=…` reference URLs to real paths.
- Task A tests pass (`ARCH-026`):
  - `KeyLedger` tests: releases on blur and close, never Super or Meta for Command, and a Shift-changed keysym is released.
  - A stream dropped while connecting sends no `SETTINGS`, and key releases go out before the close frame.
  - The raw-TCP tests: logo and icon fetches carry no `Authorization`, and only `/companion/api/` requests do.
  - The write queue is the only writer (a test fails if another module calls the file writers), runs jobs in order, and backs up an unreadable preferences file.
  - Unknown preference keys survive a rewrite.
- Task E tests pass (`ARCH-030`):
  - `audio::tests::*` use the fake `Backend`: following the default, chosen-missing fallback and return, the stall heartbeat (no false stall while muted), backoff and strikes, the probe keeping the fallback playing, and one failed listing keeping a healthy stream.
  - The `AudioBitrate` parsing accepts strings or numbers, and the `SETTINGS` audio_bitrate message is the exact text.
  - A preferences file without audio settings loads.
- Task B tests pass (`ARCH-027`):
  - `launch_progress::tests::*`: stages only move forward, a POST success wins, a same-id Retry is offered only after a lost request, a 401 waits for the outstanding POST, and a gateway 5xx counts as lost.
  - The `library` preferences load tolerantly (a bad entry costs only that entry, lists are capped), and the reuse match is app + storage + Collaborative, oldest first.
  - A Refresh from before a launch keeps that launch's session.
- Task D tests pass (`ARCH-029`):
  - Refusal classification covers the exact 401 codes, `Invalid token.`, non-JSON bodies, and the pinned 0.5.0 403 strings, and `held: true` wins.
  - After Revoked or Expired, an empty field never resends.
  - A pasted key that connects later is saved, and env or `.env` keys never are.
  - Wake and web URLs carry no key.
  - The wake poll sends nothing after it is cancelled and puts `Authorization` only on the control-plane host.
  - `WakeState` never blocks the next poll.
- Task C tests pass (`ARCH-028`):
  - `open_target::tests::*`: URL acceptance follows the server pattern (schemes, magnet spaces, 8192 characters, userinfo dropped, uppercase schemes), and Return acts only on typed links.
  - The upload streams 1 MiB chunks and stops if the file changes.
  - Error mapping: 403 storage full, 413, 429.
  - ⌘O/⌘L don't resolve under the `SessionDesktop` or `FilesWindow` key contexts.
  - No stall notice shows while uploading.
- Task F tests pass (`ARCH-031`):
  - Every chord that acts in a session is listed, forwarding matches suppression, and suppression comes last. All three run against the shipped `key_bindings()`.
  - A forwarded chord taps its key while Control stays latched, and a letter released under ⌘ is released when ⌘ lifts.
  - `StatsInterest` subscribes while Stats or the strip wants it.
- Task H tests pass (`ARCH-033`):
  - `print_jobs::tests::*`: one GET with no Range, the 1 GiB cap checked three times, the `%PDF-` check, and a cut-off stream marked Broken.
  - Failures before a 200 and SealSkin's 403 read as reconnect.
  - A re-announce replaces a failed job, and the connect burst auto-opens only the newest fresh job.
  - A closed window fetches nothing, and cleanup keeps any file under a sheet.
- Task G tests pass (`ARCH-032`):
  - `m2` carry and scaling, a held chord toggling once, a stale motion not using up the first-motion drop, and modifiers lifted before muting.
  - The `js,c/d/b/a/h` encodings, the W3C mapping, slots per session and slot number, and rumble capped at 5 s.
  - Closing while captured lifts buttons and sends `p,0`.
  - A pending print sheet skips a pending capture.
- Task I tests pass (`ARCH-034`):
  - Stripe edges snap to device pixels at scales 2.0, 1.0, 0.77 and 1.5.
  - Retired stripes are dropped after 4 paints, and every exit retires every stripe.
  - The bottom stripe presents at once, the settle presents within 2 ms, and the pacer resets.
  - A JPEG cut inside its headers is rejected, and grayscale decodes.
- Task J tests pass (`ARCH-035`):
  - `apps::tests::*`:
    - the gate sends nothing while disconnected or not allowed
    - support detection hides Apps after 5 s and a late reply restores it
    - the YAML budget and the local-repo catalog
  - SVG refusals (entity, marker, `<use>`, nested paint servers through classes, styles and ancestors, `url()` whitespace variants, `@import`) all complete in under 50 ms. Filters are stripped.
  - A pattern bomb sent to the worker is killed within 3.5 s and the next icon still decodes.
  - All 137 fixture icons decode.
  - The GitHub client sends no credentials and refuses http.
- Task K tests pass (`ARCH-036`):
  - Link, token and URL parsing: `+` becomes a space, userinfo and port 0 are refused, the loopback rules hold, and the token is dropped on a host change.
  - The pin covers the leaf only, with trusted roots loaded. A mismatch is fatal on HTTP and on the session socket.
  - The secure-mode probe, the 401 classification, and Close 4001 being fatal.
  - The `[[selkies]]` rules: at most 5, `0600`, the pin kept with Remember off.
  - `cargo tree -i aws-lc-rs` is empty.
- Task M1 tests pass (`ARCH-038`):
  - The real AudioToolbox Opus encoder makes 20 ms packets that decode with `opus-decoder`.
  - Error, StartupTimeout, Reconnect, close, quit, and closing the sink all stop capture.
  - Late permission answers are ignored, and the start policy runs only once.
  - Reconnects don't stack threads while permission is undetermined.
  - The 10 s no-traffic timeout fires.
- Task L1 tests pass (`ARCH-037`):
  - Shared Files and Cleanroom never get a DELETE (case-insensitive, zero requests sent).
  - Delete Home rechecks sessions after the confirm, and the storage pane is not in the Files registry.
  - A body cut off mid-download resumes, but a partial whose tag doesn't match restarts from 0, and a truncated folder walk fails.
  - Upload retries carry the upload_id key, and Open in… filters apps.
- Task N tests pass (`ARCH-039`):
  - `Wire` stamps and overwrites `displayId` on every Display 2 frame. The fake-Selkies harness checks both roles, and that mic frames go only on the primary.
  - Placement handles negative display origins.
  - Late events from a dropped stream are ignored, and a primary error puts Display 2 in Waiting.
  - When its owner closes Display 2, keys are released.
  - A direct-server target works.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` succeed on the workspace.

### Expects Not

- Any companion API request other than `GET /companion/api/ui/version` reaching a SealSkin server older than `0.5.0`, or a string/lexicographic comparison in place of numeric SemVer ordering (which would misorder `0.10.0` before `0.5.0`).

- Separate non-Prose markdown ticket directories (`docs/` or `.agents/prose/specs/skerry-mac/tickets/`) acting as a parallel source of truth outside `.prose.md` contracts and `architecture/decisions.md`.
- Storing session URLs or application keys inside per-server JSON preferences.
- A test that reads or writes the person's general pasteboard (`ARCH-017`), or clipboard contents in any log line.
- Any `security-framework` / macOS Keychain read or write in `crates/skerry-mac` (`ARCH-011`).
- A whole file read into memory to upload it, or a Files window action offered that `file_transfers` refuses (`ARCH-023`).
