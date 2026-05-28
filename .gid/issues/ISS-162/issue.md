---
title: 'Ingest path: extractor sees single turn, missing conversation context window'
status: open
priority: P3
severity: architecture-gap
category: ingestion
created: 2026-05-26
relates:
- ISS-148
- ISS-161
- ISS-163
- ISS-164
discovered_in: ISS-161 root-cause audit 2026-05-26
downgrade_reason:
- ISS-178 slim-prev-turn falsification — slim variant actively harmful on conv-26 (Δsh −2
- reg 11.2%
- q3 no flip). Full session-state design needs different justification before reprioritising.
downgraded_at: 2026-05-28
---

## Summary

The extractor receives one conversation turn at a time with no
surrounding context. Noun phrases that the speaker references
implicitly (relying on the previous turn's question or earlier
session content) are lost at extraction time and cannot be recovered
by any downstream retrieval / reranking / prompt-engineering work.

This is the **largest single contributor** to ISS-148 AC-5a
single-fact failures on conv-26 (≈5 of 9 missing questions per
ISS-161 root-cause audit).

## Code-layer evidence

**Single-turn ingestion path** (verified 2026-05-26 against current
working tree, commit `5adf83e` baseline):

1. `engram-bench/scripts/build_locomo_fixture.py:99-103` — fixture
   builder emits **one episode per turn**:
   ```python
   for turn in conv[sk]:
       episodes.append({
           "text": f"{turn['speaker']}: {turn['text']}",
           "occurred_at": ...,
       })
   ```

2. `engram-bench/src/drivers/locomo.rs:919` — replay calls
   `memory.ingest_with_stats_at(&episode.text, episode.occurred_at)`
   per episode, no batching.

3. `crates/engramai/src/memory.rs:7267` — `ingest_with_stats_at`
   wraps `store_raw(content, meta)`, single content string.

4. `crates/engramai/src/memory.rs:3382` — `store_raw` calls
   `extractor.extract(content)`. Single `&str` argument.

5. `crates/engramai/src/extractor.rs:480-495` — `extract` POSTs
   `prompt + text` to Anthropic Messages API. **No history, no
   previous-message context, no session summary.**

```rust
fn extract(&self, text: &str) -> Result<Vec<ExtractedFact>, ...> {
    let prompt = format!("{}{}", select_extraction_prompt(), text);
    // ... POST { "messages": [{"role": "user", "content": prompt}] }
}
```

## Comparable systems

**Mem0** (Chhikara et al., arXiv:2504.19413, 2025-04) extractor sees:
- `m_t` (current message)
- `m_{t-1}` (previous message — provides question / antecedent)
- `S` (running session summary, updated each turn)
- `recent_m` (configurable sliding window, default 4 messages)

**Zep** batches at session-end: extractor sees the entire session as
one document.

**LangMem** uses ChatHistory window: typically last 5-10 messages.

## Concrete failure example (conv-26 q3)

LoCoMo gold for q3 "What did Caroline research?" → "Adoption
agencies". The relevant conv-26 exchange:

```
Melanie: ... what are you researching?
Caroline: Adoption agencies, mainly. I've been thinking about
          fostering for a while now ...
```

These are two consecutive turns. The current ingest path creates two
independent episodes:

- Episode 1: `"Melanie: ... what are you researching?"`
- Episode 2: `"Caroline: Adoption agencies, mainly. I've been..."`

Extractor on Episode 2 in isolation sees `"Adoption agencies,
mainly. I've been thinking about fostering ..."` with no antecedent
for "what". The extracted memory becomes vague:
- `core_fact: "Caroline is thinking about fostering"` (loses
  "adoption agencies" as the research target — the noun phrase is
  there but not anchored to "research").

If the extractor saw `(Episode 1 + Episode 2)`, it would extract:
- `core_fact: "Caroline is researching adoption agencies"`

The same pattern explains q11 ("Where did Caroline move from?" →
"Sweden"), q43 ("What kind of art does Caroline make?"), q71 ("What
book did Melanie read from Caroline's suggestion?" → "Becoming
Nicole").

## Acceptance criteria

**AC-1 (mechanism)**: Extractor accepts a context bundle, not a
single string. Concretely: replace
`Extractor::extract(text: &str)` with
`Extractor::extract_in_context(ctx: ExtractionContext)` where
`ExtractionContext` carries at minimum:
- `current` — the new turn (what we're extracting from)
- `prev` — optional previous turn from the same session
- `session_summary` — optional rolling summary, max ~500 tokens
- `recent` — bounded sliding window of last N turns (configurable,
  default 4)

The existing single-string API remains as a thin wrapper
(`extract(text)` = `extract_in_context(ExtractionContext::from_text(text))`)
for non-conversational ingest paths.

**AC-2 (ingest wiring)**: `store_raw` learns an optional
`ContextHint` argument. Conversational drivers (LoCoMo, future chat
sessions, future Slack/Discord ingest) populate it. Non-conversational
callers omit it and behave exactly as today.

**AC-3 (session state)**: Memory holds a per-namespace `SessionState`
that maintains:
- The rolling summary (compressed every K turns via a cheap summary
  LLM call — Haiku at temperature 0)
- A bounded turn queue (default 4)
- Reset semantics (`begin_session` / `end_session` API)

**AC-4 (LoCoMo measurement)**: Re-run conv-26 K=10 temp=0 HyDE=off
with `ContextHint = (prev, session_summary)`. Target: single-fact
sub-bucket ≥ 11/27 = 0.41 (a +3-question lift over current best
8/27).  This is the **per-issue acceptance gate**; the AC-5a 0.60
target lives on ISS-148.

**AC-5 (no regression)**: Cross-validate on conv-44 (the other
ISS-160 anchor conversation). Single-fact and list buckets must not
regress more than 1 question vs current state.

## Out of scope

- **Mem0-style ADD/UPDATE/DELETE consolidation** — separate concern,
  tracked in ISS-163. The two can ship independently; this one is the
  "give the extractor enough information" layer, that one is the
  "reconcile multiple extracted versions" layer.
- **Entity-aware retrieval routing** — ISS-164. Even with perfect
  extraction, the classifier still bypasses Factual plan.
- **Session-end batch re-extraction** (Zep paradigm) — strictly
  more powerful than sliding window but much more expensive and
  doesn't fit the streaming chat use case. If sliding window
  underperforms, that's a follow-up.

## Estimated effort

2-3 weeks. New `ExtractionContext` type + session-state plumbing +
LoCoMo driver wiring + summarisation logic + bench validation.
Touches `engramai` ingest path + `engram-bench` LoCoMo driver.

## Expected lift

Per ISS-161 audit: 5 of 9 currently-missing single-fact questions
have the gold noun phrase in the **previous turn**. If those 5 are
recovered, single-fact on conv-26 reaches 13/27 = 0.48 — still below
AC-5a 0.60 but a 16pp lift, the largest single architectural lever
identified.

---

## DOWNGRADE — 2026-05-28 (P1 → P3)

ISS-178 was filed as the **minimum viable subset** of this issue — prev-turn
only, no rolling summary, no session-state machinery — specifically to test
whether the "extractor needs more context" hypothesis held empirically before
investing in the full design.

Conv-26 A/B sweep result (`.gid/issues/ISS-178/artifacts/falsification-conv26-20260528.md`):

- Overall Δ **−1.97 pp**
- Single-hop Δ **−6.25 pp** (4/32 → 2/32)
- Open-domain Δ **−15.38 pp**
- q3 (the canonical "prev-turn-fixable" question per ISS-161 audit) — **no flip**
- Regression rate **11.2 %** (AC-4 guard fail)

Sample regressions show the slim prev-turn context **prunes useful co-occurring
facts** the long-window extractor would otherwise keep (e.g. q15 lost 3 of 4
Melanie hobbies). The expected lift (5 of 9 single-fact misses fixable per the
above audit) **did not materialise**.

The fuller design here (`SessionState`, rolling summary, sliding window) is
strictly more aggressive than what ISS-178 tested — there is no evidence the
delta against ISS-178's failure mode is positive. Until a different mechanism
is proposed (e.g. structured retrieval pre-pass over the current namespace to
pull anchor entities **without** discarding extractor scope), this issue
should not be implementation-priority.

**Hold criteria for re-promotion to P1/P2:**

1. A different context source is proposed (not just prev-turn text); AND
2. A cheap probe (extractor-prompt-only or fixture-level) shows the proposed
   source does not exhibit the ISS-178 pruning pattern; AND
3. ISS-179 AC-5a target redefine (Options A/B/C/D) is resolved — if Option C
   is taken (move SF target off conv-26), the conv-26-derived motivation for
   this issue weakens.
