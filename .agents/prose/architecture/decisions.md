# Architecture Decisions

## ARCH-001: Attached OpenProse Root & Harness Selection

- **Status:** settled
- **Mode:** bootstrap (`prose init`)
- **Decision:** Establish OpenProse as an attached repository root at `.agents/prose/` using the default `filesystem` state backend and in-session Prose Complete agent harness.
- **Rationale:** Skerry is an existing Rust/GPUI workspace (`Cargo.toml`, `crates/skerry-mac`). Using `.agents/prose/` keeps the Cargo workspace root clean while versioning contracts and agent-managed specs in Git.
- **Affected Contracts:** `src/skerry/index.prose.md`

## ARCH-002: Consolidate Product Docs, Roadmap & Tickets under `.agents/prose/specs/`

- **Status:** settled
- **Decision:** Migrate all files from `docs/` (`gpui-notes.md`, `skerry-mac/DESIGN.md`, `skerry-mac/TODO.md`, `skerry-mac/running.md`, `skerry-mac/milestone-1-qa.md`, `skerry-mac/selkies-sidebar-reference.md`, and all 28 ticket files in `skerry-mac/tickets/*.md`) into `.agents/prose/specs/` and retire the top-level `docs/` directory.
- **Rationale:** Product specs, architecture notes, and feature tickets are consumed and maintained exclusively by agents. Consolidating them under `.agents/prose/specs/` as the maintained world-model of `src/skerry/index.prose.md` eliminates split brain between human docs and agent state.
- **Affected Contracts:** `src/skerry/index.prose.md`, `src/skerry/implement-feature.prose.md`
