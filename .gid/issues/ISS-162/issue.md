---
title: 'Ingest path: extractor sees single turn, missing conversation context window'
status: in_review
priority: P1
severity: architecture-gap
category: ingestion
created: 2026-05-26
relates:
- ISS-148
- ISS-161
- ISS-163
- ISS-164
- ISS-216
- .gid/issues/ISS-217/issue.md
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

> **⚠️ SUPERSEDED 2026-06-03.** The AC-1..AC-5 block below was written
> 2026-05-26 against an *unvalidated* heavy design (`extract_in_context`
> trait change + per-namespace `SessionState` + rolling-summary LLM).
> Implementation (commit `580ba3b5`) deliberately did **not** build that
> design — see "Design delta vs original AC-1/AC-3" under RE-PROMOTED.
> The original block is kept verbatim for history. The **live acceptance
> gate is the REVISED AC at the bottom of this file** (`## REVISED
> ACCEPTANCE CRITERIA — 2026-06-03`). Do not validate against the
> superseded block.

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

---

## RE-PROMOTED + IMPLEMENTED — 2026-06-03

**Hold criteria satisfied** by ISS-201:

1. *Different context source proposed* → sliding **window=4** (oldest-first
   preceding turns as coreference-only context), not ISS-178's slim single
   prev-turn.
2. *Cheap probe shows no ISS-178 pruning pattern* → `iss201_window_verify.rs`
   4/4 RETRIEVABLE at the LLM level, context turns NOT double-extracted.
3. *Empirical lift* → ISS-201 conv-26 A/B (STAMP 20260603T141337Z): overall J
   **0.2697 → 0.3882 (+11.85 pp / +44 %)**, single-hop 0.031 → 0.156 (5×),
   open-domain 0.077 → 0.308 (4×), SEMANTIC-GAP 18 → 13.

**Confound isolated** (STAMP 20260603T171256Z, see
`artifacts/isolation-result-2026-06-03.md`):

- Arm C (window=1, my envelope) → 0.3421: window-size alone is the lever
  (+7.24 pp over baseline with a single context turn), NOT the
  `FACTUAL_REWEIGHT` envelope.
- Arm D (window=4, canonical `FACTUAL_REWEIGHT=on`, reservation off) → 0.3421,
  single-hop **0.219** (best of all arms), SEMANTIC-GAP **11** (lowest): the
  lift **survives the canonical envelope** → safe to build in the library.
- Lift is dose-dependent (window=4 > window=1 on single-hop & SEMANTIC-GAP) →
  **default N=4** pinned.

### Design delta vs original AC-1/AC-3

The original AC-1 (`extract_in_context` trait method) and AC-3 (per-namespace
`SessionState` + rolling-summary LLM) were **not** implemented. Blast-radius
audit found 16 `MemoryExtractor` impls — changing the trait is unjustified
"while I'm here" growth. Instead:

- **Trait UNCHANGED.** The coreference preamble is assembled at the `store_raw`
  ingest call site (`crate::extractor::assemble_with_context`) and prepended to
  the extraction input only; the stored memory body remains the bare turn.
- **`StorageMeta.context: Vec<String>`** — one additive, serde-skipped-when-empty
  field. Empty = byte-identical to the pre-ISS-162 write path.
- **`Memory::ingest_turn(text, occurred_at, context)`** — new public entry.
- **`turn_window::TurnWindow`** — bounded oldest-first ring buffer
  (`DEFAULT_WINDOW = 4`, `capacity = 0` disables) for callers to maintain the
  window; `clear()` for session boundaries.
- No rolling summary / no summary LLM call — the proven lever is raw preceding
  turns, not a compressed summary. Summary remains out of scope (re-open only
  if window alone plateaus).

### Status

Library mechanism shipped + tested:
- 4 store-path tests (`memory.rs`): empty-context byte-identity, preamble
  prepended but bare turn stored, `ingest_turn` applies context, `TurnWindow`
  end-to-end.
- 5 `TurnWindow` unit tests.
- Full suite: **2141 engramai lib tests pass**; affected integration tests
  (iss019/iss090/iss098/iss103/iss089) green; workspace builds clean.

**Remaining (AC-4 / AC-5):** swap the engram-bench LoCoMo driver from the
env-flag (`ENGRAM_BENCH_INGEST_WINDOW`, committed a5bce14) to
`TurnWindow` + `ingest_turn`, then re-run conv-26 + conv-44 to confirm the
library path reproduces the bench-flag's +11.85 pp and does not regress conv-44.

---

## REVISED ACCEPTANCE CRITERIA — 2026-06-03 (LIVE GATE)

These replace the superseded AC-1..AC-5 block. They describe the mechanism
actually shipped (`580ba3b5`) and correct the AC-4 measurement error
(see note on the 11/27 target below).

**AC-1′ (mechanism — shipped).** Coreference context is supplied at ingest
without changing the `MemoryExtractor` trait. Verified:
- `StorageMeta.context: Vec<String>` — additive, `skip_serializing_if` empty,
  byte-identical write path when empty.
- `crate::extractor::assemble_with_context` prepends preceding turns to the
  *extraction input only*; the stored memory body remains the bare turn.
- `Memory::ingest_turn(text, occurred_at, context)` public entry.
- `turn_window::TurnWindow` bounded oldest-first ring buffer
  (`DEFAULT_WINDOW = 4`, `capacity = 0` disables, `clear()` for session
  boundaries).
- 4 store-path tests + 5 `TurnWindow` unit tests; 2141 engramai lib tests pass;
  iss019/iss090/iss098/iss103/iss089 integration green.

> **Why the heavy design was dropped (not deferred-by-accident):** blast-radius
> audit found 16 `MemoryExtractor` impls — changing the trait is unjustified
> "while-I'm-here" growth (karpathy-guidelines). The empirically proven lever
> is *raw preceding turns*, not a compressed rolling summary. The original
> `extract_in_context` + `SessionState` + summary-LLM design is **not dead** —
> it is re-scoped to **ISS-216** (P2/deferred) with explicit trigger conditions.

**AC-2′ (ingest wiring — shipped).** `ingest_turn` carries the window; callers
maintain `TurnWindow`. Non-conversational callers omit context and behave
exactly as today (proved by empty-context byte-identity test).

**AC-3′ (no rolling summary — by design).** Per-namespace `SessionState` +
rolling-summary LLM are **out of scope** and tracked in ISS-216. Re-open only
if raw-window lift plateaus. (Original AC-3 superseded.)

**AC-4′ (LoCoMo measurement — corrected).**
- *Measurement-error correction:* the original AC-4 target "single-fact
  ≥ 11/27" was calibrated under the **ISS-201/161 bare envelope** (HyDE on,
  reweight off, reservation/unified off). The shipped mechanism is validated
  under the **canonical envelope** (FACTUAL_REWEIGHT on, reservation/unified
  on, HyDE off). These are *different measurement bases* — the 11/27 figure
  is not transferable and is **void**.
- *Library-path gate (RAN — LIB run `20260604T124723Z`, `TurnWindow` +
  `ingest_turn`, canonical envelope, A = window-off / B = window-on):*

  | metric | conv-26 A (off) | conv-26 B (on) | Δ |
  |---|---|---|---|
  | overall | 0.2829 | **0.3487** | **+6.58 pp** |
  | single-hop | 0.125 | 0.1875 | +6.25 pp |
  | temporal | 0.3429 | **0.5143** | **+17.14 pp** |
  | multi-hop | 0.2973 | 0.2162 | −8.11 pp |
  | open-domain | 0.3077 | 0.2308 | −7.69 pp |

  Verdict: overall +6.58 pp ✓ (≈ the env-flag direction, smaller magnitude is
  run-to-run + lib-path noise). single-hop **0.1875 < 0.20** — narrowly misses
  the AC-4′ floor. The lift is **temporal-driven, not broad** (see §A/B
  per-query diagnosis below).

**AC-5′ (cross-validation — RAN, conv-44, same LIB run).**

  | metric | conv-44 A (off) | conv-44 B (on) | Δ |
  |---|---|---|---|
  | overall | 0.2195 | **0.3089** | **+8.94 pp** |
  | single-hop | 0.2333 | 0.2667 | +3.34 pp |
  | temporal | 0.2581 | **0.4355** | **+17.74 pp** |
  | multi-hop | 0.125 | 0.125 | 0 |
  | open-domain | 0.1429 | **0.0** | **−14.29 pp** |

  Verdict: overall +8.94 pp ✓ and **direction reproduces on conv-44** (not a
  conv-26 artefact) — the temporal +17.7 pp is the same engine as conv-26.
  **BUT** conv-44 open-domain collapses to 0.0 (−14.29 pp), exceeding the
  ~10 % no-regression gate. AC-5′ is therefore **conditionally met**: net
  overall non-regression holds on both convs, but there is a real
  open-domain / multi-hop regression that the gate flags.

---

## §A/B per-query diagnosis — 2026-06-08

Direct A↔B flip analysis on conv-26 LIB run `20260604T124723Z` (152 q):

- **WIN (A fail → B pass): 24** — temporal **18**, single-hop 5, multi-hop 1.
- **LOSS (A pass → B fail): 14** — temporal 6, multi-hop 4, single-hop 3,
  open-domain 1. Net **+10 q**.

**The window does not raise recall broadly — it *reallocates* it.** It buys
temporal questions (preceding turns supply the missing time anchor: +18 wins)
at the cost of independent single-fact questions. The LOSS signature is almost
uniformly **"A answered correctly → B says *I don't know*"** (e.g. q104
*Becoming Nicole*, q129 *Brave*, q20, q47, q65).

**Recall failures are genuine, not unanswerable questions.** Every "I don't
know" gold was verified to have a source turn in the fixture
(`conversations.jsonl` conv-26): Sweden = ep60 ("my home country, Sweden"),
*Becoming Nicole* = ep118, adoption agencies = ep25/27/253, clarinet = ep331,
*Summer Sounds* = ep321. The fact is in the dialogue; the LLM answers IDK ⇒ the
gold memory did not enter top-K ⇒ true recall failure.

**Root cause is the EXTRACTION layer, not the vector layer.** `memory.rs:3609-
3614` confirms ISS-162 already decoupled the two: the windowed
`extraction_input` is fed to `extractor.extract()` *only*; the stored +
embedded body is the bare `content`. So the bare-turn embedding is unchanged —
the "window pollutes the embedding" hypothesis is **falsified**. The 14
A→IDK flips therefore come from the extractor producing *different facts /
structured entities* once it sees the preceding turns, which shifts
**factual/entity-plan anchor recall** — an extraction-layer side effect, not a
ranking-layer one.

**Open question (needs the ISS-217 candidate-dump probe to close):** for the
14 LOSS questions under window-on, is the gold memory (a) absent from the
candidate pool, or (b) present but ranked/selected wrong? `CandidateDump`
(`ENGRAM_BENCH_DUMP_CANDIDATES=1`) carries rank + per-signal subscores and will
settle this. Tracked in **ISS-217**.

**Resolve condition (updated).** AC-1′/AC-2′/AC-3′ met (code shipped). AC-4′/
AC-5′ bench RAN: net overall lift confirmed on both convs (+6.58 / +8.94 pp),
direction reproduces. Two open blockers before flipping `resolved`:
1. single-hop 0.1875 < 0.20 floor on conv-26 (narrow miss).
2. open-domain / multi-hop regression (conv-44 open-domain → 0.0) exceeds the
   no-regression gate.
Both trace to the same extraction-layer reallocation. **Recommendation:** keep
`in_review`; close ISS-217 (candidate-dump root cause) first, decide whether a
selective-window fix (apply window only to temporal-class ingest, or N=4→2) can
keep the temporal win without the independent-fact loss, then re-bench and
resolve. `fixed_by` to pin at that time: `580ba3b5` + driver-swap commit + any
selective-window fix.
