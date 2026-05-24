---
title: L2 — Production retrieval classifier uses NullEntityLookup, never selects Factual plan
status: open
priority: P1
severity: bug
category: retrieval
created: 2026-05-24
relates:
- ISS-144
- ISS-145
- ISS-147
- ISS-148
depends_on: .gid/issues/ISS-145/issue.md
---

## Summary

`retrieval/api.rs:496` instantiates the production `HeuristicClassifier`
with `with_null_lookup()`. `NullEntityLookup::lookup` returns
`EntityMatch::None` for every token, so `score_entity` returns 0.0
on every query. This single line forces 80% of conv-26 queries onto
the Associative fallback path (per ISS-148 evidence), making the
Factual plan effectively unreachable in production.

This is **L2** in the entity-resolution layering:

- **L1** (ISS-144, fixed `7eee30e`): extractor produces Person entities.
- **L1b** (ISS-145, open): ingest writes those entities into
  `graph_entity_aliases` so `GraphEntityResolver` (Factual plan's
  resolver) can see them.
- **L2** (this issue): classifier `EntityLookup` reads from the
  same store. Without this, L1 + L1b are wasted — the classifier
  has its own independent EntityLookup that doesn't see graph state.

ISS-145 marked L2 explicitly out of scope and pointed at this line.

## Evidence

`crates/engramai/src/retrieval/api.rs:494-496`:

```rust
// Stage A: dispatch.
let classifier =
    crate::retrieval::classifier::HeuristicClassifier::with_null_lookup();
```

`crates/engramai/src/retrieval/classifier/heuristic.rs:130-135`:

```rust
pub struct NullEntityLookup;

impl EntityLookup for NullEntityLookup {
    fn lookup(&self, _token: &str) -> EntityMatch {
        EntityMatch::None
    }
}
```

Score path (`heuristic.rs:151`):

```rust
pub fn score_entity(query: &str, lookup: &dyn EntityLookup) -> f64 {
    // ... iterates tokens, calls lookup() for each, max-pools.
    // With NullEntityLookup every token returns None → score 0.0.
}
```

Classifier branch (`classifier/mod.rs:245-249`):

```rust
// No strong signal → Factual with Associative downgrade hint.
HeuristicResult {
    intent: Intent::Factual,
    downgrade_hint: DowngradeHint::Associative,
    ...
}
```

Dispatch (`dispatch.rs:92`):

```rust
(Intent::Factual, DowngradeHint::Associative) => PlanKind::Associative,
```

End-to-end: every entity-anchored query becomes
Associative-fallback because the Entity signal is hard-wired to 0.0.

## Conv-26 cost

From ISS-148:

| plan_kind   | count | should it be Factual? |
|-------------|------:|------------------------|
| associative | 121   | many of these are "Caroline did X" / "Melanie's Y" — Factual material |
| factual     | 0     | should be ~half of associative bucket |

121 single-hop / 152 = 80% misrouted. Single-hop accuracy stuck at
0.219 because Factual+BM25 is unreachable.

## Proposed fix

Introduce a production-capable `EntityLookup` impl that reads from
`graph_entity_aliases` (the same store `GraphEntityResolver` reads —
ISS-145 fills it). Likely candidates:

- **A — Reuse `GraphEntityResolver` shape:** wrap it in an adapter
  that implements `EntityLookup::lookup(token) → EntityMatch`.
  Map `search_candidates`-hit → `Exact` (or `Alias` if normalization
  differs), miss → `None`. Smallest code.
- **B — New thin `GraphEntityLookup`:** direct
  `SELECT 1 FROM graph_entity_aliases WHERE alias = ? LIMIT 1` ->
  Exact / None. Sidesteps the resolver's heavier `search_candidates`
  query (which scores + ranks). Cheaper per-query (classifier runs
  before plan dispatch, on every query — must be fast).

Recommend **B**. `EntityMatch` is a 3-variant enum
(`Exact`/`Alias`/`None`); the classifier only needs the
exact-vs-not bit. The resolver's ranking is wasted here.

Either way, the construction at `api.rs:496` becomes:

```rust
let entity_lookup: Arc<dyn EntityLookup> =
    Arc::new(GraphEntityLookup::new(graph_store.clone()));
let classifier = HeuristicClassifier::new(
    entity_lookup,
    SignalThresholds::default(),
);
```

## Dependency

**Blocked-by ISS-145.** Without L1b populating `graph_entity_aliases`,
the new lookup will still return None on every query, and the symptom
won't change. ISS-145 must land first.

## Acceptance Criteria

- [ ] **AC-1:** New `EntityLookup` impl + unit tests (Exact match,
       miss, case-normalization parity with `GraphEntityResolver`).
- [ ] **AC-2:** `api.rs:496` wired to the new impl. `with_null_lookup`
       kept and used by tests only.
- [ ] **AC-3:** `cargo test -p engramai --lib` green.
- [ ] **AC-4:** Conv-26 re-bench: Factual plan selection rate ≥30%
       on single-hop queries (vs 0% today).
- [ ] **AC-5:** Conv-26 single-hop ≥0.35 (matches ISS-148 AC-3).
- [ ] **AC-6:** Full LoCoMo 1540q regression: no category regresses
       more than 1pp vs ISS-147 baseline.

## Out of scope

- LLM-fallback classifier wiring (`classifier/llm_fallback.rs`).
  Separate path, not on the LoCoMo hot loop.
- Threshold tuning (`SignalThresholds::default`). The threshold for
  Entity is fine — the problem is the input signal is hard-wired 0.0.
- Multi-token entity resolution ("Caroline Smith" vs "Caroline"):
  separate quality issue in `GraphEntityResolver`, not in
  classifier wiring.

## References

- ISS-148 — root-cause writeup with conv-26 plan distribution
- ISS-145 — L1b ingest → `graph_entity_aliases` (prereq)
- `crates/engramai/src/retrieval/api.rs:496` — the offending call
- `crates/engramai/src/retrieval/classifier/heuristic.rs:121-135` — trait + Null impl
- `crates/engramai/src/retrieval/adapters/graph_entity_resolver.rs` — reader to mirror
