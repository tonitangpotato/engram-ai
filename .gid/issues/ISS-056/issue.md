---
id: ISS-056
title: "Retrieval API hardcodes namespace=\"default\" — GraphQuery has no namespace field, multi-tenant retrieval impossible"
status: closed
severity: critical
filed: 2026-04-28
closed: 2026-04-29
verified_by: [RUN-0001, RUN-0002]
related: [ISS-049, ISS-055, ISS-046, ISS-048]
---

# ISS-056: Retrieval API hardcodes `namespace = "default"` — `GraphQuery` has no namespace field, so multi-tenant retrieval is impossible

- **Status**: closed (verified by RUN-0001 + RUN-0002 on namespace `locomo-conv26-iss058`)
- **Closed**: 2026-04-29
- **Severity**: critical (blocks every non-`default` namespace retrieval — including the LoCoMo conv-26 acceptance run after ISS-055 fix lands)
- **Filed**: 2026-04-28
- **Discovered during**: ISS-055 verification — even with the worker-side namespace propagation fix (commit `4526884`), conv-26 queries still return 0 hits because the retrieval entry point (`retrieval/api.rs:431`) constructs every adapter (`HybridSeedRecaller`, `HybridAffectiveSeedRecaller`) with the literal string `"default"`, regardless of which namespace the data actually lives in.
- **Related**: ISS-049 (Phase 4 multi-namespace dispatch — explicitly deferred in the ISS-049 plan, see `api.rs:420` comment "multi-namespace dispatch is Phase 4"). ISS-055 (sibling — write-side namespace propagation, must land first; this issue is the read-side counterpart). ISS-046 (CLI write path — closed). ISS-048 (manual normalize-DB workaround — temporary, no longer needed after this lands).

---

## Closure Verification (2026-04-29)

Closed in tandem with ISS-055 — same RUN-0001 / RUN-0002 evidence chain. The retrieval driver now passes `--ns locomo-conv26-iss058`, `GraphQuery.namespace` is an `Option<String>` plumbed end-to-end, and the literal `"default"` at `api.rs:412` is a documented fallback for callers that omit the namespace (single-tenant compatibility), no longer a hardcode. RUN-0001 returning 10/13 Factual hits against `locomo-conv26-iss058` proves the read path threads the user-supplied namespace through every adapter.

---

---

## Summary

The public retrieval entry point `Memory::graph_query` accepts a
`GraphQuery` struct (`retrieval/api.rs:100`) with **no namespace field**.
Inside `graph_query` (`api.rs:431`), the namespace passed to every
storage-backed retrieval adapter is hardcoded:

```rust
// crates/engramai/src/retrieval/api.rs:420-431
// Namespace: bound to `"default"` as a Phase-3 interim. The
// resolution layer + fingerprinting work that wires real
// multi-namespace dispatch is Phase 4 (see ISS-049 plan).
// ...
let namespace: &str = "default";
```

This `namespace` is then threaded into:
- `HybridSeedRecaller::new(storage, embedding, namespace, ...)` (line 442)
- `HybridAffectiveSeedRecaller::new(storage, embedding, namespace, ...)` (line 451)

Both adapters use it to scope SQL queries against the per-namespace
`memories` / `graph_entities` / `graph_topics` tables. When the user's
data is ingested under namespace `conv26` (the LoCoMo case), the adapter
queries `WHERE namespace = 'default'` and returns empty.

**Concrete symptom:** with ISS-055's worker fix applied and conv-26
fresh-ingested under `--ns conv26`, every plan downgrades:
- Factual plan → `DowngradedFromAssociativeNoSeeds` (HybridSeedRecaller misses)
- Associative plan → empty seeds → empty results
- Affective plan → empty seeds → empty results
- → 0/25 hit rate on the LoCoMo benchmark, despite data being correctly written.

---

## Root Cause

**`GraphQuery` was designed without a namespace field.** ISS-049 Phase 3
(retrieval skeleton) shipped with `namespace = "default"` as an explicit
TODO marker for Phase 4 (multi-namespace dispatch). Phase 4 was never
scheduled — the comment is the only trace.

The downstream adapters (`HybridSeedRecaller`, `HybridAffectiveSeedRecaller`)
already accept a `&str` namespace parameter and propagate it into SQL
correctly. The hole is purely at the **API surface**: the caller has no
way to *say* which namespace they want to query.

This is a **single-point fix** — not a cross-cutting refactor. Per-callsite
audit (2026-04-28):

- 100+ `namespace: &str` / `Option<&str>` signatures across
  `memory.rs`, `storage.rs`, `graph/store.rs`, etc. → **all working
  correctly**, namespace propagates end-to-end on the write path and
  inside retrieval adapters.
- `PipelineConfig.namespace` → fixed in ISS-055.
- `Memory::with_pipeline_pool` graph-store wiring → fixed in ISS-050.
- `retrieval/api.rs:431` → **the only remaining hole**.

---

## Fix Design

**Add `namespace: Option<String>` to `GraphQuery`, plumb it to the two
adapter constructors. ~5 callsites, ~30 lines.**

### Changes

1. **`crates/engramai/src/retrieval/api.rs`**:
   - Add `namespace: Option<String>` field to `GraphQuery` (default `None`)
   - Add builder method `GraphQuery::with_namespace(self, ns: impl Into<String>) -> Self`
   - Replace `let namespace: &str = "default";` with:
     ```rust
     let namespace: &str = query.namespace.as_deref().unwrap_or("default");
     ```
   - Update the comment at line 420 to point at this issue (resolved) instead of "Phase 4 deferred"

2. **Update `Default` for `GraphQuery`** to set `namespace: None`.

3. **Benchmark/CLI callers** that build `GraphQuery` for non-default
   namespaces:
   - `crates/locomo-bench/` (or wherever the benchmark constructs queries)
   - any `engram` CLI subcommand that accepts `--ns` and runs retrieval
   - These should call `.with_namespace(ns)` when the user passes `--ns`.

4. **Tests**:
   - Unit: `GraphQuery::with_namespace()` sets the field.
   - Integration: ingest two namespaces (`ns_a`, `ns_b`) with disjoint
     content, verify `graph_query` with `namespace=ns_a` returns only
     `ns_a` content (no leakage from `ns_b`).
   - Regression: existing tests using default `GraphQuery` still pass
     (None → "default" preserves current behavior).

### Why `Option<String>` not `String`?

- Backward compat: `GraphQuery { text: "...".into(), ..Default::default() }`
  still compiles.
- Existing callsites (~100s of tests) that don't care about namespace
  don't have to be updated.
- "default" namespace is still the documented fallback for single-tenant
  use cases (matches current behavior).

### Why not a `Namespace` newtype / first-class type?

Considered and rejected for this issue. Rationale:

- The 100+ existing `namespace: &str` signatures are **working code**,
  not bugs. They correctly propagate namespace end-to-end. Replacing
  them with a newtype is a stylistic refactor, not a root fix.
- The actual bug — "caller has no way to specify namespace at the
  retrieval API surface" — is solved by adding one field.
- A `Namespace` newtype + `Default`-removal + builder enforcement
  is a 3-day refactor that would be triggered by a *future* class of
  bugs (signature ordering errors, accidental `&str` confusion). No
  evidence of those bugs today.

→ **Filed separately as ISS-057** (P2 enhancement, do later if/when
that bug class actually appears).

---

## Acceptance

1. **Unit**: `GraphQuery::with_namespace("foo")` sets `query.namespace = Some("foo")`.
2. **Integration**: two-namespace isolation test passes (no cross-namespace leak).
3. **End-to-end**: LoCoMo conv-26 fresh-ingest under `--ns conv26` →
   `graph_query` with `with_namespace("conv26")` → ≥ baseline hit rate
   (target: match or exceed the ISS-049 12/25 baseline once the data
   path is unblocked).
4. **Regression**: full `cargo test --workspace` clean.

---

## Out of Scope

- Cross-namespace queries (`namespace=All` / `Set(...)`) — separate
  feature, file as new issue if needed.
- `Namespace` newtype refactor — see ISS-057.
- `PipelineConfig` / write-path namespace plumbing — already fixed in
  ISS-055.

---

## History

- 2026-04-28: Filed during ISS-055 verification debug session. potato
  asked for "root fix without tech debt" — initial draft proposed a
  3-day `Namespace` newtype refactor, then rescoped after audit showed
  the bug is a single-point hole at the API surface, not a cross-cutting
  type-system gap.
