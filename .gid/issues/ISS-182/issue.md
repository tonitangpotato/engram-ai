---
title: B-bucket activation — Emotional bus: not consumed by retrieval/generation; bench never populates affective state
status: open
priority: P3
severity: feature-inert
category: cognitive-substrate
created: 2026-05-28
relates: [engram:ISS-181, engram:ISS-071]
relates_to: .gid/issues/ISS-181/issue.md
discovered_in: ISS-181 cognitive feature coverage matrix
---

## Summary

The emotional bus (`bus/`) compiles, has passing unit tests
(`bus_test`, `iss_090_empathy_bus_compat`), and its `EmotionalEvent`
type is wired into the affective subsystem. But:

- LoCoMo bench never injects bus events during ingestion or query
- The affective plan reads bus state but always sees an empty bus
- No A/B sweep has ever isolated the bus's contribution to
  retrieval or generation
- The substrate-level marketing claim "emotion + interoception"
  in README has zero verifiable production behavior backing it

This is the "wired-but-inert" bucket-B pattern from ISS-181.

## What it would take to make the bus production-active

Two consumer-side changes + one bench-side change:

1. **Bench-side: inject emotional events during ingest.**
   LoCoMo episodes carry implicit affect (e.g. q7 "tough breakup",
   q11 "stressful relocation"). Add an emotional-event extractor
   (heuristic or small LLM call per episode) that calls
   `bus.publish(EmotionalEvent::...)` during
   `engram-bench/src/drivers/locomo.rs::replay_conversation`.
   Without this, no production-realistic bus state exists in any
   bench artifact.

2. **Retrieval-side: affective plan reads bus, must affect ranking.**
   Today the affective plan path exists but its fusion weight
   contribution is unverified. Either:
   - (a) Wire a `bus_recency_score` channel into Factual/Hybrid
     fusion that decays affect over time and amplifies memories
     co-occurring with active emotional context, OR
   - (b) Drop the affective-plan path entirely and re-route those
     queries through hybrid.
   Pick one; current state ("plan exists, doesn't move metrics")
   is worse than either.

3. **Generation-side (optional): affect-aware answer prompt.**
   For LoCoMo emotional-category questions (n=24 on conv-26),
   condition the generation prompt on current bus state if
   non-empty. Hypothesis: better recall of emotionally-grounded
   memories when the prompt cues the right valence.

## Acceptance criteria (if/when this issue is taken on)

- AC-1: Bench injects ≥1 `EmotionalEvent` per ingested episode on
  LoCoMo conv-26 (verify via bus log dump).
- AC-2: A/B sweep with bus injection off vs on shows non-zero Δ
  on the emotional category. Direction matters less than
  non-zero (this proves the bus state actually moves the system,
  not whether it improves it).
- AC-3: A clear next-step issue filed: either (a) optimize the
  injection to lift the metric, or (b) ship the bus as
  default-OFF with documented reasoning.

## Why this is P3, not P2

Sequencing: ISS-179 AC-5a redefine still pending. Bus activation
on a corpus that has explicit emotional ground-truth would be
more valuable than bus activation on LoCoMo, where emotional
category accuracy is dominated by single-fact patterns the bus
doesn't address.

Reopen at P2 when:
- ISS-179 lands and emotional-corpus benchmarking is on the
  roadmap, OR
- A user-facing product (rustclaw / agentctl) ships an emotion-
  conditioned interaction loop and needs the bus to do real
  work.

## Linkages

- Parent audit: ISS-181 §B bucket
- Companion: ISS-071 (interoceptive hub — same bucket-B pattern)
