---
id: ISS-098
title: user_metadata silently dropped in fact path of store_raw — RUN-0012 lost it (only 17/441 retained)
status: done
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

---

## UPDATE 2026-05-01 (post-investigation): Issue diagnosis was wrong

After running RUN-0014 to validate the fix and finding it stuck (watcher dead, ingest pid orphaned), a deeper investigation showed **the original diagnosis in this issue is incorrect**. Pinning it here so we don't repeat the mistake.

### What the issue claimed
- Fact path of `store_raw` was silently dropping caller's `meta.user_metadata`
- 17/441 retention in RUN-0012 was a substrate bug
- Re-ingest with HEAD binary should fix it

### What is actually true

1. **`store_raw` fact path is fine** (and was fine in the binary that ingested RUN-0012). Manual reproduction:
   ```bash
   ./target/release/engram --database /tmp/iss098-test.db store \
     "Caroline went to LGBTQ support group yesterday" \
     -n test-iss098 -t episodic -i 0.6 \
     -s "test/D1:3" --extractor anthropic --oauth \
     --meta dia_id=D1:3 --meta session=S05
   # → metadata.user = {"dia_id":"D1:3","session":"S05"} ✅
   ```
   user_metadata is preserved end-to-end.

2. **The real cause: `.gid/eval-runs/RUN-NNNN/01_ingest.py` never passed `--meta`.** All 9 RUN-* ingest scripts (RUN-0005 through RUN-0014) inherit from the same hand-written template that only passes `-s "locomo/{conv_id}/{dia_id}"` — never `--meta dia_id=...`. So `user_metadata` was never populated *because the caller never set it*, not because `store_raw` dropped it.
   ```bash
   $ grep -l "\-\-meta" .gid/eval-runs/RUN-*/01_ingest.py
   # (empty — none of them pass --meta)
   ```

3. **The 17/441 in RUN-0012 are not "survivors"** — they're the rows where the ISS-088 temporal grounding code injected `original_content` into `user_metadata`. Caller still passed `null`; the field appears non-empty only because of that injection.

4. **There is a separate cogmembench `EngramAdapter.ingest_conversation`** in `cogmembench/benchmarks/locomo/engram_adapter.py` that *does* pass `meta={"dia_id": ..., "session": ..., ...}` to `--meta`. **But none of the RUN-* eval scripts use it** — they bypass the adapter and `subprocess.run(engram_bin, ...)` directly. So the issue conflated two ingest paths.

### Why RUN-0013's 8% J-score is NOT explained by this

`locomo_conv26_retrieval.rs:212` matches dia_id from `record.source` (i.e. the `--source "locomo/conv-26/D7:22"` field), not from `user_metadata`. So retrieval evidence_recall *never depended* on user_metadata being populated. The 8% J-score has a different root cause — most likely **ISS-093** (Hybrid plan recency-dump).

### Decisions

- **Re-ingest is not necessary.** RUN-0014/0015 do not need to complete. `user_metadata` will still be empty after re-ingest because the script doesn't populate it.
- **The regression tests added in commit b462016 are still valuable** — they pin the fact-path contract going forward, even though the bug they "guard" against was never real.
- **Acceptance criterion (3) "user_metadata retention rate ≥ 99%" is unachievable with current scripts** and should be removed.
- **New issue ISS-099 will track the real ingest-script bug** (script bypasses adapter, drops `--meta`).

### Status change

Marking this issue as **resolved-by-misdiagnosis** rather than rolling forward — the regression tests stay, but the original "P0 substrate corruption" framing was wrong. J-score investigation moves to ISS-093 / ISS-099.

---

## UPDATE 2026-05-01 (post-RUN-0013 log analysis): Re-correction

I was partially wrong in the previous update. The `user_metadata` empty state IS the root cause of RUN-0013's 8% J-score — but the bug is *caller-side* (ingest script), not *substrate-side* (`store_raw`).

### Causal chain (corrected)

1. `01_ingest.py` (eval-run script) does NOT pass `--meta dia_id=...` → `metadata.user` stays null
2. cogmembench `EngramAdapter.recall_for_question` reads `metadata.user.dia_id` to prepend `[D1:3]` to `raw_text` → fails silently, returns bare content
3. cogmembench `compute_evidence_recall` does `dia_id in text` substring-match → always 0 (verified: `SELECT COUNT(*) FROM memories WHERE content LIKE '%D1:3%'` = 0 in RUN-0012 DB)
4. → 197/199 questions have evidence_recall=0%
5. → LLM judge gets context without dia_id markers → can't ground answers → "I don't know" mass → J-score=8%

### So the substrate is innocent; the caller is the culprit

- `store_raw` fact path: ✅ correct (verified by manual repro + regression tests b462016)
- `01_ingest.py` ingest script: ❌ never set `user_metadata` in the first place

### Status of this issue

Closing as **resolved-by-misdiagnosis**:
- The fact-path regression tests (commit b462016) stay — they're still valuable as a future regression guard
- "P0 substrate bug" framing was wrong
- The actual P0 work moves to **ISS-099** (caller-side ingest fix)
- Re-ingest IS warranted, but only after ISS-099 patches the script template
