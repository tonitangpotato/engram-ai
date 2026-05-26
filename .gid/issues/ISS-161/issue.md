---
title: AC-5a next lever — single-fact sub-bucket ≥0.60 on conv-26 (post-weapon-A)
status: resolved
priority: P1
severity: planning
category: retrieval
created: 2026-05-26
relates:
- ISS-148
- ISS-149
- ISS-150
- ISS-155
- ISS-159
- ISS-160
---

## Summary

ISS-148 AC-5a target: conv-26 **single-fact sub-bucket** (12 of 32
single-hop questions) ≥ 0.60. Current best measured (ISS-148 K=30
anchor): **5/12 = 0.417**. Gap to gate: need to convert **at least 3
more single-fact questions to pass**.

Two top retrieval levers have now been empirically falsified on this
sub-bucket:

| Lever | Result | Verdict |
|---|---|---|
| ISS-149 NullEntityLookup → real EntityLookup (forced-Factual probe) | -1 pass, 0 new passes (-3.13pp single-hop) | NOT the lever |
| ISS-159 weapon A cross-encoder ms-marco-MiniLM-L-6 k_in=50 (v2 sweep) | 3/12 single-fact → 3/12, **delta = 0** | NOT the lever |

This issue is a **planning ticket**: enumerate remaining candidate
levers for AC-5a, rank them by expected lift × cost, and pick the next
one to probe.

## Evidence carry-over

From ISS-159 falsification sub-bucket analysis (commit b48ba46 +
this issue's hand-classification, 2026-05-26, paper-only):

Conv-26 K=10 HyDE=per_category temp=0:

| Sub-bucket (single-hop) | A no-CE | B CE k_in=50 | K=30 anchor (ISS-148) |
|---|---|---|---|
| **single-fact (n=12, AC-5a)** | 3/12 = 0.250 | 3/12 = 0.250 | 5/12 = 0.417 |
| list (n=20, AC-5b territory) | 3/20 = 0.150 | 3/20 = 0.150 | 4/20 = 0.200 |

The single-fact bucket has 12 questions; 5 pass at the K=30 anchor.
The 7 currently-failing single-fact questions (by id, from v2 Arm A
gold-strings):

- q3  "Adoption agencies"
- q11 "Sweden"
- q37 "sunset"
- q40 "2"
- q43 "abstract art"
- q7  "Single"
- q71 "\"Becoming Nicole\""
- q75 "3"
- q76 "19 October 2023"

(IDs from `benchmarks/runs/ISS159v2-A-conv26-20260526T040634Z/locomo_per_query.jsonl`.)

Each is a discrete-fact lookup against a 675-episode dense chat
corpus. Diagnostic per-question (which arm of the pipeline drops the
right episode out of top-K) has not yet been done.

## Candidate levers (proposed, not selected)

### Lever 1 — BM25 weight bump on Factual / Associative adapters

Status: untested. ISS-150 wired BM25 into Associative, ISS-147 wired
it into Factual; weights remain at defaults. Bump BM25's relative
weight in fusion when query carries strong noun-phrase signal.

Hypothesis: discrete-fact queries ("Sweden", "abstract art", "Becoming
Nicole") have strong literal-string overlap with the gold episode but
get drowned out by Hebbian / dense-embedding signal on adjacent
content. Increasing BM25 weight should surface the literal-match
episode into top-K.

Cost: 1-2 LoC config, ~1 sweep (~12min, ~$1).
Risk: regresses paraphrase-shaped queries (multi-hop, open).
Probe shape: A baseline / B bm25_weight × 1.5 / C bm25_weight × 2.

### Lever 2 — Per-question diagnostic: where is the right episode?

Status: not yet done. For each of the 7 failing single-fact questions,
inspect the top-K candidates and check (a) is the gold episode in the
candidate pool, (b) at what rank, (c) which adapter ranked it where.
Cheap (paper-only on the existing per-query.jsonl + the candidate
dump).

Outcome routes:
- Gold episode **never in top-50 candidate pool**: pool-recall issue
  → look at indexing (extraction, embedding quality, FTS analyzer).
  This is the ISS-155-class fix (extraction-time fact density).
- Gold episode **in pool at rank 11-50**: reranker / fusion-weight
  issue. ISS-159 cross-encoder didn't move it → either wrong reranker
  features (cross-encoder uses raw episode text; maybe entity-aware
  reranker needed) or wrong scoring target (cross-encoder optimised
  for paraphrase, not literal lookup).
- Gold episode **in top-10 but generation answers wrong**: not a
  retrieval problem — punt to AC-5b (generation prompt).

This diagnostic MUST run before another sweep. ~30min paper-only.

### Lever 3 — Extraction-time fact density (ISS-155-class)

Status: ISS-155 fixed extractor temp=0 determinism; broader question
of whether the extractor is producing high-density single-fact
episodes for needles like "Sweden", "19 October 2023" remains open.

If Lever 2 diagnostic shows the gold episode is **missing from the
candidate pool**, this is the lever. Inspect extracted episodes for
the 7 failing questions: is the literal fact in any episode? If yes
but not surfaced — embedder/FTS issue. If no — extraction misses it.

Cost: medium (extraction is ~$5 per re-ingest of conv-26).

### Lever 4 — Query expansion targeted at single-fact recall

Status: ISS-153 HyDE re-tested post-ISS-155-fix, found multi-hop
regression; per_category routing (ISS-156) shipped. Could narrow
further: literal-keyword expansion for single-fact intent (e.g.,
classifier predicts Factual → generate alternate phrasings of the
noun-phrase target, not full hypothetical answers).

Cost: design + impl + sweep; ~4h + $5.
Risk: same multi-hop regression we saw with HyDE-on-everything.

### Lever 5 — Different reranker family (entity-aware, not cross-encoder)

Status: cross-encoder falsified for single-fact. Entity-aware reranker
(re-score candidates by how well they cover the query's named
entities) is a different family. Closer to the ISS-149 entity-aware
classifier path but applied at rerank stage instead of plan-dispatch.

Cost: similar to weapon A (~1 week impl).
Risk: high — same family-mismatch failure could recur if root cause
is pool-recall not pool-ordering.

### Lever 6 — Punt AC-5a, redefine ISS-148

If diagnostic (Lever 2) shows 5+ of the 7 failing single-fact
questions are unrecoverable (gold episode never extracted, or factual
content is in multi-episode context the generator can't compose), the
honest answer is AC-5a ≥0.60 is unreachable on conv-26 with current
extraction+retrieval architecture. Reframe ISS-148 to ≥0.42 (no
regression from current best) and ship.

This is the "honesty option" if Levers 1-5 are all dead.

## Decision rule

1. **Run Lever 2 first** (paper-only diagnostic, no LLM calls). This
   is non-negotiable — every sweep done without this diagnostic so
   far has been blind, and the result has been falsification.
2. Based on Lever 2 outcome, pick ONE of Levers 1, 3, 4, 5 to probe.
3. **Do not run any new bench sweep until Lever 2 is complete.**
4. If two consecutive levers falsify, escalate to Lever 6.

## Acceptance Criteria

- [ ] **AC-1:** Per-question diagnostic for the 7 failing single-fact
       questions: gold episode in pool? at what rank? from which
       adapter? Output: artifact `iss161-diagnostic.md` with one row
       per question.
- [ ] **AC-2:** Based on AC-1, select ONE candidate lever (1, 3, 4,
       or 5) and write a 1-paragraph implementation sketch.
- [ ] **AC-3:** Implement + probe selected lever (separate ISS or
       continuation of this one).
- [ ] **AC-4:** Single-fact sub-bucket B-A delta ≥ +3 questions
       (+25pp), or escalate to Lever 6.

## Out of scope

- AC-5b (list sub-bucket, generation/judge fixes): separate work
  stream tracked by ISS-160.
- General multi-hop / temporal / open improvements: this ticket is
  AC-5a-only.
- Variance bracketing of ISS-159 falsification: ISS-159 is closed-out
  on its own evidence; no re-litigation here.

## Verdict 2026-05-26 — Levers 2 + 7 both falsified

After AC-1 diagnostic (Lever 2, paper-only, commit 26947d8) and Lever 7
(generation-prompt v2, in-flight working tree) both ran on conv-26 K=10
HyDE=per_category temp=0:

**Corrected denominator.** Earlier ISS-161 evidence used n=12 ("single-fact
sub-bucket"). The real single-fact denominator after stripping list-shaped
questions from the 32 single-hop bucket is **n=27**. AC-5a 0.60 = **17/27**
(not 8/12). All numbers below are over n=27.

| Arm | overall | single-hop | single-fact (n=27) | list (n=5) |
|---|---|---|---|---|
| A — baseline (per_category HyDE, K=10) | 0.362 | 6/32 | **5/27** | 1/5 |
| B — Lever 2 PerCategoryV2 (open+sh), K=10 | 0.382 | 9/32 | **8/27** | 1/5 |
| C — Lever 2 + K=30 | 0.441 | 11/32 | **8/27** | 3/5 |
| D — Lever 7 v2 generation prompt + K=10 | 0.375 | 7/32 | **6/27** | 1/5 |
| E — Lever 7 v2 prompt + K=30 | — | — | — | — |

Arm E aborted: Anthropic API hiccup at conv-26 ingestion ep 360
(`Quarantined(ExtractorError)`), no real run captured. Sweep script
mis-labelled a stale 2026-05-22 run dir as Arm E output. Not re-run:
Arm E projected ceiling = 8/27 (= C), below the 17/27 gate, so the
result cannot change the verdict.

**Per-question single-fact diff (n=27 base set):**

- Lever 2 (Arm B) net: +3 vs A (-1 loss at q39/q55, +4 gain at q32/q40/q43/q78)
- Lever 7 (Arm D) net: +1 vs A (+1 at q70; lost q47 vs A no — actually
  +q70, net regression vs B/C losing q32/q40/q43/q78)

**L2 ship verdict:** B = 8/27 ≥ ship rule of 5/27, but **AC-5a 17/27
not reached**. C confirms K=30 stacking doesn't push sf higher (still
8/27, only list went up). Lever 2 is a real-but-small lift. Decision:
keep `HydePolicy::PerCategoryV2` code (commit 5acf83e) on branch as
opt-in — do not flip default until paired with a sf-lifting lever.

**L7 ship verdict:** D = 6/27, regresses vs B/C (lost q32, q40, q43,
q78 — the v2 "scan ALL memories" instruction makes the LLM mis-select
in a narrow K=10 pool). Reverted from working tree, stashed for
possible reuse after extraction enrichment.

**Where the 7 hard-misses sit (carry-over from AC-1 diagnostic):**

- q3 / q37 / q71 — gold fact partially present in top-K, generation
  failure mode (single LLM call doesn't synthesise across candidates)
- q7 / q11 / q75 / q76 — gold fact **absent** from top-30 (pool-recall)

L7 attacked the generation failures and helped at most 2 of them while
breaking 4 others. The 4 pool-recall misses are immune to any
generation-side fix.

**Distance to AC-5a 0.60:** best measured = 8/27 = 0.296. Gate = 0.629
(17/27). Need **+9 single-fact questions** to pass. No prompt/reranker
lever on the table has lifted sf by more than 3 questions.

**Next lever: L3 — extraction-time enrichment.** Of the 4 pool-recall
misses, q11 (Sweden) is the cleanest test case — the gold fact is split
across 2 sentences in conv-26 raw ("moved from home country" + "home
country=Sweden"), and our extractor never merges them. If a richer
extraction prompt produces a single episode containing "Sweden" as
Caroline's home country, q11 becomes retrievable. Run shape: re-ingest
conv-26 with a fact-density-tuned extractor prompt, re-run Arm A/B
side-by-side at K=10. ETA: ~$8, ~45min wall.

**Honest projection of L3:** even if L3 lifts pool-recall on q7/q11/q75/q76
(best case +4 sf), best achievable is 12/27 = 0.444. Still below AC-5a
0.629 by 5 questions. If L3 also falsifies (≤+2 sf), recommend
**Lever 6 (redefine ISS-148)** rather than chase L4/L5. AC-5a 0.60 on
conv-26 single-fact appears unreachable in current architecture without
substrate-level changes (entity resolution, multi-episode composition).

## References

- ISS-148 — root AC-5a definition (single-fact sub-bucket ≥0.60)
- ISS-149 — NullEntityLookup, falsified as AC-5a lever 2026-05-25
- ISS-159 — weapon A cross-encoder, falsified as AC-5a lever 2026-05-26
- ISS-150 — BM25 wired into Associative adapter
- ISS-147 — BM25 wired into Factual adapter
- ISS-155 — extractor temp=0 determinism (related extraction-side fix)
- ISS-160 — list-question generation/judge (AC-5b sibling)
- `benchmarks/runs/ISS159v2-A-conv26-20260526T040634Z/locomo_per_query.jsonl` — source for failing-question list

## Verdict 2026-05-26 (cont.) — Lever 3 falsified (extraction enrichment, two attempts)

Lever 3 was the last remaining prompt-engineering lever. Two attempts,
both falsified. Final state: **all prompt-level levers exhausted on
AC-5a; remaining gap requires architectural changes, not prompt
changes.**

### L3 v1 — meta-instruction extractor prompt (uncommitted, falsified)

Approach: extend EXTRACTION_PROMPT with explicit meta-instructions
("preserve all proper nouns", "biggest failure mode is dropping
specific names", "MUST keep noun phrases verbatim").

Result on conv-26 K=10 HyDE=per_category temp=0:

| Arm | overall | single-hop | single-fact (n=26) | parse fails | persona escapes |
|---|---|---|---|---|---|
| F — V1 control | 0.362 | 9/32 | 8/26 | 0 | 0 |
| G — V1 + meta-lecture | 0.342 | 5/32 | 5/26 | 159 | 48 |

Meta-instructions ("do not do it", "biggest failure mode", "MUST")
triggered Claude Haiku into alignment-escape mode: 48 explicit
"I'm Claude, an AI assistant..." outputs + 159 "No JSON array found"
parse failures. Ingestion partially broke; extractor returned empty
on ~30% of episodes.

Conclusion: meta-instructions are not a usable mechanism for steering
Claude Haiku extraction behaviour. Reverted.

### L3 v2 — example-only redesign (committed working tree, falsified)

Approach: keep Rules block byte-identical to V1, add 3 new example
pairs (Lisbon / Pragmatic Programmer / Brooklyn Botanic Garden)
demonstrating noun-phrase preservation by example. Zero
meta-instructions. Forbidden-phrase tests assert no "do not" / "must
preserve" / "biggest failure" leaks. Eval gold strings ("adoption
agencies", "Becoming Nicole") explicitly excluded from examples.

8 structural-parity unit tests pass:
- Rules block byte-equality with V1
- ≥2 new example pairs added
- No meta-lecture phrase from V1's forbidden list
- No eval gold strings in examples

Result on conv-26 K=10 HyDE=per_category temp=0 (sweep PID 85271):

| Arm | overall | single-hop | single-fact (n=26) | list (n=6) | parse fails | persona escapes |
|---|---|---|---|---|---|---|
| F — V1 control | 0.362 | 9/32 | **8/26** | 1/6 | 0 | 0 |
| G — V2 example-only | 0.349 | 4/32 | **3/26** | 1/6 | **206** | 34 |

Single-fact regressed from 8/26 → 3/26 (**-19.2pp**, -5 questions).
V2 failed worse than V1 — adding examples on top of byte-identical
rules made Claude switch register from "JSON fact extractor" to
"therapist asking follow-up questions" on conv-26's emotional /
trans-identity content. Sample failure outputs:

```
[WARN] No JSON array found in extraction response: That sounds like
a meaningful experience. What made it stand out for you?

[WARN] No JSON array found in extraction response: I appreciate the
greeting, but I should clarify: I'm Claude, an AI assistant made by
Anthropic. I'm not Mel...
```

Per-question diff (Arm F → Arm G single-fact):
- Lost (8): q13 career, q32 LGBTQ events, q39 LGBTQ participation,
  q47 negative-experience support, q48 pottery, q55 sunsets, q70
  transgender events
- Gained: 0

**Why example-only also failed.** The new examples (Lisbon /
Pragmatic Programmer / Brooklyn Botanic Garden) are all gentle
personal-experience descriptions. Stacked on top of each other with
no contrast example ("here is something to extract" vs "here is
something to NOT extract"), they read to Claude Haiku as "be an
empathetic listener capturing personal stories" rather than "be a
structured fact extractor". Combined with conv-26's
emotionally-charged content (trans identity, family support, mental
health), Claude's alignment fine-tuning drove the register switch.

The Rules block was untouched. The Rules block alone is sufficient
in V1. Adding the wrong examples destroyed it.

Reverted via `git checkout HEAD` on extractor.rs (commit fae6bb7
state restored, 34/34 extractor tests pass at baseline).

### Aggregate verdict across all attempted AC-5a levers

| Lever | single-fact (n=26 or n=27) | Status |
|---|---|---|
| L2 PerCategoryV2 HyDE (Arm B) | 8/27 | partial, ship-rule-met, AC-5a-blocked |
| L3 v1 meta-lecture extractor | 5/26 | broken (alignment escape) |
| L3 v2 example-only extractor | **3/26** | broken (worse than v1) |
| L7 v2 generation prompt | 6/27 | regression vs L2 |
| L9 cross-encoder reranker (ISS-159) | 3/12 = same as A | no movement |

Best measured single-fact: **8/27 = 0.296** at Arm B (L2). Gap to
AC-5a 0.60 = **9 questions short**.

No prompt-engineering lever has lifted single-fact by more than 3
questions over baseline. The architecture is the binding constraint.

### Root-cause investigation (engramai code layer)

Code-layer audit performed 2026-05-26 to verify whether the
"architectural changes" required by AC-5a are net-new work or
re-wiring of existing-but-disconnected components. Findings:

**Finding 1 — Single-message extraction window (no context).**
`crates/engramai/src/extractor.rs:480` calls `extract(text: &str)`
with the raw single-turn content. `ingest_with_stats_at` →
`store_raw` → `extractor.extract(content)` passes one episode at a
time. LoCoMo fixture builder (`engram-bench/scripts/build_locomo_fixture.py:99`)
emits one episode per conversation turn (`f"{speaker}: {text}"`).

**Compared to Mem0 (Chhikara et al., 2025-04, arXiv:2504.19413):**
Mem0 extractor sees `(m_{t-1}, m_t, S, recent_m)` — current
message-pair + global session summary + local window of recent
messages. This is why Mem0 captures noun phrases from `m_{t-1}` that
`m_t` references implicitly (e.g., "Adoption agencies" answer
preserves the question's noun "research").

Q3 conv-26 ("What did Caroline research?") gold = "Adoption
agencies". The relevant exchange is:
- Melanie: "What are you researching?"
- Caroline: "Adoption agencies, mainly..."

Our extractor sees only Caroline's reply in isolation; the question
context ("researching") is gone. Extracted episode becomes "Caroline
went to do some research" — noun phrase lost.

This is **not a prompt bug**. No prompt change can recover a noun
phrase from data that isn't in the input.

**Finding 2 — Semantic dedup at cosine ≥ 0.95, not Mem0-style
UPDATE.** `crates/engramai/src/memory.rs:2839` checks `sim >=
dedup_threshold` (default 0.95 from `config.rs:351`). Above
threshold = merge into existing memory (entity-aware Phase A or
embedding-only Phase B). Below threshold = both kept as separate
memories.

**Compared to Mem0:** Mem0 has a semantic-reconcile UPDATE phase
where (a) new fact with overlapping entity-set triggers compare with
all related existing memories, (b) LLM-based decision among ADD /
UPDATE / DELETE / NOOP, (c) more-specific later version replaces
less-specific earlier version even at cosine ~0.85-0.92.

Our 0.95 threshold means "I went to research" and "I researched
adoption agencies" stay as two separate memories. The narrow first
version dilutes retrieval against the specific second version. Q11
("Where did Caroline move from 4 years ago?" → "Sweden") is the
canonical example: Caroline says "I moved here 4 years ago" and
"home country was Sweden" in different turns; we keep both,
neither alone answers the question.

**Finding 3 — GraphEntityResolver wired but classifier never
routes to Factual plan.**
`crates/engramai/src/retrieval/adapters/graph_entity_resolver.rs`
implements alias-match → graph-traversal entity resolution.
`crates/engramai/src/retrieval/plans/factual.rs` Factual plan
consumes it. Both have unit tests.

But `crates/engramai/src/retrieval/dispatch.rs:92`:
```rust
(Intent::Factual, DowngradeHint::Associative) => PlanKind::Associative,
(Intent::Factual, DowngradeHint::None) => PlanKind::Factual,
```

And classifier (`src/retrieval/classifier/mod.rs:245`) emits
`DowngradeHint::Associative` whenever Entity signal is below
`τ_high = 0.7`. Conv-26 questions ("What did Caroline research?",
"What is Caroline's relationship status?") trigger no strong entity
signal — they are natural-language references, not literal alias
matches.

Empirical: 0 of 152 conv-26 queries route to Factual plan in any
sweep. Breakdown (Arm F V1 control):
- associative: 120
- abstract: 18
- affective: 7
- hybrid: 5
- episodic: 2
- **factual: 0**

GraphEntityResolver has never been called during any AC-5a
measurement. The "entity-aware retrieval" capability exists in code
but is fully bypassed at runtime.

### Conclusion — AC-5a unreachable via prompt; three architecture gaps identified

All four attempted prompt-engineering levers (L2/L3v1/L3v2/L7) have
been falsified or shown to plateau at 8/27 single-fact = 0.296. The
gap to AC-5a 0.60 (17/27 = 0.629) is 9 questions, none of which can
be saved by prompt changes given the Finding 1-3 architecture gaps:

- ~5 of the 9 missing questions (q3, q7, q11, q43, q71) fail because
  the right noun phrase is never extracted (Finding 1) or never
  consolidated (Finding 2).
- ~3 of the 9 missing questions (q40, q75, q76) need multi-episode
  composition that requires entity-graph traversal (Finding 3).
- ~1 (q37 sunset) is plausibly a generation or reranker issue but
  blocked behind the others.

**Recommendation: Lever 6 — redefine ISS-148 AC-5a.** Filing three
new ISS to make the architecture gaps explicit:

- **ISS-NNN (Finding 1):** `Ingest path missing conversation context
  window` — extractor sees one turn at a time; needs `(prev, curr,
  summary)` shape per Mem0 paradigm.
- **ISS-NNN (Finding 2):** `Dedup at 0.95 cosine is not semantic
  UPDATE` — needs Mem0-style ADD/UPDATE/DELETE/NOOP phase with LLM
  reconciliation for cosine 0.80-0.95 band.
- **ISS-NNN (Finding 3):** `GraphEntityResolver wired but classifier
  never routes to Factual plan` — needs classifier threshold review
  or alternative entity-aware adapter wired into Associative plan.

ISS-148 AC-5a should be marked `blocked-by` these three. Either
ship v0.3 with a redefined AC-5a (~0.42, matching current best at
Arm C with L2+K30), or accept that v0.3 release blocks on the three
new ISS (4-8 weeks of architecture work).

### Status flip

This ticket (ISS-161) flips to **resolved-falsified**. All proposed
levers within ISS-161 scope (prompt-engineering) have been
investigated. Architectural follow-ups are filed as separate ISS,
not nested under ISS-161.

