---
title: "Retrieval Downgrade-Contract: cross-cutting problem writeup"
status: open
discovered: 2026-04-28
discovered_by: rustclaw
labels: [retrieval, orchestrator, root-cause, design-contract]
root_fix: ISS-063
observability: ISS-062
superseded: [ISS-060, ISS-061]
design_refs: ["v03-retrieval/design.md §3.4", "v03-retrieval/design.md §6.4", "v03-retrieval/design.md §4.7"]
---

# The Retrieval Downgrade-Contract Problem

## What this document is

This is the **umbrella writeup** for a cross-cutting problem in v0.3
retrieval. It exists because the bug surfaced as 2–3 different
"plan-specific" symptoms (Abstract returns 0, Hybrid returns 0,
`outcome=ok candidates=0` showing in logs), each of which got its own
issue filed before we realised they were all the same root cause.

If you're triaging this problem, **start at the root-fix issue
[ISS-063](../issues/ISS-063/issue.md)**. Everything below explains why
ISS-063 is the right place to fix it, and how the other issues relate.

## The one-paragraph version

The retrieval design (§3.4, §6.4) says: when a plan can't run because
its preconditions aren't met, it emits a `Downgraded*` outcome and the
**orchestrator routes the query to a designed fallback plan**
(Abstract→Associative, Episodic→Factual, Affective→…). The plans
correctly emit those downgrade outcomes today. **The orchestrator never
implements the routing.** Downgrades are translated 1:1 into
`RetrievalOutcome::Downgraded*` and returned to the API caller with
empty `candidates`. This silently fails on every query whose intent
can't satisfy its plan's preconditions.

## Why we're writing this up separately from the issues

The issue tracker captured the *symptoms* in the order we discovered
them, not the *causal structure*:

- **ISS-060** (filed first) blamed the Abstract plan's downgrade chain.
- **ISS-061** (filed next) blamed Hybrid's sub-plan handling.
- Investigation showed both are **downstream** of the same orchestrator
  contract violation. ISS-063 was filed as the actual root fix.
- **ISS-062** is the observability companion: the reason it took two
  mis-filings to find the root cause is that logs collapsed three
  distinct outcomes into a single `outcome=ok candidates=0` line.

This doc is the place to point a new contributor (or a future autopilot
session) so they don't re-derive the causal chain from four issue files.

## Pointer table

- **Root fix** → [`ISS-063`](../issues/ISS-063/issue.md) — implement the
  downgrade-to-fallback contract in the orchestrator. Required for any
  query that currently routes to a plan with unmet preconditions.
- **Observability prerequisite** → [`ISS-062`](../issues/ISS-062/issue.md)
  — distinguish `stub-empty` vs `designed-downgrade-empty` vs
  `real-empty` in logs. Lands first so ISS-063 is verifiable.
- **Superseded by ISS-063** → [`ISS-060`](../issues/ISS-060/issue.md)
  (Abstract → 0), [`ISS-061`](../issues/ISS-061/issue.md) (Hybrid → 0).
  Their `superseded_by` frontmatter points at ISS-063. Keep them open
  as sub-tasks of ISS-063 or close on fix; do not fix in isolation.

## Root-cause investigation (causal chain)

The investigation is laid out in full in ISS-063 §Evidence trail. This
section gives the same chain in narrative form so reviewers can follow
the reasoning end-to-end without bouncing between files.

### Step 1: Symptom — empty results from non-error queries

In the LoCoMo conv-26 retrieval smoke (post ISS-055/056/058 fixes,
namespace `conv26`, 192 entities + 140 edges in the graph DB),
4 queries out of 25 returned `candidates=0` with no error signal:

```
plan=Abstract  cat=4 got=0 outcome=downgraded_from_abstract  (×2)
plan=Abstract  cat=5 got=0 outcome=downgraded_from_abstract  (×1)
plan=Abstract  cat=1 got=0 outcome=downgraded_from_abstract  (×1)
plan=Hybrid    cat=5 got=0 outcome=ok                        (×1)
plan=Hybrid    cat=4 got=0 outcome=ok                        (×1)
```

Two distinct outcome strings, but the same observable failure: caller
gets nothing back.

### Step 2: First hypothesis — plans are buggy (wrong)

Initial reading of the Abstract case suggested the Abstract plan was
mis-handling its downgrade chain (filed as ISS-060). Initial reading of
the Hybrid case suggested Hybrid was a silent stub that returned `Ok`
without doing anything (filed as ISS-061).

Both hypotheses turned out to be **wrong** when we read the plan
implementations:

- `plans/abstract_l5.rs` (`AbstractPlan::execute`, the
  `topic_search`-empty and `below-threshold` branches) — Abstract
  correctly emits `AbstractOutcome::DowngradedL5Unavailable` when the
  topic searcher returns nothing or scores fall below threshold. The
  module doc explicitly says *"orchestrator routes to Associative per
  design §3.4"*. The plan is a leaf executor; routing isn't its job.
- `plans/episodic.rs` (`EpisodicPlan::execute`, the
  `time_window.is_none()` branch; outcome enum at
  `EpisodicOutcome::DowngradedFromEpisodic`) — Episodic returns
  `DowngradedFromEpisodic` with empty memories when `time_window` is
  unset, with the variant comment *"caller should re-dispatch as
  Factual"*. Same pattern.
- Hybrid's `HybridDispatchExecutor::run` is **fully implemented** as an
  RRF aggregator (see `plans/hybrid.rs::fuse_rrf` plus the executor
  block in `orchestrator.rs::execute_plan`'s `Intent::Hybrid` arm). It
  executes its sub-plans, fuses results, and returns. It is not a stub.

So both filed issues mis-located the bug. The plans behave per their
contract; something downstream is breaking.

### Step 3: Locate the contract violation in the orchestrator

`orchestrator.rs::execute_plan` is where plan outcomes are converted
to `RetrievalOutcome` for the API response. Three findings (referenced
by match-arm name rather than line number — line numbers rot the
moment anyone refactors this file):

1. **`Intent::Abstract` arm**: matches on `AbstractOutcome`,
   translates `DowngradedL5Unavailable` 1:1 to
   `RetrievalOutcome::DowngradedFromAbstract` via
   `AbstractOutcome::to_retrieval_outcome`, and returns. **It does
   not call the Associative plan.** Design §3.4 says it must.

2. **`Intent::Episodic` arm**: identical pattern. Calls
   `result.outcome.to_retrieval_outcome(scored.is_empty())` and
   returns. **It does not call the Factual plan.** Episodic's own
   variant comment says the caller (= the orchestrator) should
   re-dispatch as Factual.

3. **Hybrid sub-plan outcome dropping** (`Intent::Hybrid` arm,
   `SubPlanResult` defined in `plans/hybrid.rs`): `SubPlanResult` has
   only `kind` + `items`, no `outcome` field. A sub-plan that
   downgrades looks identical to one that found nothing. RRF sees
   only empty `items` from each downgraded sub-plan and produces 0
   fused candidates. The top-level Hybrid arm then computes its
   outcome from the *fused* `ranked` length rather than from
   sub-plan outcomes — so two downgraded sub-plans + zero ranked
   items present as a clean `RetrievalOutcome::Ok` (or `Empty`) at
   the top level rather than a downgrade signal. That's why ISS-061
   saw `outcome=ok candidates=0` with no breadcrumb pointing at the
   sub-plan downgrades that caused it.

This is the contract violation. The plans emit what the design said
they should emit. The orchestrator never honours the routing edges.

#### Why it stayed unimplemented (structural, not just a missing if)

It's tempting to read this as *"someone forgot a few `if` statements
in `execute_plan`"*. The truer reading: the type system the design
produced makes routing the awkward path and translation the easy
path. Two reinforcing shapes:

- **`execute_plan` returns `(Vec<ScoredResult>, RetrievalOutcome)`,
  not a re-dispatchable continuation.** Each plan arm builds its own
  scored vec from typed loaders (factual/episodic/abstract). A
  fallback isn't a one-liner like `if downgraded { execute_plan(Factual) }`
  — the inputs to each plan are typed differently (Episodic needs
  `time_window`, Factual needs query embeddings, Abstract needs
  topics). The fallback contract was never designed *into* the
  function signature.
- **`to_retrieval_outcome` is the wrong abstraction boundary.** Each
  plan-local outcome enum (`EpisodicOutcome`, `AbstractOutcome`,
  `FactualOutcome`) carries a `to_retrieval_outcome()` method that
  lifts plan-local state directly into the public `RetrievalOutcome`
  surface. This makes "translate then return" the path of least
  resistance and makes "translate then re-dispatch" require
  restructuring. The very existence of `to_retrieval_outcome` as a
  method on each plan outcome is what shaped the orchestrator into a
  translator instead of a router.

This matters for the fix: ISS-063 should not just sprinkle routing
into the existing arm shape — see §What "fixed" looks like for the
shallow-vs-deep choice.

### Step 4: Why this stayed hidden until now

Two reinforcing reasons:

- **Observability conflation (ISS-062).** The logger writes
  `outcome=ok candidates=0` for both "stub returned empty" and
  "real run, nothing found", and writes `outcome=downgraded_*` without
  indicating that a fallback was supposed to run. From the log alone
  you can't tell that ISS-063 was the bug; both ISS-060 and ISS-061
  were filed against the wrong layer. Note that ISS-062's framing
  needs to cover **four** empty-states, not three: stub-empty,
  designed-downgrade-empty, real-empty, and (post-ISS-063)
  fallback-eligible-empty — the case where a fallback *did* run and
  legitimately found nothing. Without naming the fourth state up
  front, ISS-062 will need a v2 the moment ISS-063 lands.
- **`plan_used` actively lies.** `api.rs::graph_query` sets
  `plan_used = intent` (the *requested* intent, verbatim). The public
  doc comment on the field says it *"may differ from `query.intent`
  after `Downgraded*`"* — i.e. the API documents a contract that the
  implementation breaks. This is worse than "dead documentation";
  external callers (and benchmarks) who trust the field are getting
  silently wrong telemetry. ISS-063 needs to fix the implementation
  *or* fix the doc comment, not just leave the contradiction.

### Step 5: Why a patch isn't enough

The tempting patch is: tweak the intent classifier so queries without
a `time_window` don't route to Episodic, queries without a topic don't
route to Abstract, etc. This hides the symptom for *some* queries
without fixing the contract:

- Any query the classifier still routes to Episodic with no time anchor
  remains broken.
- Any query routed to Abstract when L5 has no topics in the namespace
  remains broken.
- Any query routed to Affective without self-state remains broken.
- The classifier becomes the de facto routing table, which is exactly
  what the design tried to avoid by putting fallback edges in the
  orchestrator.

The root fix is to implement the routing the design already specifies.
That's ISS-063.

## What "fixed" looks like

From ISS-063 §Acceptance, restated for cross-reference:

- The 4 conv-26 queries currently logging `outcome=ok candidates=0`
  either:
  - return non-empty results from the fallback plan, with
    `plan_used = <fallback intent>` and outcome carrying
    `fallback_plan_used`, OR
  - return `RetrievalOutcome::Empty` if the fallback also has nothing.
- New unit tests cover each fallback edge (Abstract→Associative,
  Episodic→Factual, Affective→…).
- Hybrid sub-plan results carry an `outcome` field; downgraded
  sub-plans are replaced with their designed fallbacks before RRF runs.
- Existing tests asserting `plan_used == intent` on non-downgraded
  paths stay green.

### Shallow fix vs deep fix (ISS-063 needs to decide before starting)

Given the structural cause noted in Step 3, there are two viable
shapes for the fix:

- **Shallow fix.** Inside each `execute_plan` arm, after computing a
  downgraded outcome, recursively call into the fallback intent's arm
  (Abstract → Associative, Episodic → Factual, Affective → Factual).
  Easy to implement, leaves `to_retrieval_outcome` and the arm shape
  intact, but keeps the ambient pressure that produced this bug in
  the first place. Future plans will face the same trap.
- **Deep fix (recommended).** Change each plan arm to return a
  `PlanResolution { outcome, scored, fallback_request: Option<Intent> }`
  rather than `(Vec<ScoredResult>, RetrievalOutcome)` directly. A
  thin orchestrator loop drives at most one fallback hop and only
  *then* calls the equivalent of `to_retrieval_outcome` on the
  terminal resolution. This makes routing the obvious path and
  removes the temptation to translate-and-return. Roughly the same
  blast radius as the shallow fix but reshapes the contract once.

ISS-063 should pick one explicitly in its design phase rather than
drift into shallow because it's faster to type.

## Open questions (not blockers, raised in ISS-063)

- §4.7 says "RRF absorbs sub-plan empties." Is that intended to cover
  downgrades, or did §4.7 assume sub-plans never downgrade because
  Hybrid only picks them when preconditions hold? Confirm with design
  intent before implementing the Hybrid sub-plan fallback.
- `RetrievalOutcome` enum gets a new optional `fallback_plan_used`
  field. Pre-1.0, but external benchmarks consume the enum — proceed
  additively.
- Recursive downgrades: if the fallback itself fails, do we cascade?
  Recommendation in ISS-063: no, stop at depth 1. Worth confirming.

## Suggested execution order

1. **ISS-062** (observability) lands first. Without it, the fix is
   hard to verify because the log lines that triggered ISS-060/061
   don't disambiguate the three empty-states.
2. **ISS-063** (root fix). Closes ISS-060 and ISS-061 by implication;
   their issues should be closed-as-superseded once ISS-063 lands and
   the conv-26 smoke confirms the four affected queries behave per
   §Acceptance.
3. **ISS-064** (suggested in ISS-063, not yet filed): NL→TimeWindow
   heuristic so queries like *"after the charity race"* can populate
   `time_window` upstream of Episodic. Feature work, separate from the
   contract fix.

## Cross-references

- Issue: [ISS-063 — root fix](../issues/ISS-063/issue.md)
- Issue: [ISS-062 — observability prerequisite](../issues/ISS-062/issue.md)
- Issue: [ISS-060 — superseded](../issues/ISS-060/issue.md)
- Issue: [ISS-061 — superseded](../issues/ISS-061/issue.md)
- Design: `.gid/features/v03-retrieval/design.md` §3.4 (outcome surface
  and fallback graph), §4.7 (Hybrid plan), §6.4 (`RetrievalOutcome`
  enum and contract with `plan_used`).
- Smoke run: `.gid/issues/_smoke-locomo-2026-04-27/` (conv-26 baseline
  before fix).
