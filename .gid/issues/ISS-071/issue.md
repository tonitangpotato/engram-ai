---
id: ISS-071
title: "Affective plan short-circuits to no_cognitive_state — orchestrator never threads self_state"
status: open
priority: P1
filed: 2026-04-29
filed_by: rustclaw
labels: [retrieval, affective, orchestrator, cognitive-state, locomo, evaluation]
relates_to: [ISS-070]
source: RUN-0006
---

# Affective plan downgrades to NoCognitiveState every query — self_state never threaded

## Summary

RUN-0006 per-outcome telemetry shows that every query routed to the
Affective plan exits with outcome `no_cognitive_state` — the plan's
own `DowngradedNoSelfState` short-circuit (`affective.rs:14–18`,
"When `None` the plan surfaces `AffectiveOutcome::DowngradedNoSelfState`
— Associative routing is the orchestrator's concern (§3.4)").

The plan is *correctly* implementing GUARD-6 / GOAL-3.14 (never
block, downgrade gracefully when `self_state: Option<SomaticFingerprint>`
is `None`). But `Some(_)` is never passed in. The orchestrator
never threads cognitive self-state into `AffectivePlanInputs`, so
mood-congruent ranking — the entire reason the plan exists —
never runs in production retrieval. Affective queries fall back
to Associative, losing the affect signal.

## Evidence

- **RUN-0006.md** outcome distribution: every Affective dispatch
  hits `no_cognitive_state`.
- **plans/affective.rs:14–18** — module docstring confirms `None`
  is handled by *downgrading*, not by failing.
- The orchestrator code path that builds `AffectivePlanInputs`
  needs a `self_state` source. There isn't one wired in.

## Why this matters less than ISS-070 (but still matters)

LoCoMo cat=1 multi-hop is at 0% — that's the headline regression
and the P0 (ISS-070). Affective is P1 because:

- It does **not** drop results — it just removes the affect
  re-ranking signal. The Associative fallback still returns
  candidates.
- The hit@5 impact on LoCoMo conv-26 is bounded: most of conv-26
  is factual / temporal / multi-hop, not mood-congruent recall.
- It is **structural debt** rather than a correctness bug — the
  plan works correctly when given input; the input is missing.

It still matters because:

- The cognitive self-state module exists (somatic fingerprint
  storage is implemented). Plumbing is the missing piece.
- GOAL-3.8 (Kendall-tau divergence ≥ 0.1 between neutral and
  current self-state) **cannot be measured** if `self_state` is
  always `None`. The acceptance test is silently un-runnable.
- Anything downstream that expected "Affective re-ranking is
  active in production" is wrong. Worth flagging before more
  features depend on it.

## Hypothesis (root cause)

When the Affective plan was implemented (v03-retrieval), the
self-state input was modeled as `Option<SomaticFingerprint>` to
let tests pass `None` cheaply. The production wiring — orchestrator
fetches current `s_now` from the cognitive-state module, passes it
through `GraphQuery → AffectivePlanInputs` — was never closed.
This is a classic "the type allows it, so the bug is invisible"
case. `None` is a valid value, so the compiler is happy, so it
never got fixed.

## Fix sketch

1. Identify the cognitive-state read API (where `s_now` lives).
2. In the orchestrator's Affective dispatch path, fetch `s_now`
   and populate `AffectivePlanInputs::self_state = Some(s_now)`.
3. Add a regression test: in a substrate with non-zero somatic
   fingerprint storage, an Affective query produces outcome
   `Stored`/`Ranked` (not `NoCognitiveState`) and `affect_divergence`
   is populated when `query.explain = true`.
4. Decide policy for "no current self-state available": the
   downgrade-to-Associative path is fine, but should be a real
   business case (cold start, no recent affective signal), not the
   default state of every production query.

## Acceptance

- Orchestrator threads `self_state` into Affective plan when
  available; downgrade path becomes the *exception* not the rule.
- A LoCoMo benchmark run shows at least one Affective dispatch
  with outcome `Stored` (not `NoCognitiveState`).
- GOAL-3.8 telemetry (Kendall-tau divergence) is measurable from
  RUN-NNNN output.

## Out of scope

- Self-state synthesis logic (deferred to cognitive-state module).
- Choice of `K_seed_affective`, weight tuning — orthogonal.
- Multi-hop traversal (ISS-070).

## References

- `.gid/eval-runs/RUN-0006.md` — outcome distribution.
- `crates/engramai/src/retrieval/plans/affective.rs:1–80` — plan
  docstring + downgrade contract.
- `.gid/features/v03-retrieval/design.md` §4.5 — plan design.
- `.gid/features/v03-retrieval/design.md` §3.4 — orchestrator
  routing including the Affective→Associative downgrade path.
