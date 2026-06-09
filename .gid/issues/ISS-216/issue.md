---
title: 'Heavy ingest-context design: extract_in_context + per-namespace SessionState + rolling summary'
status: open
priority: P2
severity: enhancement
category: ingestion
created: 2026-06-03
relates:
- ISS-162
- ISS-163
- ISS-178
discovered_in: ISS-162 AC rewrite 2026-06-03
---

## Summary

This issue preserves the **heavy ingest-context design** that was the
original AC-1/AC-3 of ISS-162 but was **not** implemented. ISS-162 shipped
a deliberately lightweight mechanism (raw preceding-turn window via
`StorageMeta.context` + `TurnWindow`, no trait change, no session state,
no summary LLM) because that lightweight lever is what the empirical
evidence justified.

The heavy design is **not dead** — it is parked here with explicit trigger
conditions so it is reconsidered only when there is evidence the lightweight
window has plateaued.

## The deferred design (original ISS-162 AC-1 / AC-3)

1. **`Extractor::extract_in_context(ctx: ExtractionContext)`** replacing the
   `extract(text: &str)` trait method. `ExtractionContext` carries:
   - `current` — the new turn
   - `prev` — optional previous turn from the same session
   - `session_summary` — optional rolling summary, max ~500 tokens
   - `recent` — bounded sliding window (default 4)
   - single-string `extract(text)` retained as a thin wrapper.

2. **Per-namespace `SessionState`** holding:
   - rolling summary, compressed every K turns via a cheap summary LLM
     (Haiku @ temp 0)
   - bounded turn queue (default 4)
   - `begin_session` / `end_session` reset semantics.

## Why deferred (not just dropped)

- **Blast radius:** changing the `MemoryExtractor` trait touches **16 impls**.
  That is unjustified "while-I'm-here" growth (karpathy-guidelines) unless the
  payoff is proven.
- **No evidence the *summary* layer helps.** ISS-162's validated lever is
  *raw preceding turns*, not a compressed summary. ISS-178 (slim prev-turn)
  was actively harmful; the win came from a *wider raw window*, not from
  smarter summarisation. There is currently zero evidence a rolling summary
  beats a raw N=4 window.
- **Cost.** A summary LLM call per K turns adds ingest latency + token spend
  with no demonstrated retrieval benefit yet.

## Trigger conditions for promotion (ALL must hold)

1. **Plateau evidence:** raw-window (ISS-162) lift has plateaued — e.g.
   increasing `TurnWindow` capacity past N=4 stops improving single-hop /
   SEMANTIC-GAP on a held-out conv, OR a class of failures is shown to need
   context *beyond the immediate window* (cross-session antecedents).
2. **Cheap probe first:** before any trait change, a fixture-level or
   prompt-only probe shows the summary/long-context source recovers questions
   the raw window cannot — without the ISS-178 pruning regression.
3. **Scope justification:** the 16-impl trait change is justified by the probe
   delta, OR a non-trait-breaking call-site assembly (the ISS-162 pattern,
   extended) can deliver the same context without touching the trait.

## Out of scope

- Anything ISS-162 already shipped (raw window, `TurnWindow`, `StorageMeta.context`).
- Mem0-style ADD/UPDATE/DELETE consolidation — ISS-163.
- Session-end batch re-extraction (Zep paradigm) — strictly more powerful,
  separate follow-up.

## Status

Parked. Do not implement until the trigger conditions above are met. This
issue exists so the design thinking is not lost, not as scheduled work.
