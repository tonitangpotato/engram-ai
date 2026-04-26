# ISS-024 WIP snapshot from deprecated repo

Ported 2026-04-23 from `engram-ai-rust/` after ISS-023 consolidation, as part of ISS-025.

## Contents

- **`iss-024-monorepo.patch`** — diff of 4 modified files:
  - `crates/engramai/src/dimensions.rs`
  - `crates/engramai/src/lib.rs`
  - `crates/engramai/src/memory.rs`
  - `crates/engramai/Cargo.toml`
  
  (Paths rewritten from deprecated layout to monorepo layout.)

- **`graph.yml.post-iss024-design`** — graph snapshot after ISS-024 design phase.

- **New files already copied into place** (not in this dir):
  - `crates/engramai/src/dimension_access.rs`
  - `crates/engramai/src/temporal_dim.rs`
  
  These were new files (untracked in deprecated), so they were copied directly rather than staged as patches. They will not compile standalone — they depend on the modifications in `iss-024-monorepo.patch`.

## Status

**Uncommitted WIP.** Compile status in the deprecated repo at port time was unknown — ISS-024 was mid-implementation when consolidation happened.

## How to resume

1. From monorepo root:
   ```bash
   git apply .gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/wip/iss-024-monorepo.patch
   ```
2. Then try to build:
   ```bash
   cargo check -p engramai
   ```
3. If green → continue ISS-024 implementation (see the main ISS-024 spec in the parent dir).
4. If red → read compiler errors. Most likely issues:
   - `dimension_access` / `temporal_dim` modules missing `pub use` wiring in `lib.rs` (check patch for `pub mod dimension_access;` etc.)
   - Missing imports in `memory.rs` for new types
   - `Cargo.toml` deps (`two_timer`, `lru`) need `cargo update` first

## Why staged separately instead of committed

The dimension_access/temporal_dim files exist on disk (copied), but the `lib.rs`/`memory.rs` modifications that wire them in are in the patch. This is intentional:
- Leaving the patch unapplied keeps the monorepo in a buildable state (new files are just dead code until imports are added)
- When resuming ISS-024, applying the patch is a single clean step, and any conflicts are obvious
