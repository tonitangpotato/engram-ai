---
id: ISS-097
title: "eval pipeline tooling debt: no canonical RUN summary, two-file split undocumented, log vs table truth confusion"
status: open
priority: high
labels: [eval, tooling, dx, observability]
created: 2026-05-01
relates_to: [ISS-095]
---

# Eval pipeline tooling debt

## Why this is filed

In a single conversation 2026-05-01, agent (me) made FOUR consecutive wrong claims about RUN-0012 health:

1. "4 dim mismatches" (grep ingest.log)
2. "graph layer completely empty" (queried main .db, missed `<run>.graph.db` exists as separate file)
3. "216 / 437 = 49% extraction failure" (counted rows in `graph_extraction_failures` table, treated as run-level failures)
4. "silent data loss" (filed ISS-095 with this premise, then had to invalidate it)

Truth: 432/437 succeeded (~99%); the 215 "failures" are `no_facts_extracted` (benign — extractor correctly skipping chitchat).

Each error came from the SAME pattern: **grepping symptom logs / counting rows in observability tables instead of querying status columns**. The agent retained the lesson briefly then made the same class of error again 5 minutes later. This is a tooling problem, not just an agent problem — the data layout actively encourages this mistake.

## Concrete issues

### (a) No canonical RUN summary

After a RUN-NNNN finishes, there is no auto-generated summary doc that states:

```
RUN-0012-iss091 summary:
  Pipeline runs:        437 total
    succeeded:          432 (99.1%)
    failed:               4
    running:              1   ← stuck?
  Extraction events:    216 logged
    no_facts_extracted: 215 (benign — chitchat)
    unresolved_subject:   1
  Graph state:
    entities:           623
    edges:              709
    memory→entity:     1122 (315/441 memories linked)
  Memories:             441
```

Without this, every health question requires ad-hoc sqlite3 spelunking, and agents/humans both miscount.

### (b) Two-file split is undocumented

The fact that v0.3 stores memory in `<run>.db` but graph in `<run>.graph.db` is not in any obvious doc — agents and future-you both look in the wrong file first.

### (c) Failure table conflates "errors" and "observability events"

`graph_extraction_failures.error_category = no_facts_extracted` is recorded in a table named "failures" but is not a failure. This naming directly caused mistake (3) above. Either:

- Rename column / table to `extraction_events` with a `severity` column, OR
- Move benign no-op categories to a separate `extraction_observations` table

### (d) Truth-source map missing

A short doc like `engram/docs/eval-truth-sources.md` mapping "what question → which table/column" would prevent the whole class of error.

## Proposed deliverables

- [ ] **D1**: `engram/scripts/run_summary.py <run-dir>` — produces the summary block above as markdown, written to `<run-dir>/SUMMARY.md` automatically by `02_retrieve.sh` after run completes
- [ ] **D2**: `engram/docs/eval-truth-sources.md` — short doc, "to answer X, query Y" map, including the two-file split
- [ ] **D3**: Decide on (c) — either rename the table/column or split the table. Write decision in the doc above.
- [ ] **D4**: Investigate the 4 actually-failed pipeline runs in RUN-0012 — confirm they're benign or file follow-ups

## Acceptance

- [ ] Running a fresh RUN-NNNN automatically produces SUMMARY.md
- [ ] eval-truth-sources.md exists and is linked from main README + AGENTS.md
- [ ] (c) decision documented and applied (or explicitly deferred with rationale)
- [ ] Re-test by asking a sub-agent (or main agent) "is RUN-XXXX healthy?" — expect correct answer in 1 tool call instead of 4 wrong ones

## Estimated effort

D1 + D2: ~3 hours. D3: depends on choice (rename = 1h, table split = 4h). D4: 1h.

## Why this is high priority

Every wrong claim about a run's health → wrong issue gets filed → wrong issue burns hours → potato loses trust in agent's reads of run state. This compounds. ISS-094 (cogmembench adapter fix) and ISS-093 (full J-score) both require trustworthy run-state reads.
