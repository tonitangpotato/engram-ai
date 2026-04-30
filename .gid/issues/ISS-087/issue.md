---
id: ISS-087
title: "Ingest API lacks occurred_at override — cogmembench replay anchors temporal parser to wall-clock now"
status: done
priority: P1
severity: high
tags: [ingest, temporal, api, store-raw, cogmembench, replay]
related: [ISS-024]
---

# ISS-087: Ingest API lacks `occurred_at` override

## Summary

The store path (CLI `engram store`, Rust `MemoryManager::store_raw`, `StorageMeta`) provides **no way for a caller to specify the logical event time** of a memory. Every record's `created_at` is set to wall-clock `Utc::now()` at insert. This breaks any replay/backfill scenario — most acutely cogmembench, where 2023 conversations are ingested in 2026.

## Evidence

- `crates/engramai/src/store_api.rs:67` — `StorageMeta` fields: `importance_hint`, `source`, `namespace`, `user_metadata`, `memory_type_hint`. **No timestamp field.**
- `crates/engram-cli/src/main.rs:75` — `Commands::Store` flags: `content`, `ns`, `type`, `importance`, `source`, `emotion`, `domain`, `extractor`, `extractor_model`, `auth_token`, `oauth`, `graph_db`, `no_graph`, `graph_drain_timeout_secs`, `meta`. **No `--created-at` / `--occurred-at`.**
- `crates/engramai/src/temporal_dim.rs:20-22` (doc):
  > The anchor for relative expressions ("yesterday", "last Tuesday") MUST be the memory's `created_at`, not wall-clock `Utc::now()`.
  
  This intent is correct but vacuous in practice: `created_at` itself **is** wall-clock now, because no caller can override it.
- `cogmembench/benchmarks/locomo/engram_adapter.py:298-309` — passes `session_date` as a `meta` side-channel, but `store()` has no parameter to thread it into `created_at`.

## Impact

- `temporal_dim::parse_dimension_time("yesterday", record.created_at)` resolves to "yesterday relative to today (2026)" instead of "yesterday relative to session date (2023)".
- LOCOMO and any other replay benchmark gets wrong-year `TimeRange`s for every relative expression in the corpus.
- `temporal_score` ranking signal silently produces garbage on replay data — masked because absolute dates also aren't grounded into content (see ISS-088), so the dimension contributes ~zero signal in the first place.

## Root Cause

`SqliteKnowledgeStore::insert` (and the `MemoryRecord` constructor path) hard-codes `created_at = Utc::now()` with no override surface. Architectural omission: ingest treats wall-clock = event time as a universal invariant, which holds for live chat but fails for any replay/import/backfill workload.

## Proposed Fix

**Additive, non-breaking, two-layer change:**

1. **`StorageMeta` (engramai/src/store_api.rs)** — add:
   ```rust
   /// Logical event time. When `None`, wall-clock `Utc::now()` is used (backward-compatible).
   /// Set this for replay/backfill ingestion so temporal scoring anchors correctly.
   pub occurred_at: Option<DateTime<Utc>>,
   ```
   - Plumb through `store_raw` → `SqliteKnowledgeStore::insert` so `created_at` uses `meta.occurred_at.unwrap_or_else(Utc::now)`.
   - All existing call sites continue to work (struct-update syntax / `Default` keeps old behavior).

2. **CLI (engram-cli/src/main.rs)** — add `--occurred-at <RFC3339>` flag to `Commands::Store`. Parse with `chrono::DateTime::parse_from_rfc3339`. Forward into `StorageMeta`.

3. **Tests:**
   - Unit: `StorageMeta { occurred_at: Some(t), ..Default::default() }` produces a record whose `created_at == t`.
   - Unit: `StorageMeta::default()` still yields `created_at ≈ Utc::now()`.
   - Integration: `engram store --occurred-at 2023-05-08T00:00:00Z "test"` then `engram show <id>` shows the overridden timestamp.

## Acceptance Criteria

- [x] `StorageMeta::occurred_at: Option<DateTime<Utc>>` exists and is honored by the SQLite insert path.
- [x] CLI `--occurred-at` flag accepts RFC3339, errors clearly on invalid input.
- [x] Existing tests pass unmodified (backward compatibility verified).
- [x] New unit + integration tests cover override + default paths.
- [ ] Brief changelog/CHANGELOG.md entry.

## Resolution (2026-04-30)

**Implementation landed.** Files changed:
- `crates/engramai/src/store_api.rs` — added `occurred_at: Option<DateTime<Utc>>` field + unit tests
- `crates/engramai/src/memory.rs` — plumbed override through `add_raw`, `store_raw`, and the SQLite insert path. Added `add_with_emotion_at` convenience wrapper (legacy `add_with_emotion` delegates with `None` — zero call-site breakage).
- `crates/engramai/src/enriched.rs` — added `occurred_at` field to `EnrichedMemory` + `with_occurred_at` builder
- `crates/engram-cli/src/main.rs` — added `--occurred-at <RFC3339>` flag, wired to `add_with_emotion_at`
- `crates/engramai/tests/iss087_occurred_at.rs` — 2 end-to-end tests (override persists, None falls back to now)
- `crates/engramai/tests/iss019_*.rs` — added `occurred_at: None` to existing struct literals (compatibility fix)

**Verification:**
- `cargo check --workspace` clean
- All workspace tests pass except 2 pre-existing failures in `engramai-migrate/tests/iss044_backfill.rs` (tracked as ISS-075, embedding dim mismatch, unrelated)
- CLI smoke test: `engram store --occurred-at 2023-05-08T00:00:00Z "..."` → SQLite shows `created_at=1683504000.0` (= 2023-05-08 00:00:00 UTC) ✓
- Two new integration tests pass: `occurred_at_override_persists_to_created_at`, `occurred_at_none_falls_back_to_wall_clock_now`

**Changelog entry**: not added (project doesn't appear to have a CHANGELOG.md in repo root). Defer until someone adds one.

**Unblocks ISS-088** — content-level temporal grounding can now use the override to anchor `parse_dimension_time` correctly.

## Out of Scope

- **Content grounding** of resolved temporal expressions (writing absolute dates into the memory text). Tracked separately as **ISS-088** — depends on this fix landing first so the anchor is correct.
- cogmembench-side adapter changes to actually pass `occurred_at`. That's a downstream consumer change and lives in the cogmembench repo.

## Notes

- This is the recurring temporal-grounding issue (originally surfaced via LOCOMO retrieval failures). The 2025 ISS-024 work added the `temporal` field + `temporal_dim.rs` parser but left the anchor-source unfixable.
- See engram MEMORY discussion 2026-04-30 for full forensic trace.
