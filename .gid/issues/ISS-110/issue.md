---
id: ISS-110
title: "synthesis lost unified-clustering refactor in ISS-023 monorepo sync"
status: open
priority: medium
labels: [regression, synthesis, clustering, process]
created: 2026-05-08
relates_to: [ISS-023]
---

# synthesis lost unified-clustering refactor in ISS-023 monorepo sync

## Summary

`synthesis/cluster.rs` currently calls `infomap_rs::Infomap` directly. It was supposed to go through `clustering::InfomapClusterer<MultiSignal>` per ADR `docs/adr-unified-clustering.md`. The unified path **was shipped** but was silently reverted three days later by an unrelated bulk-sync commit.

## Discovery

Found 2026-05-08 while writing `docs/architecture-consolidation-synthesis-kc.md`. Code did not match ADR; git archaeology pinned the regression.

## Timeline

- **2026-04-18** `d44dc4f` — initial monorepo restructure. `synthesis/cluster.rs` uses `infomap_rs::{Infomap, Network}` directly (carried over from old engram-ai-rust repo).
- **2026-04-19** `2108e67` ("refactor: improve discovery pipeline, unified clustering, storage enhancements") — synthesis refactored to use shared `clustering::InfomapClusterer<MultiSignal>`. Module doc said: "This module is an adapter: it loads signal data from Storage, builds `ClusterNode`s, runs the shared clusterer, and converts results back to synthesis-specific `MemoryCluster` structs." 766 lines changed in synthesis/cluster.rs; net −339 lines (refactor was a clean simplification).
- **2026-04-19** `5e0d4ba` — ADR document committed describing the merged state.
- **2026-04-22** `3132194` ("feat: consolidate engram-ai-rust into monorepo (ISS-023)") — one-shot bulk sync from old `engram-ai-rust` repo into `crates/engramai/src/`. The old repo never received the 04-19 refactor. Sync used directional "old repo wins" overwrite, **silently undoing the unified-clustering refactor for synthesis**. ADR document was untouched and continued to describe the (now non-existent) merged state.

## Root cause

ISS-023 was a directional one-way sync (`engram-ai-rust → monorepo`) without per-file diff review. Files that had diverged in the monorepo since the last sync were overwritten without flagging. `synthesis/cluster.rs` had diverged on 04-19, was overwritten on 04-22.

## Fix scope

Two parts:

### Part A — restore the unified path
The diff still exists at `2108e67`. Re-apply roughly:

```
git show 2108e67 -- crates/engramai/src/synthesis/cluster.rs
```

Caveats since 04-19:
- `synthesis/cluster.rs` has been touched twice since (`3132194` overwrite + `77c3e28` clippy lint). Only the lint commit added real changes on top of the post-revert version, and clippy fixes will need re-applying after the refactor lands.
- `clustering.rs` API may have evolved — verify `InfomapClusterer<MultiSignal>` and `MultiSignal` strategy still exist and have the same signature. Quick check: `grep -n "MultiSignal\|InfomapClusterer" crates/engramai/src/clustering.rs`.
- `compute_pairwise_signals` and `compute_composite_score` (currently `synthesis/cluster.rs:26, 85`) may need to be removed or moved into the `MultiSignal` strategy.
- All synthesis tests must still pass after the refactor.

### Part B — process finding (sync hygiene)
- Future cross-repo syncs must run `git diff` per file and surface any divergence before applying.
- Or: deprecate one-way syncs entirely now that monorepo is canonical.
- Add a CI check that compares ADR-described architecture against actual imports? (open — probably overkill, but the failure mode is real.)

## Acceptance criteria

- [ ] `synthesis/cluster.rs` no longer imports `infomap_rs::{Infomap, Network}` directly.
- [ ] `synthesis/cluster.rs` calls into `clustering.rs` shared engine (e.g. `cluster_with_infomap` or `InfomapClusterer<MultiSignal>`, whichever is current).
- [ ] All existing synthesis tests pass.
- [ ] `docs/architecture-consolidation-synthesis-kc.md` open question #2 updated to "Resolved — re-applied".
- [ ] (Part B) Process note added to `docs/` or `.gid/` describing how future cross-repo syncs should be diff-reviewed.

## Out of scope

- Whether synthesis and KC should *fully* merge (different output targets, different signal sets) — see open question #1 in `docs/architecture-consolidation-synthesis-kc.md`. That's a bigger design call; this issue only covers restoring what was lost.
- KC's K2 cosine-only degeneration on single-domain corpora — separate issue (engram ISS-109 / rustclaw ISS-107).

## References

- ADR: `docs/adr-unified-clustering.md` (2026-04-18)
- Architecture map: `docs/architecture-consolidation-synthesis-kc.md` (2026-05-07)
- Refactor commit (still recoverable): `2108e67`
- Reverting commit: `3132194` (ISS-023)
- Current state: `crates/engramai/src/synthesis/cluster.rs:17`
