---
id: "ISS-039"
title: "Associations & Causal Recall Paths Misuse query-confidence Scorer"
status: resolved
priority: P2
created: 2026-04-26
resolved: 2026-04-26
component: crates/engramai/src/memory.rs
related: [ISS-032]
---

## Resolution (2026-04-26)

Shipped via Option A. New `compute_association_confidence(activation, age_hours)` lives alongside `compute_query_confidence` in memory.rs. All three association/causal call sites (lines 4466, 4611, 4977) switched. `RecallResult` shape unchanged — zero downstream churn.

Verification: 6 new association-confidence tests pass; full lib (1415 tests) + integration suite remain green. Commit: e650dd8.

# ISS-039: Associations & Causal Recall Paths Misuse query-confidence Scorer

**Status:** 🔴 Open
**Priority:** Medium — degrades downstream confidence_label semantics on three recall paths; not a hot-path correctness bug like ISS-032 was.
**Discovered:** 2026-04-26 during ISS-032 line-by-line audit.
**Reporter:** RustClaw (audit findings).
**Decision:** 2026-04-26 — **Option A** (API split). See Recommendation section.

---

## Executive Summary

`compute_query_confidence(embedding_similarity, in_fts_results, entity_score, age_hours)` is a **query-relevance** scorer. It assumes the caller is answering "how relevant is this memory to the query the user asked?" using three relevance signals (embedding similarity, FTS hit, entity overlap) and an age decay.

Three non-query recall paths call it anyway, with all three relevance signals zeroed:

- **memory.rs:4466** — `recall_associated` / `recall_associated_ns` (Hebbian associations from a seed memory).
- **memory.rs:4616** — cached working-memory associations leg (when WM cache hits, a fresh confidence is computed for already-cached items).
- **memory.rs:4987** — `recall_with_associations` (associations leg).

All three paths have **no `query` string** to score against — they walk co-activation links from a seed. Calling a query-confidence scorer with `(None, false, 0.0, age_hours)` is meaningless: only the age-decay term contributes, so every result lands with `confidence_label: "very low"`.

**Audit note:** The original issue draft listed only sites 4466 and 4987. Site 4616 was found during the line-by-line decision audit (2026-04-26) and is included here for completeness. All three sites must be fixed together — leaving any one behind perpetuates the misuse.

This is **not** a missing-signal carry-forward bug (which was ISS-032 sub-fix 3b, already shipped). It is API misuse — the wrong scoring model is being applied to a path that has different semantics.

---

## Why This Is Different From ISS-032

ISS-032 covered the **cached working-memory path**, where the scorer is the right tool but was being called with zeros because upstream signals weren't carried forward. Sub-fixes 3a (redundant probe) and 3b (carry-forward) closed it.

ISS-039 covers paths where the scorer **does not apply at all** — there is no query, so there is nothing to score relevance against. Carrying signals forward isn't possible because none exist.

---

## The Three Sites

### Site A — `recall_associated` family (line ~4466)

Walks Hebbian links from a seed memory. Result confidence ought to express "how strongly is this associated with the seed?" — i.e., a function of **link strength / co-activation count / decay**, not query relevance.

### Site B — cached working-memory associations leg (line ~4616)

When the working-memory cache is hit during an associations recall, a fresh confidence is computed for the cached records. Same all-zeros pattern as the others — no query, no embedding, no FTS, no entity.

### Site C — `recall_with_associations` associations leg (line ~4987)

Comment in the code already admits the mismatch:

```rust
let confidence = compute_query_confidence(
    None,   // no embedding in associations path
    false,  // not an FTS query match
    0.0,    // no entity score
    age_hours,
);
```

Three of the four arguments are nullified at the call site. This is a code smell — the function being called isn't the function this path needs.

All three sites have ACT-R **activation** already computed in scope (via `self.config.actr_decay`, `context_weight`, `importance_weight`, `contradiction_penalty`). Activation encodes link strength × co-activation × time decay — it is the natural basis for an association-confidence model.

---

## Design Options (decision pending)

### Option A — API split

Introduce `compute_association_confidence(link_strength, recency, ...)` for non-query paths. Keep `compute_query_confidence` strictly for query-driven recall.

- **Pro:** clean separation of concerns; type system documents the difference.
- **Con:** two scorers to maintain; need to define the association scoring model.

### Option B — Guard at call site

Detect the all-zeros / no-query case inside `compute_query_confidence` and return a sentinel (e.g., `f64::NAN` or a different label like `"associated"`) so downstream consumers can distinguish "low confidence relevance match" from "association — no relevance scoring applies".

- **Pro:** minimal API churn.
- **Con:** mixes two concepts in one function; downstream `min_confidence` filters still need to learn the new semantics.

### Option C — New scorer for associations

Same as A but more concrete: define an `AssociationScore` newtype and a dedicated scorer driven by Hebbian link strength + recency. Recall results from these paths emit `AssociationScore`, not `confidence`. Downstream consumers may have to be updated.

- **Pro:** correct model; future-proof if more non-query recall modes appear (e.g., temporal-only, entity-only).
- **Con:** largest blast radius; touches `RecallResult` shape and every caller that reads `.confidence`.

### Recommendation

**Decision (2026-04-26): Option A.**

Reasoning:
- The bug is at the **calculation layer** (wrong model applied), not the **representation layer** (`RecallResult.confidence: f64`). A scalar 0–1 confidence is meaningful for both query-relevance and association-strength — only the formula differs.
- Option C reshapes `RecallResult` (a public type), which forces a breaking change on every downstream consumer (rustclaw, autopilot, CLI). That is a large blast radius for a fix that is fundamentally about computing the right number.
- Option B mixes two semantics in one function and was rejected per SOUL.md "elegant, not clever".
- Option A: add a sibling function `compute_association_confidence(activation, age_hours) -> f64`, switch the three call sites to it, leave `RecallResult` and all downstream consumers untouched. Smallest blast radius for the smallest representable correct fix.

Future extension: if more non-query recall modes appear (temporal-only, entity-only), they each get their own scorer or share `compute_association_confidence` if activation captures their semantics. The shape of `RecallResult` stays stable across all of them.

---

## Acceptance Criteria

When this issue is resolved:

- [ ] `compute_query_confidence` is no longer called from any of the three identified non-query sites (4466, 4616, 4987).
- [ ] A new `compute_association_confidence(activation, age_hours) -> f64` exists and is documented (model: activation as primary signal + recency as secondary).
- [ ] All three association/causal call sites use the new function.
- [ ] `confidence_label` thresholds remain unchanged — the new function returns a value on the same 0–1 scale so downstream label mapping continues to work.
- [ ] Unit tests exist for the new scoring model with realistic inputs: (strong activation × fresh), (strong activation × old), (weak activation × fresh), (weak activation × old) — at minimum 4 cases.
- [ ] No changes to `RecallResult` shape — downstream consumers (RustClaw, autopilot, CLI) require zero updates.
- [ ] Existing query-path tests for `compute_query_confidence` continue to pass unchanged.

---

## Out of Scope

- The cached query-path scoring (ISS-032 — already resolved).
- Recall ranking changes other than confidence reporting.
- Confidence label thresholds — separate tuning concern.
