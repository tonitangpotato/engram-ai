---
id: ISS-132
title: apply_graph_delta rejects backfill entity embeddings with dim mismatch (ISS-044 regression)
status: resolved
priority: P1
severity: blocker
created: 2026-05-15
resolved: 2026-05-22
relates_to: [ISS-044, ISS-134, ISS-135]
fixed_by: pending-commit
labels: [substrate, v04, regression, migrate, embedding]
---

# Problem

`crates/engramai-migrate/tests/iss044_backfill.rs` has 3 integration
tests covering the v0.2 → v0.3 backfill happy path. As of 2026-05-15,
2 of those 3 tests fail on trunk:

```
test test_backfill_completes_against_populated_v02_db ... FAILED
  BackfillFatal("apply_graph_delta: invariant violation:
                 entity embedding dim mismatch")

test test_backfill_idempotent_on_v03_db ... FAILED
  first migrate: BackfillFatal("apply_graph_delta: invariant violation:
                                entity embedding dim mismatch")

test test_backfill_dry_run_does_not_write ... ok  (no actual write,
                                                   so embedding path
                                                   not exercised)
```

The `dry_run` test passes — confirms the regression is in the
write path, specifically `apply_graph_delta` rejecting entity
embeddings supplied by the backfill driver.

# Why this is a regression

`aff16dc` (2026-04-28) shipped these tests with ISS-044 and they
were green at that point. Somewhere between then and `HEAD`
(2026-05-15), a change to either:

- the backfill driver's embedding generation (likely in
  `engramai-migrate/src/backfill.rs` or its `PipelineRecordProcessor`),
- the resolution pipeline that builds the `GraphDelta`,
- the substrate dual-write writers (T13 / T21 / T22 / T29.3),
- the `apply_graph_delta` invariant check itself (it appears in
  `crates/engramai/src/graph/store.rs` at lines 1485, 1509, 2007, 2281),

…changed the embedding dimensionality assumption on either the
producer or consumer side, breaking the contract.

# Most likely root cause

The v04-unified-substrate Phase B/C/D campaign added dual-write of
entities/embeddings into `nodes` and `node_embeddings`. The newer
write path probably normalized to a specific `embedding_dim`
(`graph/store.rs:654` documents this is locked at store construction
time). The backfill driver still uses the v0.2-era default — likely
a different dim (e.g. simple_hash_embedding default of 64 vs newer
default of 128 or 384).

Both write paths have to agree on `embedding_dim` for the same
`SqliteGraphStore`. Right now they disagree.

# How to bisect

```bash
cd /Users/potato/clawd/projects/engram
git bisect start
git bisect bad HEAD
git bisect good aff16dc
git bisect run sh -c "cargo test -p engramai-migrate --test iss044_backfill test_backfill_completes_against_populated_v02_db 2>&1 | grep -q 'test result: ok'"
```

# Blocks

- **ISS-044** (Wire MigrationOrchestrator::run_backfill to
  PipelineRecordProcessor): cannot flip from in_review to done while
  this regression is present.
- **v0.4 substrate flip (T31/T32 in v04-unified-substrate design.md)**:
  if migrate is broken, the migration path from v0.2 → v0.3 → v0.4
  is broken end-to-end. T31 needs migrate working for the parity
  campaign.

# Acceptance criteria

- [x] Identify the commit that broke iss044_backfill (latent
      typo, not a regression commit — 384 was wrong from the
      start; surfaced when Phase B/C/D dual-write tightened the
      `apply_graph_delta` dim invariant check)
- [x] Root-fix the dim-mismatch contract (flipped
      `resolution::DEFAULT_EMBEDDING_DIM` 384 → 768 to match the
      graph store invariant; no test-fixture papering)
- [x] 2/3 → 3/3 `iss044_backfill` tests pass for the
      dim-mismatch path. (`test_backfill_completes_against_populated_v02_db`
      still fails — but on the orthogonal `DeferToLlm` path
      tracked separately in ISS-135.)
- [x] No new regressions in `engramai-migrate` or `engramai` lib
      tests
- [ ] Once fixed, ISS-044 can be closed with the regression-fix
      commit also referenced (still blocked by ISS-135)

# Out of scope

- Anything beyond fixing the dim-mismatch contract. If bisect
  surfaces broader substrate writer issues, file separate ISSs.

# References

- ISS-044 — the original wiring task (in_review pending this fix)
- v04-unified-substrate design §5.3 (Phase B/C/D dual-write campaign)
- `crates/engramai/src/graph/store.rs:1485,1509,2007,2281` — the
  four call sites that emit this exact error message
- `crates/engramai-migrate/tests/iss044_backfill.rs:113,190` — the
  failing test assertions

# Resolution (2026-05-22)

**Root cause**: `crates/engramai/src/resolution/mod.rs:145`
declared `pub const DEFAULT_EMBEDDING_DIM: usize = 384` while the
graph store invariant
(`crates/engramai/src/graph/store.rs:DEFAULT_ENTITY_EMBEDDING_DIM`)
locks the column to **768** (matches the production
`nomic-embed-text` embedder). The resolution module's
`default_embedder` ships an `IdentityEmbedder` that emits vectors
of the constant's dim — so backfill drafts arrived at
`apply_graph_delta` with 384-d embeddings and were rejected.

The 384 value was wrong from the start (the original comment
claimed it matched `nomic-embed-text` which is 768) — this was a
latent typo, surfaced by Phase B/C/D dual-write asserting the dim
invariant more strictly.

**Fix**: flip `DEFAULT_EMBEDDING_DIM` to `768`. Comment updated
to point at the three-site duplication and ISS-134 (single
source of truth follow-up).

**Verification**: `cargo test -p engramai-migrate --test
iss044_backfill` — 2/3 → 3/3 dim-mismatch path. The 1 remaining
failure (`test_backfill_completes_against_populated_v02_db`) is
**not** the dim mismatch; it is the
`DeferToLlm`-reaches-persist path tracked in **ISS-135**.

# Follow-ups

- **ISS-134** — consolidate `DEFAULT_EMBEDDING_DIM` /
  `DEFAULT_ENTITY_EMBEDDING_DIM` / `EmbeddingConfig::default().dimensions`
  onto a single source of truth (this fix patched only one of
  three constants; the next dim swap will hit the same
  inconsistency unless consolidated).
- **ISS-135** — `Decision::DeferToLlm` reaches persist for
  backfill records that hit tier-B similarity (e.g. `engramai-rs`
  vs `gid-rs`). Design §8.1 specifies `Conservative` default
  (fallback to `CreateNew` with `tiebreak_failed = true` trace
  flag), but code currently fails loud (`unresolved_defer`
  failure row). This is the remaining ISS-044 blocker.

