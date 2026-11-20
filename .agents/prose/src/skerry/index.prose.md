---
name: skerry
kind: responsibility
id: 067NC4KG01SKERRY0000000001
---

# Skerry Product & Architecture World-Model

### Goal

Own and maintain Skerry's product roadmap, feature specifications, architecture notes, and implementation readiness inside `.agents/prose/specs/` in lockstep with the Rust/GPUI codebase (`crates/skerry-mac/`).

### Requires

- `rust-workspace`: Rust source and manifest in `crates/skerry-mac/**` and `Cargo.toml`
- `product-specs`: Migrated architecture, roadmap, GPUI notes, and ticket specifications in `.agents/prose/specs/**`

### Maintains

- `design`: `.agents/prose/specs/skerry-mac/DESIGN.md` and `.agents/prose/specs/gpui-notes.md` — current module boundaries, Selkies/SealSkin protocol wiring, and GPUI implementation invariants
- `roadmap`: `.agents/prose/specs/skerry-mac/TODO.md` and `.agents/prose/specs/skerry-mac/tickets/*.md` — milestone status, completed features, and implementation-ready ticket specs
- `qa-evidence`: `.agents/prose/specs/skerry-mac/milestone-1-qa.md` and `.agents/prose/specs/skerry-mac/running.md` — operational and QA truth

Facets:
- `architecture` — module structure and protocol boundaries in `DESIGN.md` & `gpui-notes.md`
- `backlog` — roadmap state in `TODO.md` and feature tickets in `tickets/*.md`
- `verification` — `cargo test`, `cargo clippy`, and QA readiness

Material: structural changes to `crates/skerry-mac/src/**`, ticket completion transitions, new feature requirements, and protocol/codec capability changes.
Immaterial: check timestamps, build timings, and formatting-only edits.

Postconditions:
- Every ticket in `.agents/prose/specs/skerry-mac/tickets/*.md` is reflected in `.agents/prose/specs/skerry-mac/TODO.md`.
- `DESIGN.md` accurately describes the modules in `crates/skerry-mac/src/`.
- Any code change gated by this package passes `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check`.

### Continuity

input-driven

### Strategies

- When asked to plan, refine, or check status of a feature: inspect and update `.agents/prose/specs/skerry-mac/TODO.md` and `.agents/prose/specs/skerry-mac/tickets/<feature>.md`.
- When asked to implement a feature ticket: call `implement-feature` with the ticket slug, verify against `DESIGN.md` and `gpui-notes.md`, run Cargo verification gates, and update `TODO.md` and `DESIGN.md`.
