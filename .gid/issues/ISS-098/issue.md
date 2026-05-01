---
id: ISS-098
title: user_metadata silently dropped in fact path of store_raw — RUN-0012 lost it (only 17/441 retained)
status: open
priority: P0
labels: [bug, data-correctness, eval, regression-guard]
created: 2026-05-01
relates_to: [ISS-087, ISS-088, ISS-091, ISS-094]
discovered_in: RUN-0012-iss091
---

# user_metadata silently dropped in fact path of store_raw

## TL;DR

In RUN-0012-iss091's ingested DB (locomo-conv26-full.db, 441 memories), only **17 / 441 ≈ 3.9%** of memories carry `user_metadata`. With current `main` HEAD (38c38fe, ISS-091), reproduction shows `user_metadata` is **fully retained** end-to-end. So RUN-0012 was ingested by an *earlier* binary in which the fact path of `store_raw` was dropping `meta.user_metadata` before persistence.

This means:

- All RUN-0012 retrieval numbers are on a corrupted-substrate DB
- RUN-0013 J-score (8% overall, evidence_recall ≈ 1%) is a downstream measurement of that corruption — **not** a real measurement of the retrieval algorithm
- We cannot trust any RUN-0012 / RUN-0013 conclusions about ISS-087 / ISS-088 / ISS-091 fixes until re-ingested

## Evidence

### Reproduction with current binary (works)

```bash
$ cargo build -p engram-cli --release
# Use the built binary to ingest a few facts with user_metadata
$ ./target/release/engram-cli ... store-raw \
    --content "Caroline went to the LGBTQ support group yesterday" \
    --user-metadata '{"dia_id": "D1:3", "session": "S05"}' \
    --occurred-at "2023-05-08T00:00:00Z"

$ sqlite3 test.db 'SELECT user_metadata FROM enriched_memories LIMIT 5'
{"dia_id":"D1:3","session":"S05","original_content":"Caroline went to the LGBTQ support group yesterday"}
# ✅ retained
```

### RUN-0012 DB inspection (broken)

```bash
$ sqlite3 .gid/eval-runs/RUN-0012-iss091/locomo-conv26-full.db \
    "SELECT COUNT(*) FROM enriched_memories"
441
$ sqlite3 .gid/eval-runs/RUN-0012-iss091/locomo-conv26-full.db \
    "SELECT COUNT(*) FROM enriched_memories WHERE user_metadata IS NOT NULL AND user_metadata != 'null'"
17
$ sqlite3 .gid/eval-runs/RUN-0012-iss091/locomo-conv26-full.db \
    "SELECT user_metadata FROM enriched_memories WHERE user_metadata IS NOT NULL AND user_metadata != 'null' LIMIT 3"
# Only `original_content` populated — caller-supplied dia_id / session NOT present
```

→ The 17 that survived only carry `original_content` (the ISS-088 inserted field) — they do **not** carry the `dia_id`, `session`, etc. that cogmembench's adapter passes in. So even those 17 had the caller's metadata stripped before reaching persistence.

## Suspected cause window

`store_raw` fact path (memory.rs ~line 3060–3090) currently does:

```rust
let user_md = {
    let mut base = meta.user_metadata.clone();   // ← ISS-088 commit f6bd93b added this branch
    if let Some(orig) = grounding_results[fact_idx].original_core_fact.as_ref() {
        if base.is_null() { base = serde_json::json!({}); }
        if let Some(obj) = base.as_object_mut() {
            obj.insert("original_content".to_string(), ...);
        }
    }
    base
};
```

Before f6bd93b (ISS-088), the fact path may not have threaded `meta.user_metadata` at all. RUN-0012 was launched 2026-04-30 23:39 EDT — **28 min after ISS-091 commit** but the ingesting binary's actual build time is unverified. Hypotheses:

1. Binary was built before ISS-088 (f6bd93b @ 19:46:50) → fact path didn't pass `user_metadata`
2. Binary was built between ISS-088 and ISS-091 but had a different bug
3. Some other regression in between

Root-cause via `git bisect` is **not in scope of this issue** — see ISS-099 (TBD) if we want to trace it.

## Fix

There is no code fix required against `main` HEAD. The current code is correct.

The fix is **regression guard**:

### (a) Unit test: `iss098_user_metadata_through_fact_path`

Test that `store_raw` with `meta.user_metadata = {dia_id: "D1:3", custom: "x"}` and content that produces a fact, after persistence:

- `enriched_memories.user_metadata` JSON must contain `dia_id: "D1:3"` AND `custom: "x"` AND (post-ISS-088) `original_content`
- Repeat with no facts extracted (chitchat) — Path A should also retain caller's user_metadata

### (b) Unit test: `iss098_user_metadata_through_no_fact_path`

Already implicitly covered by Path A (no_facts_extracted) at memory.rs:2951 — but make it explicit so we never silently regress.

### (c) Re-ingest + Re-evaluate

After tests are green:

1. Re-run `RUN-0012` ingest with fresh `release` binary → `RUN-0014-iss098-clean`
2. Re-run J-score eval on the new DB → `RUN-0015-jscore-clean`
3. Compare against RUN-0013's 8% — if substantially higher, ISS-087/088/091 fixes are vindicated; if still low, retrieval-engine root-cause work is still needed

## Acceptance criteria

- [ ] Unit test `iss098_user_metadata_through_fact_path` written and passing on HEAD
- [ ] Unit test `iss098_user_metadata_through_no_fact_path` written and passing on HEAD
- [ ] Re-ingest of conv26 (`RUN-0014-iss098-clean`) verifies user_metadata retention rate ≥ 99%
- [ ] J-score re-run (`RUN-0015-jscore-clean`) recorded with comparison to RUN-0013

## Why P0

- All recent eval numbers (RUN-0012, RUN-0013) are silently invalidated by this — major decision-blocker
- Risk of silently re-introducing the bug in any future change to `store_raw` fact path is high; without a regression test this will keep biting
- Cost of fix is small (~30 min for tests) compared to cost of misleading eval numbers

## Notes

- This finding is NOT covered by ISS-094 (cogmembench adapter dropping temporal at recall) — that is a separate, render-side issue. ISS-098 is substrate-side, ingest-time data loss.
- Discovered 2026-05-01 during analysis of RUN-0013 J-score = 8% (suspicious low evidence_recall ≈ 1%).
