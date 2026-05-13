---
title: ISS-087 occurred_at threading + Ebbinghaus decay = mass soft-delete on fresh ingest
status: done
priority: P0
severity: blocker
labels:
- recall
- lifecycle
- regression
- jscore-blocker
relates_to:
- ISS-087
- ISS-088
- ISS-099
- ISS-085
discovered_in: RUN-0017
date_found: 2026-05-05
fix_summary:
- 'Option A implemented: added nullable occurred_at column to memories table. created_at = wall-clock (Utc::now()) on store_raw'
- occurred_at = caller-supplied event time. Ebbinghaus decay reads created_at (unchanged)
- 'temporal grounding reads occurred_at.unwrap_or(created_at). Verification: 3/3 regression tests pass (single + bulk(50) + decay-still-runs); RUN-0018 152q J-score 42.1% (vs RUN-0017 3.6%'
- +38.5pp)
- well above the >15% acceptance threshold; pipeline confirmed alive end-to-end.
resolution: fixed
resolved_date: 2026-05-05
resolved_in: RUN-0018
---

# ISS-103: Mass soft-delete after ingest because `created_at` is set to gold session date (years in the past)

## TL;DR

ISS-087 made `occurred_at` propagate from `meta.session_date` into the
`created_at` column on the `memories` table. This is *correct* for
temporal grounding (we want to know when an event happened). But
**Ebbinghaus decay computes "memory age" as `now - created_at`** —
treating ingest of a 2023 conversation as if it had been sitting in the
DB for 3 years.

Result: within hours of ingest, every fact decays below the 0.1
effective-strength threshold and `check_decay_and_flag` soft-deletes it
(because `access_count < 2`).

**Confirmed in conv-26.db: 457 of 458 rows soft-deleted within 4 hours of ingest.**
Recall returns 0–1 results because almost everything is filtered by
`WHERE deleted_at IS NULL`.

This is the actual blocker for ISS-085 (J-score arc). RUN-0017 was
trending **3.6% / 0% ev_recall** — worse than RUN-0013's 8.0% — for this
reason, not because of grounding/extractor coverage.

## Repro

```bash
DB=/Users/potato/clawd/projects/cogmembench/.engram_dbs/conv-26.db
sqlite3 "$DB" "SELECT
  COUNT(*) total,
  SUM(CASE WHEN deleted_at IS NULL THEN 1 ELSE 0 END) live,
  SUM(CASE WHEN deleted_at IS NOT NULL THEN 1 ELSE 0 END) deleted
FROM memories WHERE namespace='conv-26'"
# → 458 | 1 | 457
```

```bash
# All 457 deleted at the same instant — single bulk pass
sqlite3 "$DB" "SELECT substr(deleted_at,1,16), COUNT(*)
  FROM memories WHERE namespace='conv-26' AND deleted_at IS NOT NULL
  GROUP BY 1"
# → 2026-05-02T02:19:19 | 457
```

```bash
# created_at is a 2023 timestamp (ISS-087 worked as designed)
sqlite3 "$DB" "SELECT id, datetime(created_at,'unixepoch'), deleted_at
  FROM memories WHERE namespace='conv-26' LIMIT 3"
# → 5eb395d8 | 2023-07-06 17:38:00 | 2026-05-02T02:19:19...
```

## Root cause

`crates/engramai/src/memory.rs` `check_decay_and_flag()`:

```rust
let effective = ebbinghaus::effective_strength(record, now);
if effective < 0.1 {
    if record.access_times.len() < 2 {
        self.storage.soft_delete(&record.id)?;  // ← here
    }
}
```

`ebbinghaus::effective_strength` decays from `created_at` to `now`.

After ISS-087, `created_at` for ingested conversational memories ==
gold-conversation-date (2023). For LoCoMo, that's 2–3 years before
benchmark wall-clock. Decay from any reasonable starting strength to
3y elapsed → effectively 0.

Combined with `access_count < 2` filter (newly-ingested memories have
exactly 1 access from the insert), every memory becomes a deletion
candidate as soon as `sleep_cycle` runs.

## Why this is severe

1. **Silent**: ingest reports success. The DB looks healthy on insert.
   Only on later recall does it surface as "0 results".
2. **Universal**: any benchmark that ingests historical-dated content
   hits this. LoCoMo, any chat replay, any backfill from logs.
3. **Wipes ground truth**: the gold-evidence rows are exactly the ones
   most likely to be deleted (low post-ingest access).
4. **Masks every other improvement**: ISS-087, ISS-088, ISS-099, the
   extractor work, J-score work — all blocked by this until fixed.

## Likely fixes (need design discussion)

The tension is real:
- We *want* `occurred_at` = gold date so temporal queries work
- We *don't want* `now - occurred_at` to drive forgetting

### Option A: Decouple `created_at` (DB row age) from `occurred_at` (event time)

Add an explicit `occurred_at` column. `created_at` = ingest wall-clock.
`occurred_at` = event time (what ISS-087 currently writes to created_at).
Ebbinghaus uses `created_at`. Temporal grounding/recall uses `occurred_at`.

This is the cleanest root fix. Maps to how Datomic/EventStore separate
"transaction time" from "valid time" — a well-known DB pattern.

### Option B: Decay floor / minimum age

Cap `now - created_at` at some max (e.g., 90 days) so old-dated content
doesn't instantly decay. Hacky but small.

### Option C: Skip decay until N accesses OR M wall-clock-days

Don't decay until the row has had a chance to be retrieved. Doesn't fix
the core conflation; defers the deletion.

### Option D: Disable auto-soft-delete in benchmark mode

A flag/env-var. Pragmatic for benchmarks but doesn't fix production.

**Recommendation: Option A.** This is the same separation problem that
distinguishes "system time" from "user time" in any temporal database —
worth doing right.

## Related

- ISS-087: introduced the conflation (correctly fixed grounding, but
  used `created_at` as the carrier)
- ISS-088: temporal grounding for extracted facts (depends on
  `occurred_at` being available — Option A would let this stay clean)
- ISS-099: dia_id passthrough (orthogonal, not implicated)
- ISS-085: J-score arc — this issue is the current blocker

## Verification (after fix)

1. Re-ingest conv-26
2. Wait 4 hours / trigger sleep_cycle
3. Confirm `SELECT COUNT(*) FROM memories WHERE namespace='conv-26' AND deleted_at IS NULL` ≈ 458
4. Re-run RUN-0017 conv-26
5. Expect J-score > 15% (clears RUN-0013 baseline)

## Backup

Pre-investigation DB snapshot:
`/Users/potato/clawd/projects/cogmembench/.engram_dbs/conv-26.db.before-RUN-0017`
