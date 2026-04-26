# ISS-021 LoCoMo Coverage Pilot Results

**Date**: 2026-04-22
**Conv**: conv-26 (Caroline & Melanie), sessions 1-3
**DB**: `.gid/issues/ISS-021-subdim-extraction-coverage/pilot/locomo-conv26-smoke.db`
**Turns ingested**: 58 dialogue turns → 31 memories extracted (27 skipped as "no facts")
**Extractor**: engram-ai-rust HEAD (dimensional format), Anthropic Haiku via OAuth
**Runtime**: 105 seconds

## Coverage Comparison

| Dimension | engram-telegram (Step 9, 98 mem) | LoCoMo conv-26 (31 mem) | Delta |
|---|---|---|---|
| participants | 0% | **29%** | +29pp 🔥 |
| temporal | 6% | **26%** | +20pp 🔥 |
| location | 0% | 3% | +3pp |
| context | — | 16% | — |
| causation | 13% | **35%** | +22pp 🔥 |
| outcome | — | 10% | — |
| method | — | 0% | flat |
| relations | — | 0% | flat |
| sentiment | — | 81% | very high |
| stance | — | 77% | very high |

**Conversation data shows 4-5x higher coverage** on the 3 key dimensions vs documentation-style memories. This confirms a significant portion of Step 9's low numbers came from the **data distribution** (engram-telegram = mostly internal notes, not conversations).

## Root Cause Finding: Participants Gap is H2 (Bug), Not H4

Of 22 memories with **no `participants` field**, ALL explicitly name a person in `core_fact`:

```
"Caroline attended a LGBTQ support group..."           → participants: null ❌
"Caroline found transgender stories inspiring..."       → participants: null ❌
"Melanie painted a lake sunrise artwork last year"      → participants: null ❌
"Melanie ran a charity race for mental health..."       → participants: null ❌
"Caroline has been transitioning for three years"       → participants: null ❌
```

The LLM is filling `participants` only when there are **ADDITIONAL** people beyond the subject already in `core_fact`. Example from first test:

```
core_fact: "Melanie's boss frequently changes deadlines, causing her stress"
→ participants: null    (both Melanie + boss in core_fact, nothing more to add)
→ causation: null       (even though "causing her stress" IS the causation!)
→ sentiment: "stressed" (got this right)
```

## The Real Root Cause: Prompt Ambiguity, Not Data Sparsity

Current prompt wording:
```
"participants": "Who was involved (omit if not mentioned)",
"temporal":     "When it happened (omit if not mentioned)",
"causation":    "Why it happened / motivation (omit if not mentioned)",
```

LLM interpretation:
- **"omit if not mentioned"** → only fill if the info is **additional** beyond core_fact
- Subject of core_fact ≠ "participant" in the LLM's mental model
- Reason embedded in core_fact ≠ "causation" worth re-stating

Result: all three dimensions under-populate **even when the information is literally in the core_fact**.

## Dimension Quality Categories

Based on the data:

**✅ Working dimensions** (high coverage, meaningful values):
- `sentiment` (81%), `stance` (77%) — LLM fills these liberally because they aren't in core_fact
- `domain`, `confidence`, `valence`, `type_weights` — always filled (schema required)
- `tags` — always filled

**❌ Broken dimensions** (should be high on dialogue data, aren't):
- `participants` (29% — should be ~80%+ on dialogue)
- `temporal` (26% — should be ~50%+ on dialogue with timestamps)
- `causation` (35% — should be ~50%+ based on explicit causal language)

**⚠️ Marginal dimensions** (low but unclear if bug or content):
- `location` (3%), `context` (16%), `outcome` (10%), `method` (0%), `relations` (0%)

## Recommended Fix (Revised from Earlier B')

**Not just causation — all three primary dimensions need prompt clarification.**

New proposed wording:
```
"participants": "WHO is the subject or co-subject of this fact. 
                 Extract the person(s) named in the fact, even if already 
                 mentioned in core_fact. Use null only if truly about no one 
                 (e.g., abstract concepts, system behaviors with no human actor).",

"temporal":     "WHEN this occurred — explicit times/dates or relative 
                 expressions ('last week', 'yesterday', 'three years ago'). 
                 Extract from both core_fact and surrounding context. 
                 Use null only if the fact has no time dimension.",

"causation":    "WHY / the cause or motivation. Extract when the fact 
                 contains causal language (because, due to, caused by, 
                 necessitating, leads to, results in, ...) OR when the 
                 purpose/motivation is evident. Use null only if truly 
                 no cause is present."
```

Key changes:
- Shift from "if not mentioned → omit" to "extract when present, even if in core_fact"
- Make it clear duplication with core_fact is **desired** for these structured dims
- Provide causal language examples to trigger extraction

## Validation Plan (If Approved)

1. Modify `extractor.rs` with new prompt wording (single commit, gated feature flag)
2. Re-run this exact smoke test → compare coverage
3. Target: participants >60%, temporal >40%, causation >50% on conv-26 sessions 1-3
4. If green: run full conv-26 ingest (all 19 sessions, ~419 turns)
5. Compare against saved baseline DB

## Decision Required

Three options:

**A) Ship prompt fix now**  
→ Modify extractor.rs, re-test, if green → commit. Risk: low (feature flag), high impact on ISS-020 ranking quality.

**B) Gather more data first**  
→ Run 2-3 more LoCoMo convs with current HEAD for baseline. Costs ~2-3 LLM credit × conv. Gains: statistical confidence, but we already have the qualitative evidence (22/22 participants-null memories have a named subject in core_fact).

**C) Hybrid extraction**  
→ Keep current conservative primary extraction, add a second-pass LLM call that ONLY populates participants/temporal/causation from existing core_fact. Cost: +1 LLM call per memory. Benefit: isolated, reversible.

**My recommendation: A** — the fix is small, testable, and the evidence is already strong. Option B just delays; option C doubles costs for the same outcome.
