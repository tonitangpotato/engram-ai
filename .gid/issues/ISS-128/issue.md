---
id: ISS-128
title: "T26a triple-backfill driver: persist failed memory_ids for forensic recovery"
status: open
priority: P3
severity: minor
created: 2026-05-14
relates_to: [ISS-127]
labels: [substrate, backfill, observability]
---

# Problem

`backfill_triples_from_memories` (`crates/engramai/src/substrate/triple_backfill.rs`) increments a `rows_failed` counter when a memory exhausts `max_retries`, but **does not persist the failing memory_ids anywhere**. When a 14k-row T26c run completes with, say, 3 failures, we know "3 failed" but cannot recover which 3 — no DB column, no log file, no notes JSON field.

Surfaced during T26c (2026-05-14) when preparing the post-run review script. The script can report `memories_failed = N` from the checkpoint but cannot show *which* memories. Workaround for T26c is to grep the run log for stderr/error lines, which is brittle.

# Fix

Two options, both small:

**A. Notes JSON field on `backfill_runs`** — append failed memory_ids to a `failed_memory_ids` array in the `notes` JSON. Cheap, no migration.

```rust
let notes = json!({
    "driver": "backfill_triples_from_memories",
    "design_ref": "v04-unified-substrate §8.4 T26a",
    ...
    "failed_memory_ids": failed_ids_vec,  // NEW
});
```

**B. New `triple_backfill_failures` table** — `(run_id TEXT, memory_id TEXT, last_error TEXT, attempt_count INTEGER, failed_at REAL)`. More structured, supports cross-run aggregation.

Pick A unless we expect failure rates > 0.1% in production. At 14,881 memories, even 1% = 148 IDs — small JSON blob.

# Acceptance

1. After a run with simulated failures (e.g. counted mock extractor that fails every 7th), the failing memory_ids are recoverable from `backfill_runs.notes`.
2. Existing tests still pass.
3. Backwards compatible: notes without `failed_memory_ids` key parse fine.

# Scope

Out of scope: retry-on-restart logic (the resumability cursor already advances past failures). This is purely observability.

# Discovery context

T26c live run at the time of filing: PID 18943, processing 14,881 memories, target DB `/Users/potato/rustclaw/engram-memory-t26c.db`. Pace 33.8 mem/min, 0 failures so far at memory 604/14,881.
