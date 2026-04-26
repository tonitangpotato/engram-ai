# ISS-034: GraphStore trait missing `find_edges` + `invalidate_edge` (edge-side counterpart of ISS-033)

**Status:** resolved (2026-04-26, commits da4b89d + db77f0c)
**Severity:** high — blocks v03-resolution §3.4.4 (Edge Resolution) and §3.5 (Persist) driver, which blocks the end-to-end `store_raw → extraction → resolution → graph write` pipeline
**Related:**
- v03-graph-layer (feature) — owns `GraphStore` trait
- v03-resolution (feature) — caller in §3.4.4 / §3.5
- ISS-033 — entity-side counterpart (`embedding` column + `search_candidates`). This issue is its edge-side twin.
**Filed:** 2026-04-26
**Prerequisite:** None.

## TL;DR

`v03-resolution/design.md §3.4.4 (Edge Resolution)` and `§3.5 (Persist)` reference two `GraphStore` methods that **do not exist on the trait at all** (not even as `unimplemented!()` stubs):

1. `find_edges(subject_id, predicate, object_id, valid_only) -> Vec<Edge>` — used per-triple to look up prior edges with the same (S, P, O) so the resolver can decide CREATE vs SUPERSEDE vs NOOP.
2. `invalidate_edge(prior_id, superseded_by_new_id, now)` — used in the SUPERSEDE branch to close out a prior edge's `valid_to` and link it to its successor.

Without these the Layer 3 resolution driver cannot wire up §3.4.4 / §3.5 — every edge resolution decision and every persist call would either stub out a critical step or silently NOOP. Attempting an MVP without them would commit the same kind of cross-feature design/code drift that ISS-033 is fixing on the entity side.

## Symptom Found

Discovered while preparing to write the Layer 3 resolution driver (post-ISS-033, post-Layer-0 graph foundation). Audit of `crates/engramai/src/graph/store.rs` against `v03-resolution/design.md §3.4.4 + §3.5`:

| Method (referenced in design) | trait status |
|---|---|
| `find_edges(subj, pred, obj, valid_only)` | ❌ not in trait |
| `invalidate_edge(prior_id, new_id, now)` | ❌ not in trait |
| `supersede_edge` | trait sig present, body `unimplemented!()` |
| `link_memory_to_entities` | trait sig present, body `unimplemented!()` |
| `entities_linked_to_memory` | trait sig present, body `unimplemented!()` |
| `record_resolution_trace` | trait sig present, body `unimplemented!()` |
| `begin_pipeline_run` / `finish_pipeline_run` | trait sig present, body `unimplemented!()` |
| `record_extraction_failure` | trait sig present, body `unimplemented!()` |

The first two are the cross-design drift this issue is about. The remainder are tracked Phase 2/3 work (already in `v03-graph-layer-execution-plan.md` Phase 2/3) and will be fixed there.

## Why Not MVP

Same family of reasoning as ISS-033:

- **Design drift**: §3.4.4 specifies the SUPERSEDE / CREATE / NOOP decision tree. Without `find_edges` the driver can't even ask the question. Stubbing it as "always CREATE" would silently disable supersession — an invisible correctness regression.
- **Bi-temporal invariant violation**: the whole point of the bi-temporal edge model is that `valid_to` gets closed out when a successor edge supersedes a prior. Without `invalidate_edge` no edge ever gets closed → graph accumulates contradictions forever. This is a P0 invariant in v03-graph-layer §1.
- **Design must match code**: trait is the contract. If the trait is missing methods the design depends on, the design is a lie. Future readers can't trust the doc.
- **No second migration**: putting these methods on the trait now (when there are zero downstream consumers besides the upcoming driver) is free. Adding them later means breaking every implementer.

## Scope (3 layers)

### Layer 0: Cross-Feature Design Patch

Goal: make `v03-graph-layer/design.md` complete before any code is written.

- [ ] `v03-graph-layer/design.md §4.2` (`GraphStore` trait): add `find_edges`. Specify:
  - Input: `subject_id: Option<Uuid>`, `predicate: &str` (canonical predicate id), `object_id: Option<Uuid>` (NULL = literal-object edge — design TBD whether literal lookup is in scope), `valid_only: bool` (true → only edges with `valid_to IS NULL`).
  - Output: `Vec<Edge>` (full hydration, same as `get_edge`). Ordered by `valid_from DESC`.
  - Index requirement: `(subject_id, predicate, object_id, valid_to)` index on `graph_edges` so the lookup is O(log n) per triple. Spec out the index in §4.1 schema.
  - Bound output (e.g., hard cap 64) — caller (`§3.4.4`) only needs the most recent few; cap protects against pathological cases.
- [ ] `v03-graph-layer/design.md §4.2`: add `invalidate_edge`. Specify:
  - Input: `prior_id: Uuid`, `superseded_by: Uuid` (the new edge id), `now: DateTime<Utc>` (caller-supplied for testability).
  - Behavior: sets `prior.valid_to = now`, sets `prior.superseded_by = Some(superseded_by)`. Returns error if `prior.valid_to` is already set (double-supersede is a bug; caller must check or use `find_edges(valid_only=true)`).
  - Idempotence note: if `prior.superseded_by == Some(superseded_by)` already, return Ok (no-op) — supports retry semantics.
  - Atomicity: must be a single SQL UPDATE so concurrent calls can't double-close.
- [ ] `v03-resolution/design.md §3.4.4`: tighten reference to the new §4.2 methods. Make explicit which signal the driver calls (`find_edges(valid_only=true)` first; falls through to `find_edges(valid_only=false)` only if it needs historical context for trace).
- [ ] `v03-resolution/design.md §3.5`: tighten reference to `invalidate_edge` (currently prose-only).
- [ ] Cross-reference tables updated.
- [ ] **Design review** (review-design skill, standard depth — same scope as ISS-033 r3) runs against the patched sections.

### Layer 1: Schema (index)

- [ ] Add migration: composite index `idx_graph_edges_spo_valid` on `graph_edges(subject_id, predicate, object_id, valid_to)`.
- [ ] Verify: `EXPLAIN QUERY PLAN` for the §3.4.4 lookup uses the index.

### Layer 2: Trait + Implementation

- [ ] Add `find_edges` to `GraphStore` trait. Implement on `SqliteGraphStore` using the new index.
- [ ] Add `invalidate_edge` to `GraphStore` trait. Implement on `SqliteGraphStore` with single UPDATE; honor the "already-superseded-by-same-id is no-op" idempotence.
- [ ] Decide: does `invalidate_edge` subsume the existing-but-unimplemented `supersede_edge`, or is `supersede_edge` a higher-level wrapper that combines insert-new + invalidate-prior in one call? Pick one shape, document it.
- [ ] Tests:
  - `find_edges` happy path: insert 3 edges, find by full triple, find by partial triple (subject only), valid_only filter behavior.
  - `find_edges` index-coverage: assert via `EXPLAIN QUERY PLAN`.
  - `find_edges` cap: insert 100 edges with same triple, assert ≤ cap returned.
  - `invalidate_edge` happy path: invalidate, re-fetch, assert `valid_to + superseded_by` set.
  - `invalidate_edge` idempotence: call twice with same args, second is no-op.
  - `invalidate_edge` already-closed-by-different-id: returns error.
  - Concurrent invalidate: two threads call invalidate_edge on same prior with different new_ids → exactly one wins, other errors. (Best-effort under sqlite locking; document semantics if not deterministic.)

## Out of Scope

- `supersede_edge` body (Phase 3 in execution plan, separate work)
- `merge_entities` (Phase 3)
- `edges_as_of` / `traverse` (Phase 3, not driver-critical)
- Literal-object edge lookup semantics (deferred — initial driver only deals with entity-object edges; literal edges are §3.3 future work)

## Acceptance

- All Layer 0 design changes review-clean (review-design at standard depth, no critical/important findings).
- `cargo test -p engramai` green with new tests.
- `EXPLAIN QUERY PLAN` shows index usage for §3.4.4 lookup.
- v03-resolution Layer 3 driver can call both methods without stubs (no `unimplemented!()` left on these two).

## Process Note (vs ISS-033)

ISS-033 went through the full ritual (design patch + review + 3-layer impl). ISS-034 is structurally identical (same kind of cross-design gap, similar layer count). **Recommend same ritual treatment** — design patch + review + impl — to avoid skipping the design step and recreating the original gap. potato's rule: 不要 mvp，要 root fix.

## Filed By

Surfaced during pre-driver audit, 2026-04-26 ~14:50 EDT, before writing any Layer 3 driver code. Filed before any implementation is attempted (per the same discipline that produced ISS-033).
