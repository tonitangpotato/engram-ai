---
title: "Fix plan for ISS-075 + ISS-076 — unify CreateNew entity UUID + write alias/embedding"
status: draft
related: [ISS-075, ISS-076]
author: rustclaw
date: 2026-04-30
---

# Fix plan: ISS-075 + ISS-076

## Why one plan

These two issues are tangled at the root and have to be fixed together:

- **ISS-075** (alias + embedding never written → `search_candidates` returns empty → 100% `CreateNew`) makes every entity take the `CreateNew` branch.
- **ISS-076** (edge subject/object UUIDs never match persisted entity UUIDs) makes every `CreateNew` entity dangling in the edge table.

Fixing only ISS-075 would let some entities take `MergeInto` (UUID comes from the existing candidate row, edges become resolvable for those), but every entity that still goes through `CreateNew` (which will be most of them on cold caches and on truly novel mentions) stays dangling. Fixing only ISS-076 would unify UUIDs across the persist + edge paths, but every entity would still be a duplicate `CreateNew` because alias lookup never finds anything.

We need a complete graph after one fix landing window, so both go in together.

## Verified root cause (cite-before-claim)

- `crates/engramai/src/resolution/pipeline.rs` `resolve_edges` (around line 643–645) calls `Uuid::new_v4()` to mint a fresh `subject_id` / `object_id` for every `CreateNew` draft and writes it into the edge cache.
- `crates/engramai/src/resolution/stage_persist.rs` `build_new_entity` (line 417) calls `Entity::new(draft.canonical_name, draft.kind, now)`.
- `crates/engramai/src/graph/entity.rs` `Entity::new` (line 149) sets `id: Uuid::new_v4()`.
- These two `Uuid::new_v4()` calls are independent random UUIDs. The TODO comment in `pipeline.rs` (lines 620–637) claiming "they happen to be safe because edge inserts in build_delta re-resolve subject/object names against the same decision slice" is wrong — `build_new_entity` does **not** re-resolve names, it only consumes `draft.canonical_name` and mints a brand new id.
- `stage_extract.rs` does not call `upsert_alias` and does not generate embeddings; `stage_persist.rs::build_new_entity` does not write an alias row or populate `embedding`. `search_candidates` depends on both, so it always returns empty → resolution always falls through to `CreateNew`.

## The fix, in two phases

### Phase A — ISS-076: unify the UUID (must land first)

**Goal:** the UUID written into `edge.subject_id` / `edge.object_id` is the same UUID used as `Entity.id` for that draft, regardless of `CreateNew` vs `MergeInto`.

**Strategy:** mint the canonical entity id exactly once, on the `EntityResolution`, and have both `resolve_edges` and `build_new_entity` read from that.

**Concretely:**
1. Add a field `EntityResolution::resolved_id: Uuid` (always populated, regardless of `Decision`). Computed when the `EntityResolution` is built:
   - `Decision::CreateNew` → fresh `Uuid::new_v4()`, stored on `resolved_id`.
   - `Decision::MergeInto { candidate_id }` → `resolved_id = *candidate_id`.
2. `resolve_edges` switches from `Uuid::new_v4()` for CreateNew subjects/objects to `entity_resolution.resolved_id` (looked up by draft index).
3. `build_new_entity` takes the resolved id as a parameter and writes it into `Entity { id: resolved_id, .. }` instead of letting `Entity::new` mint one. Either:
   - (a) Add `Entity::new_with_id(id, canonical_name, kind, now)` and call it from `build_new_entity`, or
   - (b) Mutate `entity.id = resolved_id` after `Entity::new`.
   Prefer (a) — explicit constructor, no "mutate immediately after construct" smell.
4. Delete the wrong TODO comment block at `pipeline.rs:620–637` and replace with a one-line invariant: "`EntityResolution::resolved_id` is the single source of truth for canonical id; both edge subjects/objects and the persisted entity row reference it."

**Tests for Phase A:**
- Unit: given a draft slice with N CreateNew + M MergeInto, after `build_delta` every `edge.subject_id` / `edge.object_id` exists in `delta.entities[*].id` ∪ `existing canonical ids`. Zero dangling.
- Property test: round-trip `(drafts, edge_drafts) → resolve → persist → query graph_edges JOIN entities` returns 100% match.

### Phase B — ISS-075: write alias rows + embeddings on CreateNew

**Goal:** future runs can find the entity via `search_candidates` and take the `MergeInto` path instead of duplicating.

**Concretely:**
1. **Embedding (extract stage):** `stage_extract.rs` should compute an embedding for each `DraftEntity` (mention-text-based, same model the resolver uses for similarity). Store on `DraftEntity::embedding: Option<Vec<f32>>` so `build_new_entity` can persist it on `Entity::embedding`.
2. **Alias upsert (persist stage):** in the `Decision::CreateNew` branch of `build_delta`, after pushing `new_entity` into `delta.entities`, also push an alias row keyed `(canonical_name_normalized, entity_id=resolved_id)` into `delta.aliases` (or whatever the existing alias delta channel is — verify). For `Decision::MergeInto`, no new alias is needed unless the surface form differs from the candidate's existing aliases (NFKC-normalized comparison).
3. Apply NFKC normalization on the alias key (already done in `upsert_alias` per ISS-033 Layer 2 — confirm reuse).
4. Verify `apply_graph_delta` plumbing actually persists the alias rows and writes `entity.embedding` to the DB column. Add coverage if missing.

**Tests for Phase B:**
- Unit: ingest two episodes mentioning "Caroline" with a 1-token apart paraphrase. After episode 1 alias row exists; episode 2's `search_candidates` returns the entity from episode 1; episode 2's `EntityResolution` is `MergeInto`; final `entities` table has exactly 1 Caroline.
- The "27 duplicate Carolines" regression test from the cogmembench fixture should drop to 1.

### Order matters
- Phase A first, alone, with its tests landed and green.
- Then Phase B on top. Phase B's `MergeInto`-path tests rely on Phase A having unified ids (otherwise even successful merges would still write dangling edges from any concurrent CreateNew in the same slice).

## Risk + verification

- **Backwards compat:** existing DBs have dangling edges. After Phase A lands, new ingestion writes correct edges, but old data stays bad. Decide: write a one-shot repair migration that re-resolves dangling `graph_edges` against `entities.canonical_name`, or wipe + re-ingest the cogmembench fixture. **Recommend: wipe + re-ingest** — old data is small, repair migration is hard to verify, and we want a clean baseline for spreading-activation eval.
- **Spreading activation eval:** only run hit@k after both phases land + cogmembench is re-ingested. Anything before that is noise (graph is broken, regardless of algorithm).
- **Regression watchpoint:** `kind_source` provenance writing (ISS-072 GOAL-2) currently happens only in `build_new_entity`. If we change that function's signature, double-check the provenance write still triggers for every CreateNew. Add a unit test asserting `attributes["kind_source"]` is set.

## Open questions for potato before I start

1. Is **Entity::new_with_id** the right shape, or do you want a different API (e.g., `Entity::new` takes id, callers pass `Uuid::new_v4()` explicitly)? Personally I prefer the new constructor — keeps the implicit-id path for one-off tests where id doesn't matter.
2. For Phase B, should the embedding be computed in `stage_extract` synchronously (blocks ingestion on the embedding model) or deferred to a post-persist step (faster ingestion, but `search_candidates` for the very next episode in the same batch won't find it)? My read: synchronous, because batch ingestion of fixtures is the use case where we most need `MergeInto` to fire on episode N+1.
3. Wipe + re-ingest cogmembench fixture, confirmed?

Once you say go on these three, I'll start with Phase A.
