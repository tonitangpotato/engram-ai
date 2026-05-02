# RUN-0016 — J-score validation for ISS-099

**Status:** running (PID see _launch.pid)
**Started:** 2026-05-01 20:28 EDT
**Compare to:** RUN-0013 (evidence_recall=1%, J-score=8%)

## What this validates

ISS-099 claim: `--meta dia_id=...` was being silently dropped before, causing
locomo evaluator to score zero recall (it can't match dia_ids it never sees).

Smoke test (this session, pre-RUN-0016) proved: current engram binary +
`--meta dia_id=D1:1` ⇒ DB row has `metadata.user.dia_id = "D1:1"`.

Now we need J-score to confirm the fix moves the benchmark.

## Setup

- Pre-existing `.engram_dbs/conv-26.db*` moved to `.before-RUN-0015` (preserved, not deleted)
- Forces cogmembench to re-ingest using current adapter + current binary
- `--max-questions 25` cap (~2 min judge after ~25 min ingest = ~30 min total)

## Backup files (recoverable)

```
.engram_dbs/conv-26.db.before-RUN-0015      (3.9 MB, was RUN-0013 substrate)
.engram_dbs/conv-26.db.before-RUN-0013      (2.7 MB, even older)
.engram_dbs/conv-26.db-{shm,wal}.before-*   (companions)
.engram_dbs/conv-26.graph.db.before-RUN-0015
```

## Acceptance

| Metric | RUN-0013 baseline | RUN-0016 target | What it means |
|---|---|---|---|
| evidence_recall | 1% | >40% | adapter→binary→DB→retrieve all working end-to-end |
| J-score | 8% | >30% | LLM judge sees enough context to answer correctly |
| metadata.user.dia_id rows | 0/441 | >400/N | this is the actual fix |

If evidence_recall stays low → bug is downstream of ingest (retrieve filter,
context formatter, or judge prompt). Will need a separate dive.
