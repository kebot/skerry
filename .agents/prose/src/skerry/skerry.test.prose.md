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
- `api::tests::sealskin_0_5_0_or_newer_is_supported` accepts `0.5.0`, `0.5.1`, `0.6.0`, `0.10.0`, `1.0.0`, `0.5.1-rc1`, and `0.5.0+build7`, and refuses `0.4.2`, `0.4.99`, `0.5.0-rc1`, `v0.5.0`, `0.5`, `0.5.0.1`, and empty or non-numeric versions; `Api::library` calls `Api::check_version` before any other companion API request (`ARCH-009`).
- `api::tests::saved_key_round_trips_through_a_private_config_toml`, `saving_the_key_keeps_other_settings_and_comments`, and `malformed_saved_keys_are_reported_not_ignored` pass: the key is saved as `application_key` in `~/.config/skerry/config.toml` (`0600` file, `0700` folder), other settings and comments survive save and forget, and malformed values are reported (`ARCH-011`).
- `video::tests::predicted_frames_follow_chained_or_unchained_references` passes: H.264 P-frames need `reference == last`, and H.265/AV1 P-frames with `reference == id` need only a previous decoded frame (`ARCH-012`).
- `transport::tests::only_startup_statuses_are_retried` passes: only 404/502/503/504 (and I/O errors) are retried while a session starts (`ARCH-014`).
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` succeed on the workspace.

### Expects Not

- Any companion API request other than `GET /companion/api/ui/version` reaching a SealSkin server older than `0.5.0`, or a string/lexicographic comparison in place of numeric SemVer ordering (which would misorder `0.10.0` before `0.5.0`).

- Separate non-Prose markdown ticket directories (`docs/` or `.agents/prose/specs/skerry-mac/tickets/`) acting as a parallel source of truth outside `.prose.md` contracts and `architecture/decisions.md`.
- Storing session URLs or application keys inside per-server JSON preferences.
- Any `security-framework` / macOS Keychain read or write in `crates/skerry-mac` (`ARCH-011`).
