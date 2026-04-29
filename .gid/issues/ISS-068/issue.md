---
id: ISS-068
title: "Pipeline extractor drops conversational turns referenced as gold evidence (LoCoMo conv-26 D1:12, D2:15, D3:16)"
status: open
priority: P1
filed: 2026-04-29
filed_by: rustclaw (autopilot Phase B / RUN-0004)
related: ISS-067, ISS-064, ISS-061, locomo-test-log
labels: [retrieval, ingestion, locomo, recall-ceiling]
---

# ISS-068: Extractor silently drops "response" turns even when they carry distinct factual / relational content

## Symptom

LoCoMo conv-26 sessions 1-3 has 18 + 15 + 17 = 50 dialogue turns (`D1:1..D1:18`, `D2:1..D2:15`, `D3:1..D3:17`).
Only **28 distinct dia_ids** end up persisted in the substrate DB (substrate path:
`.gid/issues/_smoke-locomo-2026-04-28/locomo-conv26-s1-3-iss058.db`):

```
D1: 3, 5, 7, 9, 11, 14, 16              (7 of 18)
D2: 1, 2, 3, 5, 7, 8, 10, 12, 14        (9 of 15)
D3: 1, 2, 3, 4, 5, 6, 7, 10, 11, 13, 14, 17  (12 of 17)
```

The dropped turns include three that LoCoMo gold annotations explicitly cite as evidence:

- **D1:12** "You'd be a great counselor! …" — gold for q2 ("When did Melanie paint a sunrise?")
   and q-activities ("What activities does Melanie partake in?")
- **D2:15** "Caroline, you're going to be an amazing mom!" — gold for q19
   ("What does Melanie think about Caroline's decision to adopt?")
- **D3:16** "Mel and her husband have been married for 5 years" — gold for q20
   ("How long have Mel and her husband been married?")

These three account for **3 of the 5 Factual misses** in RUN-0003 / RUN-0004
(both runs sit at hit@5 = 14/25 = 56.0%, identical, despite same code/data;
see RUN-0004.md for full reproducibility notes).

The remaining 2 Factual misses (q5 D1:5, q8 D2:14) are *retrieval-side* —
the memory exists in the DB but the query doesn't surface it (semantic gap
between "Caroline's identity" and stored content "Caroline found
transgender stories inspiring"). That's a separate problem (embedding /
query rewriting), tracked elsewhere.

## Why this matters for v0.3 ship

LoCoMo recall ceiling depends on what survives ingestion. With ~44% of
turns dropped, **no amount of retrieval improvement can recover those
queries** — they hit a hard floor. Hybrid fallback (ISS-067), abstract
plan tuning (ISS-061), affective state ingestion (ISS-066/affective) all
optimise downstream of a leaky upstream.

Concretely: Phase B / RUN-0003 → RUN-0004 saw **zero** hit@5 movement
(56.0% → 56.0%, Δ < 0.001) across two runs. That's not noise — it's a
ceiling. Three of the five Factual misses can never be fixed by
retrieval changes; they require ingesting the dropped turns.

## Hypothesis (Five Whys)

1. **Why** is recall stuck at 56% on a 25-question slice?
   → Five Factual queries miss; three of those have gold-evidence dia_ids
     that simply aren't in the DB.

2. **Why** are those dia_ids not in the DB?
   → The pipeline extractor produced no `Stored` outcome for them.

3. **Why** did the extractor skip them?
   → (Hypothesis, needs verification): `process_dialogue` (or whatever
     pipeline stage groups dialogue turns) treats some turns as
     "supporting" / "agreement" / "duplicate" and either merges them
     into an adjacent Stored memory under a different `source` dia_id,
     or drops them outright when the LLM extractor returns no triples.

4. **Why** is there no `quarantine` row for them?
   → `quarantine` table is empty (verified: `SELECT COUNT(*) FROM quarantine = 0`).
     So the turns are not being quarantined — they're being skipped
     silently before the quarantine path runs, OR the extractor returned
     `Stored(_)` but with the source attributed to the prior turn.

5. **Why** is this silent?
   → No log line at WARN level says "skipping turn D1:12 because …".
     Need to instrument `PipelineRecordProcessor::process_record` (or
     whatever dialogue ingestion entrypoint conv-26 uses) to log every
     turn it sees + its outcome (Stored, Quarantined, Skipped, MergedInto).

## Repro

```sh
SUBSTRATE=$ENGRAM/.gid/issues/_smoke-locomo-2026-04-28
sqlite3 $SUBSTRATE/locomo-conv26-s1-3-iss058.db \
  "SELECT DISTINCT source FROM memories WHERE source LIKE 'locomo/conv-26/D1:%' ORDER BY source;"
# Expected: D1:1..D1:18 (with at most a couple legitimate skips like D1:1 greetings)
# Actual:   D1:3, D1:5, D1:7, D1:9, D1:11, D1:14, D1:16
```

Then check the original turn content via:

```sh
python3 -c "
import json
data = json.load(open('cogmembench/datasets/locomo/data/locomo10.json'))
for c in data:
    if c.get('sample_id') == 'conv-26':
        for t in c['conversation']['session_1']:
            if t['dia_id'] in ('D1:12','D1:14','D1:16'):
                print(t['dia_id'], t['speaker'], t['text'])
        break
"
```

Confirms D1:12 is content-rich ("You'd be a great counselor! Your empathy
and understanding will be ideal for that role.") — not filler / greeting.

## Acceptance criteria for fix

- Re-ingest conv-26 sessions 1-3 → at least 95% of original turns
  produce either `Stored(_)` or `Quarantined(reason)` outcomes
  (target: ≥ 47 of 50 turns visible in either `memories.source` or
  `quarantine.source`).
- D1:12, D2:15, D3:16 specifically must be visible in one of those
  tables after re-ingest (gold-evidence backstop).
- LoCoMo conv-26 s1-3 hit@5 should rise above 56% on RUN-0005+
  (precise target depends on whether retrieval can find the now-present
  memories; estimate ceiling lift = +12% if retrieval matches new turns).

## Notes

- This issue was discovered during autopilot Phase B RUN-0004.
- RUN-0003 and RUN-0004 producing identical numbers (56.0%/56.0%) is
  itself diagnostic — same substrate, same code, two runs apart =
  hard ceiling.
- The dropped pattern is suggestive but not conclusively "Speaker B's
  responses": e.g. D1:14 (Melanie) is kept, D1:12 (Melanie) is dropped.
  More likely a triple-extraction success/failure boundary than a
  speaker filter. Verify by logging extractor output per turn.

## Related work

- ISS-067 (Hybrid fallback) — also retrieval-ceiling-related but in a
  different code path; orthogonal.
- ISS-064 (namespace mismatch silent empty) — was masking ingestion-side
  problems by always returning empty regardless. Now closed; this
  issue is one of the things newly visible.
- LoCoMo test log RUN-0003, RUN-0004 — primary evidence.
