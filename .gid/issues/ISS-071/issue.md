---
id: ISS-071
title: "Affective plan self_state source is misaligned — bench harness never populates interoceptive_hub"
status: open
priority: P2
filed: 2026-04-29
filed_by: rustclaw
revised: 2026-04-29
labels: [retrieval, affective, cognitive-state, locomo, evaluation, semantic-bug]
relates_to: [ISS-070]
source: RUN-0006
---

# Affective plan downgrades to NoCognitiveState every LoCoMo query — self_state source misaligned

## Status of original diagnosis: WRONG

The original ISS-071 (filed 2026-04-29 morning) claimed the
orchestrator never threaded `self_state` into `AffectivePlanInputs`.
That diagnosis was **incorrect**. The plumbing is in place:

- `crates/engramai/src/retrieval/api.rs:479` —
  `self_state_override.or_else(|| self.current_self_state())`
- `crates/engramai/src/retrieval/api.rs:532` — passed to
  `execute_plan` as the `self_state` parameter
- `crates/engramai/src/retrieval/orchestrator.rs:992` — threaded
  into `AffectivePlanInputs::self_state`

The pipe is wired end-to-end. The `None` is coming out of
`Memory::current_self_state()`, not from a missing call.

## Real root cause

`Memory::current_self_state()` (memory.rs:1301) reads from
`self.interoceptive_hub.current_state().to_somatic_fingerprint()`.
`to_somatic_fingerprint()` (interoceptive/types.rs:515–535) returns
`None` when `domain_states.is_empty()` or `total_weight <= 0.0`
(no signals observed yet).

The LoCoMo bench harness (`../engram-bench/`, was `crates/engram-bench/` before 2026-05-02 split) **never calls
any interoceptive_tick API** to feed signals into the hub. Confirmed
by:

```
$ grep -rn "interoceptive_tick\|interoceptive_hub" \
    ../engram-bench/ crates/engramai/examples/
(no matches)
```

So during a benchmark run the hub stays at cold-start, the
fingerprint is `None`, and every Affective dispatch correctly
downgrades. The plan is doing exactly what GUARD-6 / GOAL-3.14
specify: graceful downgrade when self_state is unavailable.

## Why this is a *semantic* bug, not a *plumbing* bug

`Memory::current_self_state()` returns the **agent's own** affective
state — that of the entity running the retrieval system. The
Affective plan's purpose, per `affective.rs:12–14`, is mood-congruent
re-ranking: surface memories whose stored somatic fingerprint
matches the **mood at query time**.

For a chat agent answering its own queries those two things are the
same thing. For LoCoMo replay (and any benchmark that replays
historical user conversations) they are not:

- The "querier" is a synthetic test driver with no genuine affective
  state. Its hub will always be cold.
- The semantically-correct self_state is the **emotional context of
  the LoCoMo question itself** ("how did I feel about X?", "what
  did we enjoy doing?"), inferable from query text.
- Using a (synthetic) cold-start `None` is honest. Using a
  fabricated synthetic fingerprint to "make Affective fire" would
  be telemetry pollution.

So the bug is: **`current_self_state()` reads from the wrong source
for replay/benchmark workloads.** The agent-hub source is right for
live RustClaw operation; it is wrong for LoCoMo.

## Why this is P2 (not P1) for now

- ISS-070 (multi-hop = 0%) is the actual hit@5 regression. This
  one is a missing signal, not a wrong answer — Associative
  fallback still returns candidates.
- Even fixed perfectly, the LoCoMo cat=2/3 lift is bounded: most
  questions in conv-26 are factual / temporal / multi-hop, not
  mood-congruent recall.
- The "wrong" behavior in RUN-0006 (every Affective query →
  `no_cognitive_state`) is **technically correct** given a
  cold hub; it's the design that needs a query-time path, not
  a hotfix to force `Some(_)` into the slot.

## What's actually broken (acceptance criteria)

1. There is no documented or stable mechanism for a benchmark
   harness to inject query-time self_state without polluting
   the agent's interoceptive hub.
2. RUN-NNNN telemetry currently labels these dispatches under
   `outcome=no_cognitive_state` mixed in with genuine plan
   failures, making it impossible to distinguish "plan skipped
   because hub is cold" from "plan ran and produced nothing".
3. GOAL-3.8 (Kendall-tau divergence ≥ 0.1 between neutral and
   current self-state) is unmeasurable from any current
   benchmark configuration.

## Fix sketch (deferred — not committing to design here)

Two paths, both legitimate:

**Path A — query-context override (small, mechanical):**
Use the existing `GraphQuery::with_self_state_override(...)` API
(api.rs:467). Have the LoCoMo driver compute a per-query synthetic
fingerprint deterministically from query text or fixture metadata,
inject it via override. Document that this is benchmark-synthetic.
Telemetry stays honest because the override flag is recorded.

**Path B — first-class query mood (real fix):**
Add a `query.context_self_state` field with a documented inference
function (text → fingerprint). Wire `current_self_state()` to prefer
query-context over hub when both are present. This is closer to the
mood-congruent retrieval literature and would also help live agents
where "the user just typed something angry" matters more than "the
agent's accumulated valence trend".

Path A is a one-day chore. Path B is a small design + couple of
days of work. Neither is urgent.

## Out of scope

- Multi-hop traversal (ISS-070, P0).
- Restructuring telemetry outcomes — separate, smaller issue if
  needed.
- Touching the agent-hub path for live RustClaw.

## References

- `.gid/eval-runs/RUN-0006.md` — outcome distribution showing
  `no_cognitive_state` saturation.
- `crates/engramai/src/retrieval/api.rs:467–485` — existing
  `with_self_state_override` and `current_self_state` fallback.
- `crates/engramai/src/memory.rs:1295–1340` —
  `current_self_state()` reading from interoceptive hub.
- `crates/engramai/src/interoceptive/types.rs:515–535` —
  `to_somatic_fingerprint()` returning None on cold hub.
- `crates/engramai/src/retrieval/plans/affective.rs:1–80` — plan
  contract; downgrade behavior is correct.
- `.gid/features/v03-retrieval/design.md` §3.4, §4.5 — orchestrator
  routing and Affective plan design.
