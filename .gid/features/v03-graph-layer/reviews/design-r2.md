# Design Review r2 — v03-graph-layer

> **Reviewer:** sub-agent (review-design skill)
> **Date:** 2026-04-26
> **Target:** `.gid/features/v03-graph-layer/design.md`
> **Requirements:** `.gid/features/v03-graph-layer/requirements.md`
> **Method:** 27-check review-design skill, depth=standard
> **Focus:** ISS-035 changes — `find_edges` signature change to slot/triple dual-mode lookup (§4.2)

## Summary

| Severity   | Count |
|------------|-------|
| Critical   | 0     |
| Important  | 2     |
| Minor      | 1     |
| **Total**  | 3     |

**Recommendation:** needs fixes before implementation — FINDING-1 (cardinality advisory-vs-write-time contradiction) and FINDING-3 (naming mismatch) should be resolved before implementing `find_edges` and `EdgeInvalidation`.

### ✅ Passed Checks (ISS-035 scope)

- **Check #0:** Document size ✅ — 11 sections, ≤8 major components.
- **Check #1:** Types fully defined ✅ — `find_edges` signature uses `Option<&EdgeEnd>`, `Uuid`, `Predicate`, `Edge`, `GraphError` — all defined in §3.2, §3.3, §7.
- **Check #2:** References resolve ✅ — ISS-034, ISS-035, resolution §3.4.4, `idx_graph_edges_spo` all exist and are correct.
- **Check #3:** No dead definitions ✅ — no new types introduced; `Option<&EdgeEnd>` is consumed by resolution §3.4.4.
- **Check #5:** Logic correctness ✅ — dual-mode lookup (slot vs triple) correctly uses SQLite leftmost-prefix index rule. Both modes documented with clear semantics.
- **Check #6:** Data flow complete ✅ — every `find_edges` parameter has a clear source; every output field is consumed by §3.4.4 match arms.
- **Check #7:** Error handling ✅ — `Result<Vec<Edge>, GraphError>` with `NotFound`, `Invariant` error paths documented.
- **Check #8–12:** Type safety ✅ — no string slicing, no integer overflow, no unwrap, match is exhaustive via `Option<&EdgeEnd>`.
- **Check #13:** Separation of concerns ✅ — `find_edges` is a pure read; mutation is separated into `invalidate_edge` / `supersede_edge`.
- **Check #14:** Coupling addressed in FINDING-1 (cardinality advisory claim contradicts write-time need).
- **Check #15:** Configuration ✅ — `MAX_FIND_EDGES_RESULTS = 64` is a named constant, not a magic number. `valid_only` is a parameter.
- **Check #16:** API surface ✅ — `Option<&EdgeEnd>` is the minimal extension; no internal details leak.
- **Check #17:** Goals explicit ✅ — ISS-035 rationale documented in §4.1 index comments and §4.2 docstring.
- **Check #18:** Trade-offs documented ✅ — ISS-035 issue doc covers Option A vs B explicitly.
- **Check #19:** Cross-cutting concerns ✅ — namespace scoping, literal-object handling, concurrency all addressed.
- **Check #20:** Abstraction level ✅ — signature + docstring + SQL index is the right level for a storage trait.
- **Check #30:** No technical debt ✅ — no "temporary" language, no workarounds.
- **Check #31:** Root fix ✅ — ISS-035 addresses root cause (lookup key too specific), not symptom.
- **Check #32:** Addressed in FINDING-2 (naming inconsistency).
- **Check #33:** Simplification concern noted — cardinality advisory-only claim under-serves the write path (FINDING-1).
- **Check #34:** Breaking change risk ✅ — `Option<&EdgeEnd>` is additive; existing `Some(object)` callers unaffected.
- **Check #35:** Purpose alignment ✅ — every component serves `find_edges` dual-mode need.

---

## FINDING-1 🟡 Important — `Cardinality` documented as advisory-only, but ISS-035 fix demands write-time consultation

**Check #14 (Coupling), #32 (Conflicts with existing architecture)**
**Location:** §3.3 `Cardinality` enum doc comment + `PredicateCatalog::cardinality()`

**Problem.** The `Cardinality` enum's doc comment says:

> "Cardinality hint — purely advisory; not enforced at write time. Used by consolidation/audit (out of scope for this feature) to flag suspicious fan-out."

But the ISS-035 fix to resolution §3.4.4 uses slot lookup `find_edges(subject_id, &predicate, None, true)` which returns ALL live edges for a `(subject, predicate)` pair. The decision match arms then route `(_many, _) => EdgeDecision::DeferToLlm`. For multi-valued predicates (`Cardinality::OneToMany` or `ManyToMany` — e.g., a proposed `"knows"` predicate), multiple live edges per slot is the normal state, not branched state. The resolution pipeline **needs** cardinality at write time to correctly route these cases (see v03-resolution review FINDING-1).

The graph-layer doc creates an explicit expectation that `Cardinality` is NOT used at write time, while the resolution pipeline's correctness depends on it being available and accurate at write time. This is an inter-document contradiction.

**Impact:**
- Implementers of `PredicateCatalog` may treat `cardinality()` as low-priority (it's "advisory"), but resolution correctness depends on it.
- The cardinality mapping for each `CanonicalPredicate` is unspecified — `cardinality()` body is `/* ... */`. An implementer cannot know which predicates are functional vs multi-valued without a table.

**Suggested fix:**
1. Remove or amend "purely advisory; not enforced at write time" — it is consumed by the write path.
2. Add a cardinality mapping table for all `CanonicalPredicate` variants (e.g., `WorksAt → OneToOne`, `ParentOf → OneToMany`, `MarriedTo → OneToOne`, etc.).
3. Specify the default cardinality for `Proposed` predicates (suggested: `OneToMany` — safer to assume multi-valued than functional for unknown predicates).

---

## FINDING-2 🟢 Minor — `find_edges` docstring claims `idx_graph_edges_spo` for both modes, but `idx_graph_edges_live` may be preferred for slot lookup

**Check #2 (References resolve), #4 (Consistent naming)**
**Location:** §4.2 `find_edges` docstring, "Index" paragraph + §4.1 index definitions

**Problem.** The `find_edges` docstring states:

> "EXPLAIN QUERY PLAN for both modes must show SEARCH ... USING INDEX idx_graph_edges_spo; asserted in integration tests."

But §4.1 also defines:

```sql
CREATE INDEX IF NOT EXISTS idx_graph_edges_live
    ON graph_edges(subject_id, predicate_label) WHERE invalidated_at IS NULL;
```

This partial index is a tighter fit for the slot lookup + `valid_only = true` case (exactly `(subject_id, predicate_label)` with `WHERE invalidated_at IS NULL`). SQLite's query planner may prefer `idx_graph_edges_live` over the wider `idx_graph_edges_spo` for slot queries with `valid_only = true`, causing the integration test assertion to fail.

**Suggested fix:** Amend the docstring to: "...must show SEARCH ... USING INDEX idx_graph_edges_spo (triple lookup) or idx_graph_edges_live (slot lookup with valid_only=true)". Or drop the specific index name assertion and just assert "USING INDEX" without specifying which one — let the optimizer pick.

---

## FINDING-3 🟡 Important — `EdgeInvalidation.superseded_by` vs `Edge.invalidated_by` naming mismatch

**Check #4 (Consistent naming)**
**Location:** §5bis `EdgeInvalidation` struct vs §3.2 `Edge` struct

**Problem.** Two types in the same design doc use different names for the same field:

- `Edge` (§3.2): `invalidated_by: Option<Uuid>` — "the successor's id"
- `EdgeInvalidation` (§5bis): `superseded_by: Option<Uuid>` — same semantics

The `apply_graph_delta` implementation will need to map `EdgeInvalidation.superseded_by` → `Edge.invalidated_by`. Two engineers could interpret these as distinct concepts (one might think `superseded_by` is an additional field). The resolution doc §4.2 also uses `superseded_by` (see v03-resolution review FINDING-2).

**Suggested fix:** Rename `EdgeInvalidation.superseded_by` to `EdgeInvalidation.invalidated_by` to match the `Edge` struct's field name. Consistently use `invalidated_by` throughout both docs for "prior → successor pointer" and `supersedes` for "successor → prior pointer."

---

<!-- FINDINGS -->

## Applied

(None — awaiting human approval before apply phase.)
