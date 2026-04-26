---
id: "ISS-035"
title: "§3.4.4 Edge Decision lookup key inconsistent with match arms"
status: open
priority: P2
created: 2026-04-26
severity: high
---
# ISS-035: §3.4.4 Edge Decision lookup key inconsistent with match arms

**Status:** design-fixed 2026-04-26 (r2 review applied), implementation pending
**Severity:** medium-high — design fix is mechanical, but propagates to v03-graph-layer §4.2 trait signature + the already-shipped `find_edges` implementation (ISS-034) and its tests. Worth doing right; the alternative (Option B) is permanent cross-layer tech debt.

## Design Fix Record (2026-04-26)

After r1 + r2 design review the actual fix is **broader than the original Option A**. Five coupled design changes were applied:

1. **`find_edges` signature** — `object: &EdgeEnd` → `object: Option<&EdgeEnd>` (or `SlotQuery` enum). Slot lookup `(S, P)` and full triple lookup share the trait method but route to different SQL/index paths.
2. **`Cardinality` enum** elevated from advisory-only to **normative write-time input**. EdgeDecision MUST consult it. Added 17-variant mapping table + `Proposed` predicates default to `ManyToMany`. (graph-layer §3.3)
3. **EdgeDecision match block** rewritten to branch by Cardinality first:
   - **Functional / OneToOne** — slot returns 0-1, drives Replace/Update.
   - **Multi-valued / OneToMany / ManyToMany** — slot returns 0-N, deterministic Add/None routing, **never `DeferToLlm`** for the cardinality-known case.
   - **Temporal** — always Add (currently modeled as ManyToMany; future enum variant).
4. **Naming unification** — `superseded_by` → `invalidated_by` everywhere on edges, matching `Edge.invalidated_by` field. (Topic-level `knowledge_topics.superseded_by` is a different domain, left alone.)
5. **`MAX_FIND_EDGES_RESULTS` cap** — removed for slot lookup (whole point is "give me everything for `(S, P)`"); 64-row cap retained for triple lookup (logically 0-1 live edges).

Reviews of record:
- `.gid/features/v03-graph-layer/reviews/design-r2.md` — 3 findings, all applied.
- `.gid/features/v03-resolution/reviews/design-r2.md` — 3 findings, all applied.
**Related:**
- v03-resolution feature (owns §3.4.4 — the contradiction)
- v03-graph-layer feature (owns §4.2 `find_edges` signature)
- ISS-034 (shipped `find_edges` with `object: &EdgeEnd` — the constraint to relax)
**Filed:** 2026-04-26 ~16:25 EDT
**Scope expanded:** 2026-04-26 ~16:35 EDT — ISS-034 actually shipped exact-triple lookup, NOT `Option<Uuid>` as ISS-035 v1 claimed. Fix requires changing the `find_edges` signature in graph-layer + impl + tests, not just the resolution design.

## TL;DR

`v03-resolution/design.md §3.4.4` contains a contradiction between the
`find_edges` lookup and the match arms that consume its result:

```rust
// Lookup filters by (subject, predicate, object) — exact triple match.
let existing = graph_store.find_edges(subject_id, &predicate, &object, /*valid_only=*/ true)?;

// But this arm assumes existing edges may have a DIFFERENT object:
([prior], _) if prior.object != object
                          => EdgeDecision::Update { supersedes: prior.id },
```

The `prior.object != object` arm is unreachable as written, because the
lookup already filtered to only edges with the same object. Either:

- **Option A (recommended):** Lookup should filter by `(subject, predicate)` only
  (i.e. `find_edges(Some(subject), &pred, /*object=*/ None, valid_only=true)`),
  letting the match arms differentiate same-object-conf-bump vs object-change vs
  redundant-restate. This matches the ISS-034 `find_edges` signature
  (`object_id: Option<Uuid>`) which already supports the None case.
- **Option B:** Keep lookup by full triple, drop the `object != object` arm, and
  accept that object changes (`Alice works at Google` → `Alice works at Microsoft`)
  produce parallel live edges rather than supersession. This conflicts with GOAL-2.10
  ("supersession sets invalidated_at + invalidated_by") for the most common kind of
  fact evolution.

## Why Option A

GOAL-2.9 says "Edge ADD / UPDATE / NONE decision with cheap-path short-circuit
before LLM." UPDATE here is meaningful only if it covers the case where the
*same* (subject, predicate) now points to a *different* object — that's the
prototypical fact change. With Option B, fact-change becomes ADD-without-
supersession, which:

- violates GUARD-3 expectations (live graph carries both old and new without
  marking the old as superseded);
- breaks the natural-language intuition of "Alice no longer works at Google";
- forces retrieval to silently de-duplicate at read time rather than letting
  the write path establish ground truth.

GOAL-2.10 explicitly mentions "supersession sets `invalidated_at` +
`invalidated_by` + successor `supersedes`; never deletes." This whole apparatus
is designed for *object change*, not for *confidence bump on identical triple*.
The decision tree must be able to reach Update from a different-object case.

## Concrete Patch Plan (Layer 0)

`v03-resolution/design.md §3.4.4` rewrite of the pseudocode block:

```rust
// Look up by (subject, predicate). object intentionally None — the match arms
// below distinguish object-equal vs object-different.
let existing = graph_store.find_edges(
    Some(subject_id),
    &predicate,
    /*object=*/ None,
    /*valid_only=*/ true,
)?;
let decision = match (existing.as_slice(), new_edge_confidence) {
    // No prior live edge for this (subject, predicate). Add.
    ([], _) => EdgeDecision::Add,

    // Single prior, same object, new confidence higher by margin ε. Bump.
    ([prior], conf)
        if prior.object == new_object && conf > prior.confidence + EPS
        => EdgeDecision::Update { supersedes: prior.id },

    // Single prior, same object, no meaningful confidence change. Redundant.
    ([prior], _) if equals_within_tolerance(prior, &new_triple)
        => EdgeDecision::None,

    // Single prior, DIFFERENT object. Fact has changed; supersede.
    // (This is the case the original draft tried to handle but couldn't reach.)
    ([prior], _) if prior.object != new_object
        => EdgeDecision::Update { supersedes: prior.id },

    // Defensive catch-all (shouldn't reach if arms above are exhaustive,
    // but new_object equality check is cheap).
    ([_prior], _) => EdgeDecision::None,

    // Multiple prior live edges on (subject, predicate) — branching graph
    // state; cheap signals can't disambiguate. Defer to LLM (mem0 prompt,
    // master §4.4).
    (_many, _) => EdgeDecision::DeferToLlm,
};
```

Cross-references:
- `v03-graph-layer/design.md §4.2 find_edges` — already specifies
  `object_id: Option<Uuid>`, so no Layer 1 schema/index work needed beyond
  what ISS-034 delivered.
- `v03-resolution/design.md §3.5` — Persist section already covers
  `Update { supersedes }` correctly; no changes needed there.

## Out of Scope

- Multi-prior-live disambiguation logic (DeferToLlm path). Master §4.4 owns
  the LLM tie-breaker; this issue only ensures the cheap path can route to it.
- Literal-object edges. The `EdgeEnd::Literal` case still uses `find_edges`
  with `object_id = None` (literals don't have entity ids); decision logic is
  identical.
- The `equals_within_tolerance` helper. To be specified during Layer 2
  implementation; conservative default = `prior.predicate == new.predicate &&
  prior.object == new.object && (new.conf - prior.conf).abs() < EPS`.

## Acceptance

- `v03-resolution/design.md §3.4.4` updated with the patched pseudocode.
- Match arms exhaustive: every `(existing, conf)` shape has exactly one
  matching arm.
- design review (review-design at standard depth) clean on the patched section.
- Ready for Layer 2: `crates/engramai/src/resolution/edge_decision.rs` can be
  written without further design questions.

## Process Note

Surfaced before any Layer 2 code was written, by reading the design and
running it through the match-arm reachability check (no exec, just a mental
trace). Filing as a separate issue rather than silently fixing in code,
because:

1. potato's rule: 设计与代码必须一致。Code-fixing a design contradiction makes
   the design a lie.
2. Two reasonable readings exist (Option A vs B); committing one in code
   without surfacing the choice would be a unilateral decision on a question
   that belongs to the design reviewer.
3. Pattern matches ISS-033 / ISS-034 — cross-design gaps caught pre-impl,
   patched in design, reviewed, then implemented. Consistent ritual.

## Implementation Plan (post-r2)

Layer 2 work, broken into 5 sequential tasks (in graph as `iss-035-impl-*`):

1. **iss-035-impl-cardinality** — `graph/schema.rs`: add `Cardinality` enum + `CanonicalPredicate::cardinality()` method with 17-variant table. Unit tests cover all variants + Proposed default.
2. **iss-035-impl-trait-signature** — `graph/store.rs`: change `find_edges` trait signature to accept `object: Option<&EdgeEnd>`. Update doc comments (slot vs triple semantics, index choice per mode, cap behavior per mode).
3. **iss-035-impl-sql-branch** — `graph/storage_graph.rs`: SQL implementation branches on `object.is_some()`. Slot mode uses `idx_graph_edges_live` (or `_spo` if first), no cap; triple mode keeps existing path with 64-row cap.
4. **iss-035-impl-test-migration** — Update all 15 ISS-034 tests in `store.rs` (lines 4034-4191) from `&object` to `Some(&object)`. Add new tests for slot lookup mode: empty result, multi-result (functional case has 0-1, multi-valued has 0-N), valid_only filter, no cap behavior.
5. **iss-035-impl-edge-decision** — New file `resolution/edge_decision.rs`: `compute_edge_decision()` function with three-cardinality match block. Unit tests cover all branches + boundary cases (confidence epsilon, equals_within_tolerance helper).

Verification: `cargo test -p engramai` clean. No new clippy warnings.

## Implementation Record (2026-04-26)

All 5 Layer 2 tasks completed in single session. Status: **DONE**.

| Task | What was done | Tests |
|---|---|---|
| 2.1 cardinality | `Cardinality` enum + `cardinality()` free fn + `Predicate::cardinality()` method, 17-variant table verbatim from design §3.3 | 6 new (covers_all, OneToOne/OneToMany/ManyToMany variants, dispatch, proposed default, serde) |
| 2.2 trait signature | `GraphStore::find_edges` `object: &EdgeEnd` → `Option<&EdgeEnd>` with dual-mode docstring | (covered by 2.4) |
| 2.3 sql branch | `SqliteGraphStore::find_edges` mode-branched: slot uncapped via `idx_graph_edges_live`, triple keeps 64-cap via `idx_graph_edges_spo` | (covered by 2.4) |
| 2.4 test migration | 11 existing tests migrated to `Some(&object)`; 5 new slot-mode tests | 5 new (slot returns all, valid_only filter, empty, uncapped above MAX, namespace isolation) |
| 2.5 edge_decision | New file `resolution/edge_decision.rs`: pure `compute_edge_decision()` with three-cardinality branching | 15 new (Functional 5 cases + Multi-valued 5 cases + Proposed default + literal-object change + serde + 1×OneToMany routes via multi) |

**Final test count:** 1314 lib tests pass (was 1294 before ISS-035; net +20). 0 new clippy warnings. All 6 r2 review findings verified against implemented code.

**Key root-fix outcome:** the ISS-035 functional-fact-change scenario ("Alice WorksAt Acme" → "Alice WorksAt Beta") is now caught by `compute_edge_decision` and routed to `Update { supersedes }` instead of producing parallel live edges. Test `functional_different_object_returns_update` is the canonical regression guard.
