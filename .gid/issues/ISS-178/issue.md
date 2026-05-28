---
title: Slim prev-turn ExtractionContext (subset of ISS-162) — minimum lever to fix conv-26 q3-style noun-phrase drop
status: open
priority: P2
severity: extraction-fidelity
category: extraction
created: 2026-05-28
relates:
- engram:ISS-162
- engram:ISS-148
- engram:ISS-161
relates_to: .gid/issues/ISS-162/issue.md
---

## Summary

Spin off a *minimum-viable* subset of ISS-162 ("extractor sees single
turn, missing conversation context") that does ONLY what evidence
supports: pass the previous turn's text alongside the current turn into
the extractor. No `SessionState`, no rolling Haiku summary, no per-namespace
queue. Just `ExtractionContext { current: String, prev: Option<String> }`.

## Why a slim version

The ISS-161 audit hand-mapped all 9 single-fact misses on conv-26
(`.gid/issues/ISS-161/artifacts/heartbeat-1240-real-failure-modes.md`):

| qid | failure mode | prev-turn-fixable? |
|---|---|---|
| q3  | extractor dropped "adoption agencies" noun phrase from "Researching adoption agencies" (no question-noun anchor) | **YES** — prev turn is the question that names the noun |
| q7  | "tough breakup" → "significant life change" abstraction | maybe — prev turn = "tell me about your love life" |
| q11 | cross-session synthesis (D3:13 + D4:3) | NO — needs graph traversal (ISS-070) |
| q37 | BLIP image caption missing | NO — needs BLIP ingestion |
| q40 | counting across episodes | NO — aggregation, not extraction |
| q43 | aggregation across 3 painting episodes | NO — same |
| q71 | fact present in top-K, LLM said "not mentioned" | NO — generation failure |
| q75 | counting (kids/brother refs) | NO — aggregation |
| q76 | "did it yesterday" → date binding | NO — temporal grounding |

**Honest count**: 1 cleanly fixable (q3), 1 maybe (q7). The full ISS-162
session-state machinery (summary, queue, SessionState type, etc.) is not
supported by this evidence — those would be infrastructure for a hypothetical
broader use case not visible in conv-26.

The slim version is also a falsification probe: if even prev-turn-only
doesn't move q3, the broader ISS-162 hypothesis collapses cheaply.

## Scope

**In:**
- `ExtractionContext { current: String, prev: Option<String> }` type in `engramai`
- Threaded through `AnthropicExtractor::extract` and `OllamaExtractor::extract`
- Extractor prompt change: when `prev` is `Some`, prepend `"Previous turn: {prev}\n\nCurrent turn: {current}"` instead of raw `current`
- `engram-bench/src/drivers/locomo.rs` populates `prev` from the previous conversation turn during ingestion
- Unit tests in `engramai` for the prompt-shape change (3-4 tests covering: prev=None byte-identity, prev=Some prompt structure, multi-line prev handling)
- Bench: conv-26 A/B sweep, focus on q3 + SF aggregate

**Out (deferred to ISS-162):**
- `SessionState` per namespace
- Rolling Haiku summary every K turns
- Window/queue management
- Cross-session context
- Driver-level batched begin/end

## ACs

- [ ] AC-1: `ExtractionContext` type defined in `engramai::extractor`, both extractors (Anthropic + Ollama) accept it, default behavior (`prev=None`) is byte-identical to current
- [ ] AC-2: Unit tests added: `prev_none_byte_identical`, `prev_some_prompt_shape`, `prev_multiline_handled`
- [ ] AC-3: `engram-bench` driver wires the previous turn into `ExtractionContext` during conv-26 ingestion (env-gated `ENGRAM_BENCH_PREV_TURN_CONTEXT=on`, default off for envelope preservation)
- [ ] AC-4: conv-26 A/B sweep (envelope: K=10, temp=0, HyDE=off, MMR=off, entity_channel=off, FACTUAL_REWEIGHT=on per ISS-177 canonical) shows q3 score Δ ≥ 0 (1 → still 1 on B, or 0 → 1 on B). Regression rate ≤10% (AC-3-style guard).
- [ ] AC-5: SF aggregate Δ ≥ +1 question (≥6/27 from 5/27 baseline). If Δ = 0, ISS-178 falsified, lever closed.
- [ ] AC-6: If AC-4 + AC-5 met, decision-tree branch:
  - lift attributable to prev-turn only → ship as default-on
  - lift only with longer prev windows → escalate to full ISS-162

## Estimated effort

3-4 days end-to-end (type + extractor wiring + tests + driver patch +
bench + write-up). Compare ISS-162 full scope: 2-3 weeks.

## Falsification

If AC-5 fails (SF aggregate Δ = 0), the broader ISS-162 hypothesis is
substantially weakened — the audit evidence said prev-turn covers 1-2
questions, and if neither moves, more context probably won't help either.
Close ISS-178 falsified; consider downgrading ISS-162 to P3 or closing.

## Decision pending (before any code)

Whether to start now or block on ISS-179 (AC-5a redefine) outcome first.
The two are coupled — if AC-5a is redefined to "any axis ≥+5pp" or
shifted off conv-26 single-fact specifically, the lever-vs-target math
changes and ISS-178 might or might not still be worth pursuing.

## Related

- ISS-162: full session-state version (this is the slim subset)
- ISS-161 audit: `.gid/issues/ISS-161/artifacts/heartbeat-1240-real-failure-modes.md`
- ISS-148 AC-5a: blocking conv-26 single-fact target
- ISS-179: AC-5a redefine discussion (paired)
