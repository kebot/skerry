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
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` succeed on the workspace.

### Expects Not

- Any companion API request other than `GET /companion/api/ui/version` reaching a SealSkin server older than `0.5.0`, or a string/lexicographic comparison in place of numeric SemVer ordering (which would misorder `0.10.0` before `0.5.0`).

- Separate non-Prose markdown ticket directories (`docs/` or `.agents/prose/specs/skerry-mac/tickets/`) acting as a parallel source of truth outside `.prose.md` contracts and `architecture/decisions.md`.
- Storing session URLs or application keys inside per-server JSON preferences.
