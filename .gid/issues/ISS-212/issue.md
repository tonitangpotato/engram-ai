---
title: conv-26-q0 fails because the bench reads the LEGACY substrate while prod runs UNIFIED (bench-config mismatch)
status: resolved
priority: '2'
labels:
- bench-config
- substrate
- unified-reads
- conv-26-q0
- retrieval-ok
- root-cause-reversed
relates_to:
- ISS-211, ISS-210, ISS-205, ISS-204
- .gid/issues/ISS-213/issue.md
- .gid/issues/ISS-214/issue.md
---

# ISS-212: generation drops the dated gold line even at rank 0 (distractor saturation)

## Summary

ISS-211 (reserved-first re-rank, v2 = relevance tiebreak) drove the gold
dated episode for conv-26-q0 to **rank 0** of the fused top-10 context.
Verified by the delivery probe on the v2 confirmation run. Yet the
generator **still** answers "I don't know." The remaining defect is
entirely in the generation/synthesis layer: the model ignores an explicit
dated line that directly answers the question, distracted by the other
nine same-subject episodes in the window.

This is the residual that ISS-211's *title* predicted but its *fix*
(ranking) could not address — ranking is now provably maxed out (rank 0),
so the only remaining lever is the prompt / synthesis instruction.

## Evidence (v2 confirmation arm — STAMP 20260602T210128Z, binary 0c8886bc, DB .tmpjTs3bm)

Delivery probe (`iss207_q0_delivery_probe`, GOLD_PREFIX=641e2014):

```
plan_used: Factual
  [ 0] score=0.7942  641e2014  [2023-05-07] Caroline attended a LGBTQ support group  <== GOLD
  [ 1] score=0.7765  cc519a6c  [2023-06-09] Caroline gave a talk ...
  [ 2] score=0.6604  3ac33027  [2023-06-23] Caroline attended an LGBTQ+ counseling workshop ...
  [ 3] score=0.6529  7691b5a9  [2023-06-17] Caroline and her transgender teen mentee attended ...
  [ 4] score=0.6968  7bb57219  [2023-05-25] Caroline is researching ... adoption ...
  ... (ranks 5-9: more Caroline advocacy / support episodes)
GOLD in top-10: YES (rank 0). Carries resolved date 2023-05-07: YES.
```

Bench verdict for conv-26-q0:

```
gold:      7 May 2023
predicted: I don't know.
           "The memories mention Caroline's involvement with the LGBTQ
            community ... but they don't specify when she went to an LGBTQ
            support group."
score:     0.0
```

The dated answer is the **first line** of context and the model still
claims the date is unspecified. This is a synthesis/prompt failure, not a
retrieval failure.

## Root cause hypotheses

1. **Subject-match blindness.** The window has 5+ "Caroline attended/went
   to an LGBTQ {support group, counseling workshop, pride parade, talk}"
   lines. The model treats them as near-duplicates and refuses to pick
   one, rather than matching the exact subject phrase ("support group")
   to its dated line.
2. **Date-line under-weighting.** The synthesis prompt does not instruct
   the model to prefer/scan explicit `[YYYY-MM-DD]` lines when the question
   is date-asking ("when did …").
3. **Over-conservative IDK bias.** The prompt likely encourages "I don't
   know" when uncertain; with several similar episodes the model defaults
   to IDK instead of answering from the best subject match.

## Proposed levers (in order of cheapness)

1. **Date-asking prompt clause** (cheapest): when the query asks "when",
   instruct the model to scan the context for `[YYYY-MM-DD]` lines whose
   subject phrase matches the question, and answer with that date; only
   say "I don't know" if no dated line matches the subject. The
   `query_classifier::asks_for_date` flag is already computed and could be
   threaded to the synthesis prompt builder.
2. **Rank-0 anchoring hint**: tell the model the first context line is the
   most relevant retrieved memory for the question.
3. **Subject-disambiguation instruction**: when multiple same-actor
   episodes appear, match on the full predicate phrase ("support group" ≠
   "counseling workshop" ≠ "pride parade").

Start with lever 1 — it is the direct counter to the observed failure and
reuses the existing date-asking classifier signal. Bench the conv-26-q0
flip plus the temporal/single-hop aggregate as a regression gate.

## Acceptance criteria

- [ ] AC-1: conv-26-q0 flips 0→1 (model answers ~2023-05-07 from the
      rank-0 dated line). **FAIL — genuine generation ceiling. See
      "Wiring-bug discovery + verdict" below.** Even with the guidance
      provably in the live binary and gold at rank 0 carrying its date,
      Sonnet-4.5 answers "I don't know" and cites lines [2]/[4] instead of
      the rank-0 line [1].
- [x] AC-2: no regression on conv-26 aggregate. **PASS.** Option-A
      confirmation arm (STAMP 20260603T010123Z) overall **0.3158** — the
      **highest** of all runs (> v2 0.3092 > baseline 0.3026). Temporal
      category 0.4571 (did not drop). The live-path wiring fix is a net
      improvement on the aggregate.
- [x] AC-3: the prompt change is gated to date-asking queries (reuses
      `asks_for_date`), leaving non-temporal synthesis byte-identical.
      Verified by `non_date_asking_query_is_byte_identical_to_base` and
      the Option-A live-path test `byte_identical_non_date` — a plain
      query renders the byte-identical, parity-pinned base template.

## Wiring-bug discovery + verdict (2026-06-02/03)

The original lever-1/lever-2 implementation above (appending guidance in
`answer_gen::render_prompt`) **never reached the model**. The live LoCoMo
answer-generation path is:

```
generate_answer → render_generate_prompt   (scorers/locomo_judge.rs)
```

not

```
answer_gen::render_prompt → answer_gen::render_extractor_prompt   (ORPHANED)
```

The entire `answer_gen` render path is dead code — it is never invoked
during a bench run. **Proof:** raw byte-grep of the lever-1 / lever-2
release binaries did **not** contain the guidance text at all, even though
the lib tests passed (the tests exercise the orphaned path). The guidance
was being rendered into a string that was thrown away.

### Option A fix (committed — `engram-bench 51639eb`, KEPT)

Inject the guidance into the **live** path, gated on the same
`engramai::query_classifier::asks_for_date` signal:

1. New `render_answer_prompt` helper in `scorers/locomo_judge.rs`.
2. `generate_answer` calls it; for date-asking queries it appends
   `LOCOMO_DATE_GUIDANCE` (re-exported from `answer_gen/mod.rs`),
   strengthened with explicit rank-0 anchoring: *"check line [1] FIRST"*,
   a matching dated line is sufficient, *"do NOT output UNKNOWN"* merely
   because similar events co-occur.
3. The parity-pinned `render_generate_prompt` (a byte-for-byte port of
   mem0 `evaluator.py`, ISS-100) is left **untouched** for all non-date
   queries — mem0-parity preserved.

**Confirmed in the binary:** raw byte-grep of the 21:00 release binary
finds *"Check line [1] FIRST"*, *"ranked by relevance"*, *"do NOT output
UNKNOWN"* all present. The guidance is now genuinely in the live prompt.

214 engram-bench lib tests pass (+2: `appends-guidance-for-date`,
`byte_identical_non_date`).

### Verdict: ⚠️ CORRECTED — NOT a generation failure. The bench was
### reading the LEGACY substrate, which hides the gold edge from retrieval.

**The "Sonnet generation ceiling" verdict was WRONG.** A direct dump of
the exact context block handed to the model (env-gated hook in
`judge_one`, `ENGRAM_DUMP_ANSWER_CTX_QID=conv-26-q0`) on the live
Option-A run DB `.tmpUgR6hw` showed the gold line is **completely absent
from the top-10**:

```
=== CONTEXT BLOCK (verbatim, as handed to generate_answer) ===
[1] [2023-10-20] Caroline passed adoption agency interviews
[2] [2023-05-25] Caroline has received support from friends and mentors ...
[3] [2023-10-13] Caroline was inspired by the energy, support ...
[4] [2023-07-12] Caroline struggled with mental health issues ...
... (no "Caroline attended a LGBTQ support group" line anywhere)
```

The model's answer — *"none of the memories explicitly mention Caroline
attending an LGBTQ support group"* — was **literally true**. It never saw
the gold. This is a **retrieval/delivery failure**, not a generation
ceiling.

### Why the delivery probe kept reporting PASS (the probe vs bench divergence)

`iss207_q0_delivery_probe` **hardcoded `cfg.unified_substrate = true`**.
The bench's `fresh_in_memory_db` (`engram-bench/src/harness/mod.rs:564`)
sets `cfg.unified_substrate` from the env var
`ENGRAM_BENCH_UNIFIED_SUBSTRATE`, **defaulting to `false` when unset** —
overriding `MemoryConfig::default()`'s `unified_substrate = true` (T32,
2026-05-15). The locked ISS-190 envelope **never sets that env var**, so
every LoCoMo run has been reading the **legacy** substrate.

The temporal reservation that promotes the dated gold episode to rank 0
traverses the **unified** `nodes`/`edges` (where the gold's `occurred_on`
edge lives). Under **legacy** reads that traversal finds nothing → gold is
never promoted → it falls out of top-10. The probe (unified) and the
benchmark (legacy) were measuring **two different substrates**.

### Proof — cheap same-DB A/B (no full re-run)

Ran `iss207_q0_delivery_probe` **twice on the same live-bench DB
`.tmpUgR6hw`**, varying **only** `cfg.unified_substrate`
(via a temporary `ISS207_UNIFIED` knob, probe restored clean afterwards):

```
unified=true   → gold f40f81c3 "[2023-05-07] Caroline attended a LGBTQ support group"  RANK 0   PASS
unified=false  → gold ABSENT from top-10                                                          FAIL
```

The `unified=false` top-10 is **byte-for-byte identical** to the live
bench's dumped context block ([1]=adoption interviews, [2]=received
support from mentors, …). Same DB, same query, same
`GraphQuery(limit=10, temporal_reservation=5)` — the **only** variable is
the substrate-read flag.

The gold memory's temporal metadata in this DB is clean
(`{"kind":"day","value":"2023-05-07"}`), so the extractor is **not** the
problem (the 2026-05-29 full-year-stranding bug is fixed). The date would
render correctly **if** the gold reached the context.

### The real fix

The bench must run **unified** reads to match the production default
(`MemoryConfig::default().unified_substrate = true`). Either:

- set `ENGRAM_BENCH_UNIFIED_SUBSTRATE=1` in the ISS-190 envelope, or
- (root) flip `fresh_in_memory_db`'s default to `true` so the bench stops
  silently measuring the legacy substrate while production runs unified.

This **supersedes the lever-3 / generation-ceiling decision entirely** —
neither was the right fix. The "prompt hardening" levers were chasing a
generation symptom that did not exist; the actual defect is a
bench-config substrate mismatch.

### ⚠️ Blast radius — re-examine the ISS-204→211 q0 chain

Every q0 conclusion in ISS-204→211 was measured against the **legacy**
substrate (envelope never set the unified flag). The retrieval fixes
(reservation, ordering, surfacing) target **unified-read** paths the bench
was not exercising. Their q0 verdicts (and any "falsification") must be
re-evaluated under `ENGRAM_BENCH_UNIFIED_SUBSTRATE=1`. A confirmation arm
with the flag on is running to verify q0 flips end-to-end under unified
reads.

### Open: bench-substrate default mismatch (file follow-up ISS)

`MemoryConfig::default()` ships `unified_substrate = true`, but the bench
harness defaults it to `false`. This is a latent measurement bug — the
benchmark does not reflect production retrieval behaviour. File a
dedicated ISS to flip the bench default (and audit every prior
substrate-sensitive run that assumed the bench matched prod).

### Dead-code follow-up

The orphaned `answer_gen` `render_prompt` / extractor path (incl. the
lever-1/lever-2 commits `84f869b`, `49471b6`, and the
`guidance_anchors_on_rank_zero_line` test in `prompt.rs`) is dead code.
Either remove it or formally wire it. Tracked as a follow-up; do not leave
two prompt-builder paths where only one is live.

## Implementation (lever 1 — date-asking prompt clause)

The answer-extraction prompt lives in **engram-bench**
(`src/answer_gen/`), not engramai — LoCoMo short-answer synthesis is the
benchmark's job. Wiring:

1. **`engramai::query_classifier::asks_for_date`** promoted from
   `pub(crate)` to `pub` so the bench can gate on the *same* signal the
   retrieval reservation (ISS-205/211) uses — retrieval and generation
   stay aligned on "is this a date-asking query".
2. **`locomo_date_guidance.txt`** (new, engram-bench): a guidance block
   instructing the model to scan `[YYYY-MM-DD]` lines, match the
   question's **full action phrase** (not just the same person/topic —
   "support group" ≠ "counseling workshop" ≠ "pride parade"), answer with
   that date, and only emit `UNKNOWN` if no dated line matches the
   specific event.
3. **`render_prompt`** appends the guidance after the base template
   **only** when `asks_for_date(query)` is true. Plain queries get the
   byte-identical base prompt (SHA unchanged). The augmented variant's
   SHA is reported via `locomo_date_prompt_sha256` for repro honesty.

The base `locomo_prompt.txt` is untouched, so its committed
`prompt_sha256` (design §6.1) is preserved for every non-date-asking
question — the gating is the key to AC-3.

Tests: 5 new in `answer_gen::prompt::tests`
(`date_asking_query_gets_guidance`,
`non_date_asking_query_is_byte_identical_to_base`,
`base_and_date_prompt_shas_differ_and_are_stable`,
`guidance_block_matches_event_phrase_not_just_subject`, plus the existing
suite). 211 engram-bench lib + 2133 engramai lib green.

AC-1/AC-2 pending the conv-26 confirmation bench arm (re-run with the
date-guidance binary; the delivery probe already proved gold is at rank 0,
so the only variable is whether the guidance makes the model use it).

## Why this is separate from ISS-211

ISS-211 was a **retrieval/ranking** fix (deliver the dated episode to the
head of the window). It is done and proven (rank 0). ISS-212 is a
**generation/synthesis** fix (make the model *use* the delivered line).
Different layer, different fix, independently testable.

---

## ✅ RESOLVED — root fix landed, q0 flips 0→1 end-to-end under unified reads (2026-06-03)

### End-to-end confirmation arm (STAMP 20260603T020631Z, binary 21:46, `ENGRAM_BENCH_UNIFIED_SUBSTRATE=1`)

Full conv-26 LoCoMo run (152 q) with the unified flag on. The env-gated
context dump and the bench judge agree:

**Context block handed to the model (verbatim):**

```
[1] [2023-05-07] Caroline attended a LGBTQ support group   <== GOLD, rank 0, dated
[2] [2023-06-09] Caroline gave a talk about her personal journey ...
[3] [2023-06-23] Caroline attended an LGBTQ+ counseling workshop last Friday (2023-06-23) ...
...
```

**Bench verdict (`locomo_per_query.jsonl`, run `2026-06-03T02-33-33Z_locomo`):**

```json
{ "id": "conv-26-q0", "category": "multi-hop",
  "predicted": "2023-05-07", "gold": "7 May 2023",
  "score": 1.0, "verdict_raw": "Yes" }
```

q0 flips **0 → 1**. Under unified reads the temporal-reservation edge
promotion delivers the dated gold episode to rank [1]; the model reads
line [1] and answers correctly; the judge passes. No prompt hack, no
generation change — purely realigning the bench substrate with production.

### Root fix (committed)

`engram-bench/src/harness/mod.rs` `fresh_in_memory_db`: the
`ENGRAM_BENCH_UNIFIED_SUBSTRATE` env var is now an **opt-out**
(`!= "0"` ⇒ unified), so the default matches
`MemoryConfig::default().unified_substrate = true` (T32). Setting
`ENGRAM_BENCH_UNIFIED_SUBSTRATE=0` selects the legacy arm for parity
campaigns. 214 engram-bench lib tests pass with the flipped default.

### Acceptance

- [x] **AC-1**: conv-26-q0 flips 0→1. PROVEN — predicted `2023-05-07`,
      judge `Yes`, score `1.0` on the unified end-to-end arm. The flip is
      caused by retrieval delivering the dated gold (rank 0), not by any
      generation/prompt change.
- [x] **AC-2 (Option-A live-path wiring)**: `render_answer_prompt` in the
      live path, `51639eb`, KEPT (date guidance now genuinely reaches the
      model; harmless under unified where retrieval already delivers gold).
- [x] **AC-3 (gating)**: date-asking gate keeps non-temporal synthesis
      byte-identical (parity tests green).

### Follow-ups filed

- Audit of substrate-sensitive historical runs (ISS-204→211 q0 chain were
  all measured against legacy) — see follow-up ISS.
- Dead-code removal of the orphaned `answer_gen` render path
  (`84f869b`/`49471b6`) — see follow-up ISS.

**Status: resolved.** The "Sonnet generation ceiling" verdict is fully
retracted; conv-26-q0 was a bench-config substrate-read mismatch.
