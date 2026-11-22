# ARCH-001: Attached OpenProse Root & Harness Selection

- **Status:** settled
- **Mode:** bootstrap (`prose init`)
- **Decision:** Establish OpenProse as an attached repository root at `.agents/prose/` using the default `filesystem` state backend and in-session Prose Complete agent harness.
- **Rationale:** Skerry is an existing Rust/GPUI workspace (`Cargo.toml`, `crates/skerry-mac`). Using `.agents/prose/` keeps the Cargo workspace root clean while versioning contracts and architecture records in Git.
- **Affected Contracts:** `src/skerry/index.prose.md`
