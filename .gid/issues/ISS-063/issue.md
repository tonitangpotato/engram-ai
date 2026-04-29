---
id: ISS-063
title: Implement downgrade-to-fallback contract (all plans → Associative) per design §3.4 / §6.4
status: in_progress
severity: high
priority: P1
labels:
- retrieval
- orchestrator
- root-fix
relates_to:
- ISS-060
- ISS-061
- ISS-062
discovered: 2026-04-28
writeup: .gid/docs/retrieval-downgrade-contract-problem.md
---

# Implement downgrade-to-fallback contract

## TL;DR

Plans correctly emit `Downgraded*` outcomes when their preconditions don't
hold. The design says the orchestrator routes downgrades to a fallback plan
(Abstract→Associative, Episodic→Factual, etc.). **Nobody implements that
routing.** Downgrades are surfaced to the API caller as the final outcome
and the response is empty.

This is the actual root cause of ISS-060 (Abstract returning 0) and
ISS-061 (Hybrid empty). Filing as a separate issue because it crosses
multiple plans and the orchestrator, and because ISS-060/ISS-061 mis-located
the bug to specific plans (ISS-060 at "Abstract downgrade chain", ISS-061
at "Hybrid sub-plan handling"). The plans themselves are correct.

## Evidence trail

### 1. Plans correctly emit downgrades

- `plans/episodic.rs:421` — when `inputs.time_window` is `None` or
  `TimeWindow::None`, returns `EpisodicOutcome::DowngradedFromEpisodic`
  with `memories: Vec::new()`. Comment at line 264:
  `"caller should re-dispatch as Factual"`. Plan does NOT do the
  re-dispatch itself, by design (it's a leaf executor).
- `plans/abstract_l5.rs:368, 381` — emits
  `AbstractOutcome::DowngradedL5Unavailable` when `topic_searcher`
  returns nothing OR all hits fall below `min_topic_score`. Module doc
  §3.4 reference at line 40, 57, 68: *"orchestrator routes to Associative
  per design §3.4"*.
- `plans/affective.rs` — emits `DowngradedNoSelfState` analogously.

### 2. Orchestrator does NOT implement the contract

`crates/engramai/src/retrieval/orchestrator.rs::execute_plan`:

- **Abstract branch (lines 835-885):** match on `AbstractOutcome` does
  exactly four things:
  - `Ok` → translate to `RetrievalOutcome::Ok`
  - `DowngradedL5Unavailable` → translate to
    `RetrievalOutcome::DowngradedFromAbstract { reason: "L5_unavailable" }`
  - `Cutoff` → translate to `RetrievalOutcome::Cutoff`
  - `Empty` → translate to `RetrievalOutcome::Ok` with empty candidates

  **No call to Associative plan.** The function returns the empty result
  and the downgrade outcome propagates to `Memory::graph_query`'s caller
  unchanged. Design §3.4 ("orchestrator routes to Associative") is not
  honoured.

- **Episodic branch (lines 798-833):** identical pattern. `Downgraded*`
  → translate to `RetrievalOutcome::Downgraded*`, return empty. **No
  call to Factual plan.** Episodic's own doc says
  `"caller should re-dispatch as Factual"`; the orchestrator is the
  caller and it doesn't.

- **Affective branch:** ditto.

### 3. Hybrid silently drops sub-plan outcomes

`orchestrator.rs::HybridDispatchExecutor::run` (lines 555-700):

```rust
SubPlanKind::Episodic => {
    let result = plan.execute(inputs, ...);
    let items: Vec<HybridItem> = result
        .memories
        .into_iter()
        .map(HybridItem::Memory)
        .collect();
    SubPlanResult { kind, items }
}
```

`result.outcome` is never inspected. `SubPlanResult` has no `outcome`
field. So a sub-plan that emitted `DowngradedFromEpisodic` looks
identical to one that emitted `Ok` with zero items. Hybrid's RRF
aggregator sees only empty `items`, fuses nothing, returns 0 candidates.

### 4. Hybrid top-level outcome is hard-coded to `Ok`

`orchestrator.rs` (Hybrid branch in `execute_plan`): the final outcome
returned to the API is `RetrievalOutcome::Ok` regardless of whether all
sub-plans returned 0. This is what produces the misleading
`outcome=ok candidates=0` log lines that triggered the misdiagnosis in
ISS-061. (See ISS-062 for observability fix.)

### 5. `plan_used` never reflects fallback

`api.rs:526`: `plan_used: intent` — set to the dispatched intent
verbatim. The doc comment on `plan_used` (line 319) says it
*"may differ from `query.intent` after `RetrievalOutcome::Downgraded*`"*,
but no code path makes it differ. Today this is dead documentation.

## Design references

- `.gid/features/v03-retrieval/design.md` §3.4 — outcome surface and
  fallback graph
- `.gid/features/v03-retrieval/design.md` §6.4 — `RetrievalOutcome`
  enum and the contractual relationship between `Downgraded*` and
  `plan_used`
- `.gid/features/v03-retrieval/design.md` §4.7 — Hybrid plan: per
  current text, sub-plan failures are absorbed by RRF (this may need
  revisiting in light of this issue — see Open Questions)

## Why this is a root fix, not a patch

A patch would be: tweak the classifier so queries without a `time_window`
don't route to Episodic (or to Hybrid-with-Episodic-sub-plan). That just
hides the symptom for *some* queries. The contract violation remains for:

- Any query the classifier still routes to Episodic with no time anchor
- Any query routed to Abstract when L5 has no topics in the namespace
- Any query routed to Affective without self-state

The root cause is that the orchestrator never implemented the
downgrade-to-fallback edges that the plans were designed to emit. Fix
the orchestrator → all of these query classes start producing results
(or fail with a more honest outcome).

## Scope

### Required

1. **Episodic → Factual fallback** in `execute_plan::PlanKind::Episodic`:
   on `DowngradedFromEpisodic`, run the Factual plan with the same
   `query`/`graph`/`loader` and return its result. Set
   `plan_used = Intent::Factual` in the response and surface
   `RetrievalOutcome::DowngradedFromEpisodic { fallback_plan_used: Factual }`.

2. **Abstract → Associative fallback** in `execute_plan::PlanKind::Abstract`:
   on `DowngradedL5Unavailable`, run the Associative plan. Set
   `plan_used = Intent::Associative` and surface the downgrade outcome
   with `fallback_plan_used`.

3. **Affective → ?** check design §3.4 for the prescribed fallback
   (likely Episodic-or-Factual depending on whether self-state is the
   missing piece).

4. **Hybrid sub-plan outcome propagation:** extend `SubPlanResult` with
   an `outcome` field. When a sub-plan downgrades, the dispatch
   executor MUST replace it with the designed fallback sub-plan
   (e.g. an Episodic sub-plan that downgrades is replaced with
   Factual). RRF then operates on the actual results from the
   fallback. This is the Hybrid analogue of the single-plan fallback
   contract.

5. **`RetrievalOutcome` enum extension:** add
   `fallback_plan_used: Option<Intent>` to the `Downgraded*` variants
   so callers can see what actually ran. Update `plan_used` write-back
   in `api.rs:526` to use the fallback plan when applicable.

### Out of scope (separate issues)

- ISS-064 (suggested): NL→TimeWindow heuristic so queries with phrases
  like "after the charity race" can populate `time_window` upstream of
  Episodic. This is feature work, not contract enforcement.
- ISS-062: observability/logging — separate issue, lands first to make
  this fix verifiable.

## Acceptance

- conv-26 retrieval smoke: the 4 queries currently logging
  `outcome=ok candidates=0` either return non-empty results OR surface
  a *post-fallback* outcome (`DowngradedFromEpisodic { fallback_plan_used: Factual }`
  with non-empty results from Factual, OR a final empty with
  `RetrievalOutcome::Empty` if Factual also has nothing).
- New unit test per fallback edge: feed a downgrade-triggering input,
  assert (a) the fallback plan ran, (b) `plan_used` reflects the
  fallback, (c) outcome carries `fallback_plan_used`.
- Hybrid integration test: an Episodic-flavoured sub-plan with no
  `time_window` produces Factual-fallback items in the RRF input,
  not zero items.
- Existing tests still pass; in particular the tests that currently
  assert `plan_used == intent` for non-downgraded paths must stay
  green.

## Open questions

- **Hybrid sub-plan fallback design:** §4.7 says "RRF absorbs sub-plan
  empties." Is the design actually OK with downgrades being absorbed
  silently? Or did §4.7 assume sub-plans would never downgrade because
  Hybrid only picks them when their preconditions hold? Worth a
  one-line check with the design intent before implementing item (4)
  above.
- **`RetrievalOutcome` API stability:** adding `fallback_plan_used`
  changes the public enum. Acceptable? (The crate is pre-1.0, but
  external benchmarks consume this enum.)
- **Recursive downgrades:** if Factual itself fails (e.g. no entity
  match → `NoEntityFound`), should we cascade further or stop?
  Recommend: stop. Document the chain depth = 1.

## Risk

Medium. This touches the public `GraphQueryResponse` shape (via
`RetrievalOutcome` extension) and changes observed behaviour for any
query that currently downgrades. Mitigation:

- Land ISS-062 first so logs distinguish pre/post-fallback.
- Snapshot conv-26 results before/after to confirm only previously-empty
  queries change.
- Keep `Downgraded*` enum variants additive (new optional field, not
  renamed).

## Replaces / supersedes

- **ISS-060** ("Abstract downgrade chain returns 0 candidates"): the
  Abstract plan itself is correct. Re-target ISS-060 as a sub-task of
  this issue (specifically: implement the Abstract→Associative edge),
  or close ISS-060 in favour of ISS-063.
- **ISS-061** ("Hybrid empty cascade"): same story — Hybrid itself is
  correct; the empties come from sub-plans whose downgrades aren't
  routed. Re-target as the "Hybrid sub-plan outcome propagation" line
  item (§Scope item 4).

## Resolution Plan (locked 2026-04-28 by RustClaw)

After re-reading design §3.4, §4.7, §6.4 and the plan source comments,
the canonical fallback target is **Associative** for every plan. This
resolves a documentation conflict:

- `episodic.rs` header comment says "Episodic → Factual" (legacy intent)
- design §6.4 outcome table says "Episodic plan downgraded to Associative"

§6.4 is treated as source of truth (it defines the public contract). The
episodic.rs comment will be updated to match. Rationale: Associative is
the v0.2 known-good baseline, depth=1 fallback to a single target is
simple and predictable, and avoids transitive fallback chains.

### Final scope (in this issue)

1. **`RetrievalOutcome::EmptyResultSet { reason }`** — new variant for
   the case where even the Associative fallback returns nothing. Replaces
   the `if scored.is_empty() { Ok } else { Ok }` dead code in Hybrid and
   the silent-empty paths in single plans.

2. **`RetrievalOutcome::DowngradedFromFactual { reason }`** — new variant
   to fill the §6.4 table gap (§3.4 explicitly mentions Factual→Associative
   but no outcome variant existed).

3. **Orchestrator implements 4 fallback edges (depth=1):**
   - Factual: empty `scored` (NoEntityFound / EntityFoundNoEdges) → run
     Associative, return `DowngradedFromFactual`.
   - Episodic: empty `scored` or `DowngradedFromEpisodic` outcome → run
     Associative, return `DowngradedFromEpisodic`.
   - Abstract: `DowngradedL5Unavailable` or empty `scored` → run
     Associative, return `DowngradedFromAbstract`.
   - Affective: `DowngradedNoSelfState` → run Associative, return
     `NoCognitiveState` (preserve existing tag, add fallback results).

4. **Hybrid bug fix (NOT sub-plan fallback):** the dead code branch
   `if scored.is_empty() { Ok } else { Ok }` becomes
   `if scored.is_empty() { EmptyResultSet { reason: HybridAllSubPlansEmpty } }`.
   Sub-plans inside Hybrid do **not** trigger fallback (out of scope —
   filed as separate diagnostic against ISS-061).

5. **Test coverage:** one test per fallback edge confirming
   (a) outcome is the correct `Downgraded*` variant, (b) `scored` is
   non-empty when Associative has results, (c) `EmptyResultSet` when
   Associative is also empty.

6. **Update `episodic.rs` doc comment** to say "Associative" instead of
   "Factual" so code and design agree.

### Out of scope (deferred)

- Hybrid sub-plan fallback (would require running Associative inside
  HybridDispatchExecutor and feeding RRF a non-empty Associative vector
  per failed sub-plan). ISS-061 is independent — its 0-candidate
  behaviour likely lives in `hybrid_to_scored` ID-mapping, not fallback
  routing.
- Fallback chains of depth > 1 (Associative is terminal).
- LLM-driven re-classification on downgrade.

