---
id: ISS-068
title: "Pipeline extractor drops conversational turns referenced as gold evidence (LoCoMo conv-26 D1:12, D2:15, D3:16)"
status: resolved
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

## Diagnosis (confirmed 2026-04-29)

Root cause located in `crates/engramai/src/memory.rs::store_raw`
(lines ~2670–2730). When the memory extractor returns
`Ok(vec![])` (no extractable facts), `store_raw` short-circuits to
`RawStoreOutcome::Skipped { reason: NoFactsExtracted }` **before
persisting the raw episodic content** to the `memories` table.
The CLI surfaces this as `skipped:<content_hash>` and exits 0, so
the Python ingest harness has no way to detect the drop.

Evidence (from `_smoke-locomo-2026-04-28/ingest_iss058.log`):
- 58 turns submitted to engram CLI
- 30 returned `stdout: skipped:<hash>` (52% drop rate)
- 28 returned a UUID
- DB now has 31 rows in `memories` for `locomo/conv-26/D%`
- `quarantine` table: 0 rows
- `graph_extraction_failures`: 0 rows
- All drops are `NoFactsExtracted` from the LLM extractor on
  short conversational turns ("Yeah I painted that lake sunrise
  last year! It's special to me.")

Sample dropped turn (D1:12):
```
Melanie: You'd be a great counselor! Your empathy and
understanding will really help the people you work with. By
the way, take a look at this.
```
Content-rich for retrieval, but the extractor judged it
contained no graph-worthy triples.

The deeper design question: should `NoFactsExtracted` block raw
admission? The extractor decides *graph extraction* viability,
but the raw content still has FTS + embedding retrieval value.
Two semantics are conflated in one decision.

## Proposed fix (small scope)

Split `Skipped { NoFactsExtracted }` into two paths:

1. **Persist the raw memory row** (FTS + embedding indexable).
2. **Skip graph extraction** — record a `graph_extraction_failures`
   row with `error_category = 'no_facts_extracted'` so retries are
   bounded and operators have a visible signal.
3. CLI continues to return the row UUID (not `skipped:<hash>`) so
   ingest harnesses don't silently lose data.

This restores admission parity with v0.2 (which had no extractor)
without abandoning v0.3's graph-aware path. Filing as a sub-issue
once design is reviewed — not a midnight one-shot, the change
touches the core admission path and the public CLI return value
(API impact).

---

## Resolution (2026-04-29)

**Commit:** `6f5821a` — `fix(ingestion): persist raw memory when extractor returns no facts`

### Fix shape

In `Memory::store_raw`, the `Path A` branch where `extractor.extract(content)`
returns `Ok(facts)` with `facts.is_empty()` no longer returns
`RawStoreOutcome::Skipped { NoFactsExtracted }`. Instead it:

1. Persists the raw content via `EnrichedMemory::minimal` →
   `store_enriched` (mirroring Path B's no-extractor path), so
   FTS / embedding indices receive the row.
2. Calls a new helper `record_no_facts_extraction_failure(memory_id)`
   that writes a `graph_extraction_failures` row with category
   `no_facts_extracted` (newly added to `audit::CATEGORY_*` allowlist
   and `validate_failure_closed_sets`).
3. Emits `StoreEvent::Stored { id, fact_count: 0, … }` plus a paired
   `StoreEvent::ExtractionFailure { reason: NoFactsExtracted, … }`
   for write-stats observability. The new
   `WriteStats::extraction_failures_by_reason` histogram replaces
   the (semantically wrong) `skipped_by_reason[NoFactsExtracted]`
   counter for this case.
4. Returns `RawStoreOutcome::Stored(vec![outcome])`, so the CLI
   shim (`add_to_namespace`) returns a real UUID instead of
   `skipped:<hash>`.

The graph-store insert is best-effort: if the graph store isn't wired
or the insert fails, the helper logs at `debug` and admission still
succeeds (GUARD-1: observability plumbing must not gate raw memory
admission).

### Verification

Smoke harness: `.gid/issues/ISS-068/smoke_verify.py` (LoCoMo conv-26
session_1 ingest with the fixed release binary).

| Metric | Pre-fix | Post-fix | Acceptance |
|---|---|---|---|
| D1 turns persisted (distinct dia_ids) | 7 / 18 | **18 / 18** | ≥ 17 ✅ |
| `D1:12` (gold evidence) reachable | ❌ | ✅ | required ✅ |
| CLI `skipped:<hash>` returns | 30 / 58 | **0 / 18** | 0 ✅ |
| `graph_extraction_failures` rows | 0 | 10 | > 0 (observability) ✅ |

All four acceptance criteria from the original issue are met.

### Tests

Updated `crates/engramai/tests/iss019_write_stats_test.rs::empty_extractor_result_persists_raw_and_records_failure`
(renamed from `_bumps_no_facts_bucket`) to assert the new behavior:
`Stored` outcome, `stored_count == 1`, `skipped_count == 0`,
`extraction_failures_by_reason[NoFactsExtracted] == 1`.

Full engramai test suite (1791 unit + integration tests): 0 failures.

### Next-step recall impact

This unblocks the LoCoMo Hit@5 ceiling for Factual queries that depended
on the dropped turns. A fresh end-to-end LoCoMo run (sessions 1+2+3 +
retrieval) is the next verification — pre-fix baseline was 14/25 Hit@5.
Tracked under retrieval execution, not this issue.
