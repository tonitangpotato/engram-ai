# Design Review r2 — v03-resolution

> **Reviewer:** sub-agent (review-design skill)
> **Date:** 2026-04-26
> **Target:** `.gid/features/v03-resolution/design.md`
> **Requirements:** `.gid/features/v03-resolution/requirements.md`
> **Method:** 27-check review-design skill, depth=standard
> **Focus:** ISS-035 changes — §3.4.4 Edge Decision slot lookup + supersession chain semantics

## Summary

| Severity   | Count |
|------------|-------|
| Critical   | 0     |
| Important  | 2     |
| Minor      | 1     |
| **Total**  | 3     |

**Recommendation:** needs fixes before implementation — FINDING-1 (multi-valued predicates misrouted to DeferToLlm) blocks correct behavior for `OneToMany` / `ManyToMany` predicates. FINDING-2 (naming mismatch) causes implementer confusion. FINDING-3 is minor but should be documented.

### ✅ Passed Checks (ISS-035 scope)

- **Check #0:** Document size ✅ — 10 sections, within bounds.
- **Check #1:** Types fully defined ✅ — `EdgeDecision` variants (Add, Update, None, DeferToLlm) are used exhaustively. `find_edges` call matches graph-layer §4.2 signature.
- **Check #2:** References resolve ✅ — ISS-035, graph-layer §4.2, `invalidate_edge`, `idx_graph_edges_spo` all verified.
- **Check #3:** No dead definitions ✅ — every match arm produces a decision variant consumed by §3.5.
- **Check #5:** Logic correctness — ISS-035 original bug (unreachable `prior.object != object` arm) is fixed ✅. Slot lookup correctly exposes the full slot state. Path traces: happy path (slot empty → Add ✅), confidence bump (same object, higher conf → Update ✅), redundant (same object, same conf → None ✅), fact change (different object → Update ✅), branched state (multiple priors → DeferToLlm ✅ for functional predicates). Multi-valued case flagged in FINDING-1.
- **Check #6:** Data flow ✅ — `existing` is produced by `find_edges` and consumed by match arms. `new_object`, `new_edge_confidence`, `new_triple` are pseudocode locals from the enclosing `for each Triple` loop.
- **Check #7:** Error handling ✅ — `?` propagates `GraphError` from `find_edges`.
- **Check #8–10:** Type safety ✅ — no string slicing, no integer overflow, no unwrap.
- **Check #11:** Match exhaustiveness ✅ — `[]`, `[prior]` (4 sub-arms with catch-all), `_many` covers all slice shapes.
- **Check #12:** Ordering sensitivity ✅ — `[prior]` arms partition cleanly: arms 1+2 require `object ==`, arm 3 requires `object !=`, arm 4 is catch-all. Reordering arms 1-3 would change behavior (arm 1 must precede arm 2 since both require `object ==`), but the order is correct (confidence-increase check before tolerance check).
- **Check #13:** Separation of concerns ✅ — `find_edges` is a read; match is pure decision logic; §3.5 persist is the side-effect layer.
- **Check #15:** Configuration ✅ — `EPS` is named (value TBD at implementation). `valid_only = true` is explicit.
- **Check #16:** API surface ✅ — `EdgeDecision` is the minimal output type.
- **Check #17–18:** Goals + trade-offs ✅ — "Why slot lookup, not triple lookup" paragraph clearly documents the ISS-035 rationale.
- **Check #19:** Cross-cutting concerns ✅ — literal-object handling mentioned ("literals don't have entity ids; decision logic is identical" per ISS-035 issue).
- **Check #20:** Abstraction level ✅ — pseudocode is concrete enough to implement directly.
- **Check #30:** No technical debt ✅ — no "temporary" or "good enough for now" language.
- **Check #31:** Root fix ✅ — changes the lookup key (root cause), not the match arms (symptom).
- **Check #32:** Architecture conflict — addressed in FINDING-2 (naming mismatch with Edge struct fields).
- **Check #34:** Breaking change risk ✅ — §3.4.4 is a new section (no prior callers); §3.5 persist correctly handles the new decision shape.
- **Check #35:** Purpose alignment ✅ — every match arm serves a stated GOAL (2.9, 2.10, 2.12).

---

## FINDING-1 🟡 Important — Multi-valued predicates misrouted to DeferToLlm by slot lookup

**Check #5 (Logic Correctness — state machine invariants), #33 (Simplification vs completeness)**
**Location:** §3.4.4, match arm `(_many, _) => EdgeDecision::DeferToLlm`
**Cross-ref:** v03-graph-layer §3.3 `Cardinality` enum (`OneToOne`, `OneToMany`, `ManyToMany`)

**Problem.** The slot lookup `find_edges(subject_id, &predicate, None, true)` returns ALL live edges for a `(subject, predicate)` pair. For **functional** predicates (e.g. `lives_in`, `works_at` — `Cardinality::OneToOne`), at most one live edge is expected per slot, so the `[prior]` arms handle the common case and `(_many, _)` is a genuine anomaly worth deferring to LLM.

But for **multi-valued** predicates (e.g. `knows`, `likes` — `Cardinality::OneToMany` or `ManyToMany`), having multiple live edges in the same `(subject, predicate)` slot is the **normal** state. "Alice knows Bob" and "Alice knows Carol" both occupy the `(Alice, knows)` slot simultaneously. Every new `knows` triple will hit the `(_many, _)` arm and route to `DeferToLlm`, burning an LLM call on what should be a cheap Add.

The decision logic implicitly assumes all predicates are functional (at most one live object per subject-predicate slot). This is the exact simplification that ISS-035 was trying to fix — it got the *lookup* right (slot, not triple) but the *match arms* still only handle the functional case correctly.

**Impact:**
- GOAL-2.9 violation: "Edge ADD / UPDATE / NONE decision with **cheap-path short-circuit before LLM**" — multi-valued predicates never get the cheap path.
- GOAL-2.14 / GUARD-12 violation risk: every multi-valued predicate assertion triggers an LLM call, blowing the avg ≤ 3 budget on any graph with relational data.
- `Cardinality` enum in graph-layer §3.3 exists but is never consulted by the decision logic.

**Suggested fix:** The match arms should consult `PredicateCatalog::cardinality(&predicate)` before routing. Sketch:

```rust
let cardinality = catalog.cardinality(&predicate);
let existing = graph_store.find_edges(subject_id, &predicate, None, true)?;

let decision = match (cardinality, existing.as_slice(), new_edge_confidence) {
    // === Functional predicates (OneToOne): existing arms are correct ===
    (Cardinality::OneToOne, [], _) => EdgeDecision::Add,
    (Cardinality::OneToOne, [prior], conf)
        if prior.object == new_object && conf > prior.confidence + EPS
        => EdgeDecision::Update { supersedes: prior.id },
    (Cardinality::OneToOne, [prior], _)
        if equals_within_tolerance(prior, &new_triple)
        => EdgeDecision::None,
    (Cardinality::OneToOne, [prior], _)
        if prior.object != new_object
        => EdgeDecision::Update { supersedes: prior.id },
    (Cardinality::OneToOne, [_prior], _) => EdgeDecision::None,
    (Cardinality::OneToOne, _many, _) => EdgeDecision::DeferToLlm,

    // === Multi-valued predicates (OneToMany, ManyToMany) ===
    // Multiple live edges are normal. Check if this exact triple exists.
    (_, existing, _) => {
        match existing.iter().find(|e| e.object == new_object) {
            Some(prior) if equals_within_tolerance(prior, &new_triple)
                => EdgeDecision::None,
            Some(prior) if new_edge_confidence > prior.confidence + EPS
                => EdgeDecision::Update { supersedes: prior.id },
            Some(_) => EdgeDecision::None,
            None => EdgeDecision::Add,  // new object for this predicate — normal
        }
    }
};
```

For `Proposed` predicates (not in catalog), default to multi-valued (safer: Add is cheaper than DeferToLlm, and proposed predicates have unknown semantics).

---

## FINDING-2 🟡 Important — §4.2 legend uses `superseded_by` but Edge struct field is `invalidated_by`

**Check #4 (Consistent naming)**
**Location:** §4.2 re-extraction table + §4.2 Legend paragraph
**Cross-ref:** v03-graph-layer §3.2 `Edge` struct

**Problem.** The resolution §4.2 table row says:

> "emit a new edge row with `supersedes = prior.id`; set `prior.superseded_by = new.id`"

And the Legend says:

> "Supersede creates a new edge row with `supersedes = prior_edge.id` and sets `prior_edge.superseded_by = new_edge.id`."

But the Edge struct in graph-layer §3.2 has:
- `invalidated_by: Option<Uuid>` — on the prior, pointing to successor
- `supersedes: Option<Uuid>` — on the successor, pointing to prior

There is no `superseded_by` field on Edge. The correct field name for "prior points to successor" is `invalidated_by`. This inconsistency could cause an implementer to look for a nonexistent field, or to create a `superseded_by` alias that diverges from the canonical schema.

**Suggested fix:** Replace `prior.superseded_by = new.id` with `prior.invalidated_by = new.id` (and correspondingly `prior.invalidated_at = now`) in §4.2 table and legend. The persist section §3.5 already uses the correct API: `GraphStore::invalidate_edge(prior_id, new.id, now)`.

---

## FINDING-3 🟢 Minor — `MAX_FIND_EDGES_RESULTS = 64` cap may cause false Adds on high-fanout multi-valued predicates

**Check #33 (Simplification vs completeness)**
**Location:** v03-graph-layer §4.2 `find_edges` output cap

**Problem.** The `find_edges` docstring says:

> "Hard-capped at MAX_FIND_EDGES_RESULTS = 64 rows... §3.4.4 only ever inspects the head of the list."

This is true for the current match arms (which only handle `[]`, `[prior]`, and `_many`). But if the multi-valued predicate fix (FINDING-1) is adopted, the resolution logic would scan `existing.iter().find(|e| e.object == new_object)` across all returned edges. For a subject with >64 live edges on a multi-valued predicate (e.g., a person entity with 100+ `knows` edges), the cap truncates the result set and `find()` may miss the existing edge, producing a false `EdgeDecision::Add` (duplicate edge).

**Impact:** Low for v0.3 MVP (few subjects will have >64 live edges per predicate at launch). Higher risk as the graph grows.

**Suggested fix:** If FINDING-1 is adopted, either (a) increase the cap for multi-valued predicates, (b) switch to a triple lookup `find_edges(subject, predicate, Some(new_object), true)` as a secondary check after the slot lookup (belt-and-suspenders: slot lookup for routing, triple lookup for dedup), or (c) document the limitation explicitly and add a telemetry counter for truncated results so operators can detect the issue.

---

<!-- FINDINGS -->

## Applied

(None — awaiting human approval before apply phase.)
