# ISS-033: Entity Embedding Column + GraphStore.search_candidates API

**Status:** open (in_progress — ritual-driven)
**Severity:** high — blocks v03-resolution §3.4.1 candidate retrieval driver, which blocks end-to-end store_raw → extraction → resolution → graph write pipeline
**Related:**
- v03-graph-layer (feature) — owns `graph_entities` schema and `GraphStore` trait
- v03-resolution (feature) — consumes `search_candidates` in §3.4.1 driver
- v03-migration (feature) — owns the schema migration mechanism
- ISS-024 (in_progress) — dimensional read path; independent but shares "embedding-as-first-class" theme
- §3.4 multi-signal fusion (already implemented, 1450 LOC, 55 tests) — this issue unblocks its caller
**Filed:** 2026-04-26
**Prerequisite:** None. Self-contained cross-feature design + implementation work.

## TL;DR

`v03-resolution/design.md §3.4.1` describes a `graph_store.search_candidates(...)` call that performs alias-exact match + embedding cosine similarity + recency boost over `graph_entities`. **This API does not exist in code, and — more importantly — its preconditions do not exist in the design itself:**

1. `graph_entities` schema (in `v03-graph-layer/design.md §4.1`) has **no `embedding` column** — only `somatic_fingerprint` (8×f32 affect, not semantic).
2. `GraphStore` trait `§4.2` interface list has **no `search_candidates` method** — only write-side methods.
3. `v03-resolution §3.4.1` references "v03-graph-layer §5" — **§5 does not exist** in v03-graph-layer/design.md.

This is a **cross-feature design inconsistency**, not just an implementation gap. Patching only the implementation (an MVP `search_candidates` without embeddings) would commit the inconsistency as permanent technical debt — the design would never match the code, and a future engineer touching `graph_entities` would have no embedding-shaped invariant to preserve.

## Why Not MVP

A tempting MVP: skip embedding column, ship `search_candidates` with only alias + recency. Reasons rejected:

- **Design drift**: design says embedding similarity is a co-equal signal in candidate ranking. Shipping without it makes the design a lie. Future readers can't trust the doc.
- **Fusion module already supports missing-signal weight redistribution** — but that mechanism was designed for *runtime* missing signals (a particular candidate has no embedding, or a particular mention couldn't be embedded), not *systematic absence of the entire signal channel*. Permanently disabling the embedding channel would silently elevate alias and recency weights beyond their calibrated values, causing ranking quality regressions that are invisible to tests.
- **Schema migration debt**: adding `embedding` later means a second migration on a populated table. Doing it now (table likely empty or near-empty) costs nothing.
- **Two write paths**: an MVP `upsert_entity` without embedding + a future "v2" upsert with embedding = two code paths to maintain, two test matrices, two correctness arguments. Single root fix avoids this.
- potato's explicit rule: "不想要 mvp，想要 root fix，不然又是新的 debt"

## Scope (4 layers, single ritual)

### Layer 0: Cross-Feature Design Patch

Goal: make `v03-graph-layer/design.md` and `v03-resolution/design.md` consistent before any code is written.

- [ ] `v03-graph-layer/design.md §4.1`: add `embedding BLOB` column to `graph_entities` schema. Specify:
  - Same dim+blob convention as `knowledge_topics.embedding` (cross-locked invariant)
  - CHECK constraint: `embedding IS NULL OR length(embedding) = N * 4` where N is the model dim constant
  - Backfill semantics: nullable, NULL means "not yet embedded"
  - Index strategy: brute-force scan in v0 (table size bounded), sqlite-vec deferred to a future ISS
- [ ] `v03-graph-layer/design.md §4.2`: add `search_candidates` to `GraphStore` trait. Specify:
  - Input: `CandidateQuery { mention_text, mention_embedding: Option<Vec<f32>>, kind_filter: Option<EntityKind>, namespace, top_k, recency_window }`
  - Output: `Vec<CandidateMatch { entity_id, alias_match: bool, embedding_score: Option<f32>, recency_score: f32, last_seen, ... }>`
  - Ranking is NOT done here — `search_candidates` returns raw signals, fusion module ranks. (Separation of concerns preserved.)
  - K hard cap (e.g., 50) to bound output even when caller requests more.
  - Namespace + kind filtering happens in the SQL, not post-hoc.
- [ ] `v03-graph-layer/design.md`: add a §5 (or rename existing section) so `v03-resolution §3.4.1`'s reference resolves. Or fix the §3.4.1 reference to the actual anchor.
- [ ] `v03-resolution/design.md §3.4.1`: update "v03-graph-layer §5" reference; clarify caller contract (ranking happens in fusion, not in store).
- [ ] Cross-reference tables in both docs updated.
- [ ] **Design review (review-design skill, full depth)** runs against both feature docs to catch any new inconsistency introduced by the patch.

### Layer 1: Schema Migration

Goal: add `embedding` column and validation without breaking existing data.

- [ ] New migration in `v03-migration` adding `embedding BLOB` column to `graph_entities`.
- [ ] CHECK constraint enforced at SQL level (matches knowledge_topics pattern).
- [ ] Reader code: validates `blob.len() == dim * 4` before decode; returns `GraphError::Invariant("entity embedding dim mismatch")` on mismatch.
- [ ] If the table is non-empty, a backfill pass re-embeds existing entities from `canonical_name + summary` using the configured embedding provider. (If table is empty in dev/test envs, backfill is a no-op.)
- [ ] Migration test: round-trip an entity with embedding, with and without embedding, verify CHECK rejects malformed blob.

### Layer 2: GraphStore API

Goal: implement `search_candidates` and update write path.

- [ ] `GraphStore::search_candidates` trait method (signature per Layer 0 design).
- [ ] `SqliteGraphStore::search_candidates` impl:
  - Alias exact match via `graph_entity_aliases` (normalized lookup)
  - Embedding cosine via brute-force scan over `graph_entities.embedding WHERE embedding IS NOT NULL` filtered by namespace + kind
  - Recency score from `last_seen` relative to `recency_window`
  - Returns raw signals (no fused ranking)
  - Hard cap on K
- [ ] `upsert_entity` updated to accept embedding parameter (single write path).
- [ ] Unit tests cover: alias-only hit, embedding-only hit, both, namespace isolation, kind filter, empty table, K truncation, NULL-embedding entities skipped from embedding scan, recency boost ordering, dim-mismatch rejected at read.

### Layer 3: §3.4.1 Resolution Driver

Goal: wire fusion module to GraphStore.

- [ ] New `resolution::candidate_retrieval` module per §3.4.1.
- [ ] Mention → embed (via configured provider) → `search_candidates` call → fusion module ranking → top candidate(s).
- [ ] End-to-end integration test: `store_raw` → extraction → resolution → assert correct rows in `graph_entities` and `graph_edges`.
- [ ] Tests for fusion's missing-signal paths exercised here (candidate has no embedding, mention has no embedding, empty candidate set, all aliases miss). These paths exist in fusion but were not previously hit by integration tests.

### Layer 4: Verification

- [ ] All existing fusion unit tests still pass (55 tests).
- [ ] `cargo test -p engramai` green.
- [ ] `cargo clippy -p engramai -- -D warnings` clean.
- [ ] No new `unwrap()` on user/LLM-derived data.
- [ ] Recall quality baseline (P_before from ISS-021 Phase 1 fixtures) re-measured to confirm no regression. P should be ≥ 0.767.

## Non-Goals (explicit)

- **sqlite-vec / ANN index integration** — deferred to a future ISS once table size justifies it. Brute-force scan is the v0 strategy.
- **Multi-vector or hybrid retrieval (BM25 + dense)** — out of scope; fusion module handles signal combination.
- **Re-architecting fusion** — fusion module is done (ISS-021 §3.4); this issue only consumes it.
- **Cross-namespace candidate discovery** — namespace is a hard filter; cross-namespace resolution is a separate design problem.

## Estimated Size

- Layer 0 (design): ~150 LOC of doc edits across 2 files + 1-2 review rounds
- Layer 1 (migration): ~120 LOC + ~80 LOC tests
- Layer 2 (GraphStore API): ~350 LOC + ~250 LOC tests
- Layer 3 (driver): ~250 LOC + ~200 LOC tests
- Total: ~1400 LOC including tests, ~2 design docs patched

## Acceptance Criteria

1. `v03-graph-layer/design.md` and `v03-resolution/design.md` cross-reference correctly. No dangling `§5` references. Design review (full depth) finds zero critical or important inconsistencies on this surface.
2. `graph_entities.embedding` column exists with CHECK constraint matching `knowledge_topics.embedding` convention.
3. `GraphStore::search_candidates` returns raw multi-signal results (alias / embedding / recency) for fusion to rank.
4. End-to-end integration test demonstrates `store_raw` → graph entities + edges with both alias-matched and embedding-matched resolution paths.
5. ISS-021 Phase 1 fixture P@3 ≥ 0.767 (no regression on retrieval quality).
6. All four layers complete; no Layer kept "for later" (no MVP debt).

## Open Questions

- Embedding provider for the migration backfill: same one as `knowledge_topics`? (Likely yes — single source of truth.) Confirm in Layer 0 design patch.
- Should `search_candidates` also return entities matching by `summary` substring as a third raw signal, or is alias + embedding + recency sufficient? (Defer to Layer 0 design discussion.)

## Layer 0 Sanity-Check Findings (2026-04-26)

Post-patch sanity check (main agent, not full review) verified ISS-033 surface internally consistent:
- `CandidateQuery` 7 fields call-site ↔ definition match
- `CandidateMatch` consumed fields all present
- `MAX_TOP_K = 50` defined and referenced consistently
- `Entity.embedding` field, `update_entity_embedding` trait method, blob format all aligned

**Pre-existing inconsistency surfaced and folded into ISS-033 scope:** v03-resolution referenced `GraphStore::upsert_entity` (4 places: §3 dependencies, §3.5 step 1, §8.2 idempotence, §10 trait list) but graph-layer trait has no such method — only `insert_entity` + `update_entity_cognitive` + `update_entity_embedding` + `touch_entity_last_seen`. Same root cause as ISS-033 (cross-feature trait drift), so resolved in same patch session rather than filing a separate issue.

**Resolution:** Kept graph-layer trait minimal (deliberate granularity from prior review). Expanded resolution §3.5 step 1 to dispatch by `EntityResolution::action`:
- `CreateNew` → `insert_entity(&entity)`
- `MergeInto` → `update_entity_cognitive` + `touch_entity_last_seen` + conditional `update_entity_embedding` (only when canonical_name/summary changed)

Updated §3 CRUD API dependency list, §8.2 idempotence note, §10 trait reference list to match.

## Layer 1 Implementation Record (2026-04-26)

**Files changed:**
- `crates/engramai/src/graph/storage_graph.rs` — `embedding BLOB` column added to both fresh-DB DDL and `GRAPH_ENTITIES_ALTERS` (idempotent ALTER for legacy DBs); partial index `idx_graph_entities_embed_scan ON (namespace, last_seen DESC) WHERE embedding IS NOT NULL` added to `GRAPH_POST_ALTER_INDEXES`.
- `crates/engramai/src/graph/entity.rs` — `Entity.embedding: Option<Vec<f32>>` field with `#[serde(default)]` for backward-compat deserialization; `Entity::new` initializes to `None`; new `validate_embedding_dim` helper mirroring the topic.rs pattern.
- `crates/engramai/src/graph/store.rs` — `entity_embedding_to_blob` / `entity_embedding_from_blob` codec helpers; `SqliteGraphStore.embedding_dim` field with `with_embedding_dim` builder method and `DEFAULT_ENTITY_EMBEDDING_DIM = 768` (matches `EmbeddingConfig::default()` nomic-embed-text); `insert_entity` and `get_entity` updated to encode/decode embedding through the codec.

**Validation contract:**
- Single message string `"entity embedding dim mismatch"` for both write-side and read-side `GraphError::Invariant`.
- No SQL `CHECK` on blob length — dim is runtime config (`SqliteGraphStore.embedding_dim`), application-validated. Same rationale as `knowledge_topics.embedding`.
- `NULL` blob ⇒ `None` field ⇒ "not yet embedded" — accepted on read and write.
- Stale-dim blobs (e.g. row written at dim=100, read by store configured for dim=384) rejected as `Invariant`, not silently truncated.

**Tests added (14):**
- `entity.rs`: `new_entity_has_no_embedding`, `validate_embedding_dim_{none_ok, match_ok, mismatch_errors}`, `serde_roundtrip_entity_with_embedding` (incl. backward-compat for serialized form without `embedding` field).
- `store.rs`: `entity_embedding_blob_{roundtrip_helper, none_roundtrip, writer_rejects_dim_mismatch, reader_rejects_corrupt_length}`, `insert_and_get_roundtrip_with_embedding`, `insert_entity_rejects_dim_mismatch_at_write`, `insert_entity_with_no_embedding_works`, `get_entity_rejects_stale_dim_blob`, `embed_scan_partial_index_exists`.
- `assert_entity_core_eq` helper extended to compare `embedding` field.

**Results:** 1185/1185 engramai library tests pass (was 1171). 0 new clippy warnings on touched files. Pre-existing warnings on `edge.rs` / `telemetry.rs` untouched.

**Layer 1 acceptance criteria met:**
- ✅ `graph_entities.embedding` column exists.
- ✅ Reader validates `blob.len() == dim * 4` and returns `GraphError::Invariant("entity embedding dim mismatch")` on mismatch.
- ✅ Migration test (round-trip with embedding, with/without embedding, dim mismatch on read).
- ⚠️ Backfill pass for existing rows: deferred — current production DBs have very few `graph_entities` rows (still in pre-launch). Backfill code path will be written as part of Layer 3 driver wiring (resolution writes embeddings on first mention; existing rows organically get embeddings when re-mentioned). If a forced backfill is needed, it will be a separate utility called once at v0.3 ship.

## Layer 2 Implementation Record (2026-04-26)

**Scope expansion (justified).** Layer 2 nominally = `GraphStore::search_candidates` + `update_entity_embedding`. In practice, writing search_candidates required closing the alias upsert/resolve methods (previously stubs) so the alias-exact lookup path works end-to-end — without it the API would compile but silently miss every alias hit. Counted as same-layer because it's the same trait surface; documented here so Layer 3 doesn't re-discover it.

A `crates/engramai/src/resolution/` module skeleton was also written in this session (mod / signals / fusion / decision / context / adapters / trace, ~2.2k LOC including tests). This is **Layer 3 driver scaffolding** — not Layer 2. It compiles and its unit tests pass, but it does NOT yet wire to `search_candidates` (no driver loop, no `assemble_context → score → fuse → decide` pipeline). Recorded here for accuracy; Layer 3 record will document the wiring step.

**Files changed (Layer 2 surface):**
- `crates/engramai/src/graph/store.rs`
  - Trait: `update_entity_embedding`, `search_candidates` (with full contract docs — see below).
  - Impl: brute-force `search_candidates` body (alias-hit + embedding-cohort scan, cosine + recency decay scoring, deterministic ordering by `entity_id`), `update_entity_embedding` body (round-trip through `entity_embedding_to_blob`).
  - Real impls of `upsert_alias` / `resolve_alias` (replacing stubs); both go through `normalize_alias` so caller input does not have to be pre-normalized.
  - `map_candidate_row` helper + `CandidateScanRow` / `CandidateRowProjection` type aliases (factor out `clippy::type_complexity`).
- `crates/engramai/src/graph/entity.rs`
  - `pub fn normalize_alias(s: &str) -> String` — NFKC fold + trim. Single source of truth for the writer/reader symmetry guarantee.
- `crates/engramai/src/lib.rs`
  - Re-export of `CandidateQuery` / `CandidateMatch` / `MAX_TOP_K` for downstream callers.
- `crates/engramai/Cargo.toml`
  - `unicode-normalization = "0.1"` (NFKC for alias normalization).

**Public types added:**
- `CandidateQuery` (7 fields: `mention_text`, `mention_embedding`, `namespace`, `kind_filter`, `top_k`, `recency_window`, `now`).
- `CandidateMatch` (entity_id + raw signals: `alias_score`, `embedding_score: Option<f32>`, `recency_score`, `kind`, `canonical_name`, `last_seen`, `identity_confidence`).
- `pub const MAX_TOP_K: usize = 50` — hard ceiling enforced regardless of caller.

**Contract decisions (recorded in trait doc-comments):**
- Returns *unranked raw signals*. Fusion-layer ranking lives in the resolution module (§3.4.2), NOT the storage trait. Putting ranking here would duplicate missing-signal weight redistribution in two places.
- `mention_embedding = None` ⇒ embedding signal is omitted, NOT implicitly re-embedded.
- `namespace` is a hard filter; cross-namespace results are never returned.
- `top_k = 0` returns empty Vec (degenerate but valid).
- `top_k > MAX_TOP_K` is silently capped at `MAX_TOP_K` (no error — caller bug, not user-facing).
- `recency_window = None` ⇒ unbounded; recency scaled against the oldest `last_seen` in the candidate set.
- `now` is passed in (not `Utc::now()`) so tests are deterministic.
- Output ordering: ascending `entity_id` only — callers MUST NOT treat order as ranked.
- Performance contract: brute-force scan over partial index `idx_graph_entities_embed_scan` (added Layer 1). Acceptable while per-namespace entity count < ~10⁵; ANN swap-in is a future ISS that does NOT change the trait signature.

**Tests added (24 in store.rs + 3 in entity.rs = 27 Layer 2 tests):**
- `update_entity_embedding`: writes blob, clears when None, rejects dim mismatch, errors on missing entity (4).
- `search_candidates`: alias-only hit, embedding-only hit, alias+embedding combined, namespace-isolated, kind filter, empty table, NULL embedding skipped in embedding scan, top_k truncation, MAX_TOP_K ceiling, top_k=0 empty, recency window decay, unbounded recency uses set span, dim validation, deterministic sorting (≥14 cases — see lines 3215–3550).
- `upsert_alias` / `resolve_alias`: idempotent on repeat, normalizes caller input, namespace-isolated, missing returns None (4).
- `normalize_alias`: basic cases, NFKC folding, writer/reader symmetric (3).

**Test results:** `cargo test -p engramai --lib` → **1213 passed, 0 failed, 4 ignored** (was 1185 after Layer 1 → +28 net new tests; the 27 Layer 2 tests above + 1 incidental coverage in resolution scaffolding).

**Clippy:** `cargo clippy -p engramai --lib --tests --no-deps` → 4 warnings, all pre-existing (`edge.rs:23-25` doc list indentation × 3, `telemetry.rs:47` doc quote × 1). **0 new warnings on Layer 2 surface.** During this session, 6 Layer 2 clippy warnings were introduced and immediately fixed:
- `redundant_closure` ×2 in `search_candidates` alias-hit branches → replaced `|row| Self::map_candidate_row(row)` with `Self::map_candidate_row` directly.
- `type_complexity` ×2 (the embedding-cohort scan row tuple + `map_candidate_row` return) → factored into `CandidateScanRow` and `CandidateRowProjection` type aliases.
- `field_reassign_with_default` ×2 in fusion.rs validate-rejects tests → rewrote as struct-update syntax.

**Layer 2 acceptance criteria met:**
- ✅ `GraphStore::search_candidates` returns raw multi-signal results (alias / embedding / recency).
- ✅ Cross-feature trait gap closed: `update_entity_embedding` exists; v03-resolution can now write the embedding signal as designed.
- ✅ Alias upsert/resolve is no longer a stub; alias-exact path is fully working.
- ✅ All 27 new tests pass; baseline 1185 → 1213 with 0 regressions.
- ✅ Clippy clean on Layer 2 surface (4 remaining warnings are pre-existing, untouched).

**Deferred to Layer 3 (driver):**
- Wiring `assemble_context → search_candidates → score signals → fuse → decide → upsert/merge`.
- Backfill code path for legacy `graph_entities` rows missing embeddings (Layer 1 record flagged this).
- Integration test exercising end-to-end `store_raw → graph entities + edges` with both alias-matched and embedding-matched resolution paths (Acceptance Criterion #4 — currently unverified end-to-end; only unit-tested per layer).
