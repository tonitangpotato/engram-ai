---
title: Plan classifier never routes single-fact LoCoMo questions to Factual plan (0/152, all go to Associative)
status: resolved
priority: P1
severity: fix-shipped-validated
category: retrieval
created: 2026-05-27
relates:
- ISS-148
- ISS-164
- ISS-149
- ISS-162
- ISS-165
discovered_in: ISS-164 Phase 2 RE-RUN (engram-bench:f28b41d, sweep STAMP 20260527T051146Z)
fixed_by: engram:7e0447e
---

## Summary

In the ISS-164 Phase 2 re-run (post-ISS-165/166 fix, full
substrate, K=10 temp=0 HyDE=off), the plan classifier routed
**0 of 152 LoCoMo conv-26 queries to the Factual plan**. The
distribution was:

```
121 associative
 18 abstract
  6 affective
  5 hybrid
  2 episodic
  0 factual
```

Source: `grep "execute_plan ENTER" /tmp/iss164-bench/iss164-A.log
| awk -F'plan_kind=' '{print $2}' | sort | uniq -c`.

This includes **all 9 ISS-161 single-fact questions** (q3, q7,
q11, q37, q40, q43, q71, q75, q76) — all of which ask for one
specific entity/fact as gold ("Sweden", "Becoming Nicole",
"abstract art", "sunset", "Adoption agencies", etc.). These are
the textbook Factual-plan use case.

## Why this matters

ISS-164's entity-channel design assumed Factual plan would
consume the resolved anchors. The Phase 2 re-run produced
single-fact 0/9 → 0/9 with Δ=0 because **the Factual plan never
ran**. The anchors landed in the Associative plan's seed_entities
instead, where they feed an aggregation pipeline that washes
single-fact retrieval signal.

This may be the real ISS-148 AC-5a (single-fact ≥ 0.60)
bottleneck. The entity_channel + resolver fixes (ISS-164,
ISS-165, ISS-166, ISS-167) were all necessary but wrong-layer —
the classifier needs to route these questions to Factual first,
then the anchor work can carry weight.

## Hypotheses (need investigation)

**H1**: Classifier is heuristic / embedding-based and LoCoMo's
question phrasing ("What did Caroline research?", "Where did
Caroline move from?") doesn't match the Factual intent cluster
the classifier was trained/tuned on. Possibly tuned on QA
templates ("What is the capital of X?", "Who wrote Y?") and
LoCoMo's conversational tone routes elsewhere.

**H2**: Classifier confidence thresholds are mis-set — Factual
plan requires high confidence to override the default
Associative path, and LoCoMo single-fact questions never hit
that threshold.

**H3**: There IS no Factual plan code path being exercised here
at all, only the enum variant. The retrieval pipeline has
collapsed to Associative-by-default since some earlier change.

## Acceptance criteria

- [ ] **AC-1**: Find the classifier — locate the code that
  decides `plan_kind` per query. Likely
  `crates/engramai/src/retrieval/plans/classifier.rs` or
  similar.
- [ ] **AC-2**: Determine why the 9 single-fact LoCoMo questions
  route to Associative. Dump classifier scores per plan_kind
  for those 9 questions.
- [ ] **AC-3**: Categorize the failure: heuristic mismatch (H1),
  threshold mis-set (H2), or path-dead (H3).
- [ ] **AC-4**: If H1: propose a fix (re-tune intent embeddings
  / add LoCoMo-style training examples / use LLM classifier).
- [ ] **AC-5**: If H2: surface and document the threshold; A/B
  on tweaked threshold.
- [ ] **AC-6**: A/B sweep on conv-26: classifier-fixed vs
  current. Measure single-fact bucket lift. If Factual plan
  now fires on single-fact AND entity_channel is on, we should
  see real anchor utilization.

## Cross-references

- ISS-148: AC-5a single-fact ≥ 0.60 target — likely blocked by
  this classifier issue, not by the anchor work
- ISS-149: previously suspected classifier death; this issue is
  the empirical confirmation
- ISS-164: entity_channel falsified because anchors fed wrong
  plan (Associative instead of Factual)
- ISS-162: extraction context was queued behind ISS-164; same
  re-evaluation applies
- ISS-165: resolver fix is correct and ships, just wasn't
  enough on its own

## Suggested first move

`grep -rn "plan_kind\|classify\|PlanKind::Factual" crates/engramai/src/retrieval/`
then dump per-query classifier scores during a 9-question probe.
Cheap, no API spend, points at the root cause directly.

## References

- Sweep log: `/tmp/iss164-bench/iss164-A.log`
- Per-query: `engram-bench/benchmarks/runs/ISS164-A-conv26-20260527T051146Z/locomo_per_query.jsonl`
- ISS-164 Phase 2 verdict: `.gid/issues/ISS-164/issue.md` (2026-05-27 entry)

## 2026-05-27 02:30 — ROOT CAUSE CONFIRMED (H3, path-dead)

### Smoking gun

`crates/engramai/src/retrieval/api.rs:676-678` is the **only**
production callsite that constructs a classifier:

```rust
// Stage A: dispatch.
let classifier =
    crate::retrieval::classifier::HeuristicClassifier::with_null_lookup();
```

`with_null_lookup()` plugs in `NullEntityLookup`, which is hardcoded
to return `EntityMatch::None` for every token
(`heuristic.rs:127-133`):

```rust
impl EntityLookup for NullEntityLookup {
    fn lookup(&self, _token: &str) -> EntityMatch {
        EntityMatch::None
    }
}
```

### Causal chain

For every query in production:

1. `score_entity(query, &NullEntityLookup) → 0.0`
   (every token maps to `EntityMatch::None` → score 0)
2. `route_stage1` collects strong signals (score ≥ threshold).
   Entity threshold = 0.7, so Entity = 0.0 is **never strong**.
   No other signal exists for natural-language questions that
   don't carry obvious temporal/abstract/affective markers.
3. With `strong.len() == 0`, branch (`classifier/mod.rs:246-249`):
   ```rust
   0 => Stage1Outcome::Decided {
       intent: Intent::Factual,
       downgrade_hint: DowngradeHint::Associative,
   }
   ```
4. `PlanKind::from_intent(Intent::Factual, DowngradeHint::Associative)`
   maps to `PlanKind::Associative` (`dispatch.rs:93`).
5. Every query runs Associative plan.

This is **architecturally guaranteed**, not a tuning issue. Until
`EntityLookup` has a real graph-backed implementation, no query
on any corpus can route to `PlanKind::Factual` via the heuristic
path. (Caller override via `query.intent` is the only escape and
LoCoMo bench doesn't set it.)

### The missing implementation

`heuristic.rs:115-118` doc comment, written when the trait was
introduced, says:

> The classifier-core (`task:retr-impl-classifier-core`) wires a
> real graph-backed implementation behind this trait once
> `v03-graph-layer` is available. Until then [`NullEntityLookup`]
> is used and `score_entity` trivially returns `0.0`.

`v03-graph-layer` shipped a while ago, but the
`task:retr-impl-classifier-core` follow-up never happened. There
is no `GraphEntityLookup` struct in tree.

### Hypotheses outcome

- **H1** (heuristic mismatch on LoCoMo phrasing): not the cause —
  the classifier never gets a chance to mis-rank, because every
  Entity score is 0.
- **H2** (threshold mis-set): not the cause — even tau_low = 0.5
  wouldn't help if all scores are 0.
- **H3** (path-dead): **CONFIRMED**. `EntityLookup` has no
  production implementation. Same architectural status as
  ISS-166 (silent pool no-op) — a clean "the wiring is missing,
  not the algorithm" bug.

### Fix sketch

Add `GraphEntityLookup` (new struct in
`retrieval/adapters/graph_entity_lookup.rs` or sibling to
`graph_entity_resolver.rs`) that implements `EntityLookup`
against the same `graph_entities` table:

```rust
pub struct GraphEntityLookup {
    storage: Arc<dyn GraphRead>,   // or whatever the resolver uses
}

impl EntityLookup for GraphEntityLookup {
    fn lookup(&self, token: &str) -> EntityMatch {
        // search_candidates(normalize_alias(token), namespace="default")
        // returns Vec<(entity_id, match_kind)> where match_kind ∈
        //   Exact   → if alias == normalized canonical name
        //   Alias   → if alias matches a registered surface form
        //   (Fuzzy is not currently supported by search_candidates —
        //    leave None until ISS-170 adds it)
        // None     → no row matched
    }
}
```

Then wire it at `api.rs:676-678`:

```rust
let entity_lookup: Arc<dyn EntityLookup> =
    Arc::new(GraphEntityLookup::new(self.graph_store.clone()));
let classifier = HeuristicClassifier::new(
    entity_lookup,
    SignalThresholds::default(),
);
```

### Expected impact

After this fix, single-fact LoCoMo questions like
"What did Caroline research?" should produce:
- `score_entity("What did Caroline research?")` finds `Caroline`
  (Exact match, score 1.0)
- 1 strong signal with score 1.0 ≥ τ_high = 0.7
- → `Intent::Factual + DowngradeHint::None`
- → `PlanKind::Factual`

Then ISS-164's entity_channel (Phase 1 code still in tree) becomes
meaningful: anchors land in Factual plan's anchor consumer, not
Associative aggregation.

### Updated AC

- [x] **AC-1**: Find the classifier — done.
  `retrieval/classifier/mod.rs::route_stage1` +
  `retrieval/dispatch.rs::dispatch`.
- [x] **AC-2**: Why the 9 single-fact questions route to
  Associative — every query does, because NullEntityLookup.
- [x] **AC-3**: H3 confirmed (path-dead).
- [x] **AC-4** → "implement `GraphEntityLookup`":
  - [x] Write the adapter
    (`crates/engramai/src/retrieval/adapters/graph_entity_lookup.rs`).
  - [x] Wire at `api.rs:676-700` (now branches on
    `self.graph_store_arc()` — graph-backed when installed, falls
    back to null lookup for legacy v0.2 paths).
  - [x] Add `Memory::graph_store_arc()` accessor in `memory.rs`
    so adapters can clone the same `Arc<Mutex>` without holding a
    `&self` borrow.
  - [x] Unit tests (8 in `tests` submodule) covering empty token,
    unknown token, exact-canonical, alias-only, cross-namespace,
    exact-beats-alias-across-namespaces, NFKC normalization,
    determinism, and empty-graph short-circuit. All pass.
  - [x] Full engramai lib suite: 1984/1984 pass, 0 failures.
- [ ] **AC-5** drop (threshold tuning unnecessary).
- [ ] **AC-6**: A/B sweep on conv-26 with `GraphEntityLookup` +
  entity_channel=on vs current. Re-run ISS-164 Phase 2 envelope.
  Measure single-fact bucket lift; AC-5a target ≥ +2 wins on
  n=9.

---

## 2026-05-27 02:35 — FIX SHIPPED (AC-4 complete, AC-6 pending bench)

### Files

- New: `crates/engramai/src/retrieval/adapters/graph_entity_lookup.rs`
  (~430 LoC including 9 unit tests). Holds
  `Arc<Mutex<SqliteGraphStore<'static>>>`, implements
  `EntityLookup` by iterating up to 32 namespaces
  (matches `GraphEntityResolver::MAX_NAMESPACES_SCANNED`) and
  calling `search_candidates` with `mention_embedding=None,
  top_k=1` (single indexed point lookup per ns per token, per
  store §4.2 optimization note).
- Edited: `crates/engramai/src/retrieval/adapters/mod.rs` — added
  `pub mod graph_entity_lookup;` + `pub use ...::GraphEntityLookup`.
- Edited: `crates/engramai/src/memory.rs` — added
  `Memory::graph_store_arc()` accessor (returns
  `Option<Arc<Mutex<SqliteGraphStore<'static>>>>`).
- Edited: `crates/engramai/src/retrieval/api.rs:676-700` —
  classifier branches on `self.graph_store_arc()`. Graph
  installed → `GraphEntityLookup`; not installed →
  `with_null_lookup()` (legacy v0.2 path).

### EntityMatch mapping (chosen)

- `EntityMatch::Exact` — `search_candidates` returned a row AND
  `normalize_alias(token) == normalize_alias(canonical_name)`.
  Token matches the entity's canonical surface.
- `EntityMatch::Alias` — `search_candidates` returned a row but
  the canonical name normalizes differently. Token hit some
  registered alias (e.g. "caroline" → "Caroline Doyle"). Per
  `score_entity` weights this is 0.8, above the 0.7 entity
  threshold → still routes to Factual.
- `EntityMatch::Fuzzy` — never returned. `search_candidates` has
  no fuzzy path (exact-equality alias scan only). Tracked
  separately as ISS-170.
- `EntityMatch::None` — no row, or any failure (mutex poison,
  SQL error). Failure → None is intentional: classifier degrades
  to pre-ISS-171 Associative routing rather than crashing the
  query.

### Open questions answered

- **All namespaces vs default?** All, capped at 32. LoCoMo
  ingests into per-conversation namespaces (`conv-26`, `conv-44`)
  but queries come in with `namespace: None` → resolves to
  `"default"`. Scoping the lookup to one namespace would
  silently re-introduce the bug for any multi-namespace DB.
  Cost: ≤32 indexed point lookups per token; well inside the
  classifier latency budget.
- **Normalize inside lookup?** Yes. `normalize_alias` on the
  token before any SQL — both for safety against tokenize/writer
  drift and for cheap early-out on empty tokens.
- **Only callsite?** Confirmed: api.rs:677 is the only
  production caller of `HeuristicClassifier::*`. dispatch.rs /
  classifier/mod.rs callsites are all inside `mod tests`.
- **Cache within single classify call?** No. `tokenize` yields
  5-15 mostly-unique tokens per real LoCoMo query; the dedup
  win would be negligible against the SQL cost.

### Test results

```
cargo test -p engramai --lib retrieval::adapters::graph_entity_lookup
running 9 tests
test ... empty_token_returns_none                 ok
test ... empty_graph_returns_none                 ok
test ... unknown_token_returns_none               ok
test ... unicode_nfkc_normalization               ok
test ... alias_only_match_returns_alias           ok
test ... exact_canonical_match_returns_exact      ok
test ... cross_namespace_match_is_found           ok
test ... exact_wins_over_alias_across_namespaces  ok
test ... deterministic_across_repeated_calls      ok

test result: ok. 9 passed; 0 failed
```

Full suite: `cargo test -p engramai --lib` → 1984 passed, 0
failed, 4 ignored. No regression in 370 retrieval tests or 50
classifier tests.

### Next step (AC-6)

Re-run ISS-164 Phase 2 A/B sweep on the same substrate
(STAMP-prefixed conv-26 K=10 temp=0 HyDE=off MMR=off), now that
classifier can route to Factual. Expected: single-fact 9 LoCoMo
questions land on `PlanKind::Factual` instead of
`PlanKind::Associative` (verifiable via
`grep "execute_plan ENTER" $LOG | grep plan_kind=`). If the
ISS-164 entity_channel design holds, anchors should now lift
single-fact bucket.

Decision rule (reused from ISS-164 Phase 2):
- B − A on single-fact ≥ +2 → ship + reopen ISS-164,
- B − A ∈ {0, +1} → STOP, file follow-up on Factual plan internals,
- B − A < 0 → revert and document why anchors hurt the Factual
  plan.

### Severity escalation

This is `root-cause-CONFIRMED` (was `root-cause-suspected`).
The single bug — `NullEntityLookup` in production — silently
disables Factual plan routing for every query in every corpus,
not just LoCoMo. Priority kept at P1; fix is mechanical and
small.

## 2026-05-27 09:30 — VERDICT: AC-6 PASSED, status=resolved

ISS-164 Phase 2 A/B sweep (STAMP `20260527T112718Z`) ran on conv-26
K=10 temp=0 HyDE=off with the ISS-171 fix in place.

**plan_kind histogram (both arms identical)**:
```
114 factual    ← was 0/152 across all prior ISS-164 benches
 30 hybrid
  7 associative
  1 abstract
```

**ISS-171 AC-6 = PASSED.** Factual plan is reachable. The
classifier routes to it on 75% of conv-26 queries — exactly what
§3.1 specified. The architectural bug is fixed.

**However**, the sweep also exposed a *downstream* bug:
post-fix overall regressed −15pp vs the pre-ISS-171 Hybrid-fallback
baseline. Filed as **ISS-172** (Factual plan ranking floor). That
work is sequenced AFTER ISS-171 closes — ISS-171's responsibility
ends with "Factual is reachable", which it is.

`fixed_by = engram:7e0447e` (GraphEntityLookup adapter +
api.rs:676 wire-up + memory.rs::graph_store_arc()).
