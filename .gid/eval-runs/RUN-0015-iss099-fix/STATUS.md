# RUN-0015 — ISS-099 fix verification

**Started:** 2026-05-01 19:48 ET
**PID:** 39799
**Goal:** Verify ISS-099 fix (`01_ingest.py` passes `--meta dia_id/speaker/session_*`) breaks the chain that caused RUN-0013 J-score = 8%.

## Delta vs RUN-0014
- Same script, same dataset, same binary (38c38fe)
- **Only change:** added 4-5 `--meta` flags per `engram store` call (dia_id, speaker, session_num, turn_index, session_date)
- All other CLI flags identical

## Smoke test (pre-launch)
1-turn dry-run on `/tmp/smoke.db` confirmed:
```
metadata.user = {
  "dia_id":"D1:1",
  "session_date":"1:56 pm on 8 May, 2023",
  "session_num":1,
  "speaker":"Caroline",
  "turn_index":0
}
```
✅ Side-channel reaches DB.

## Live progress (T+39s)
- 13 / 419 turns ingested
- `metadata.user.dia_id` populated on all 3 sampled rows
- Estimated total: ~1h25m

## Next phase (after ingest finishes)
1. **Verify metadata coverage**: SQL count `metadata.user.dia_id IS NOT NULL` → expect ≥ 95% of rows
2. **Re-run J-score on first 25 Qs only** (≈ 2 min, ≈ 25 LLM judge calls)
3. **Compare evidence_recall**: RUN-0013 = 1% (197/199 zeros). Target after fix: > 0%, ideally > 30%.
4. **If ev_recall jumps**: causal chain confirmed → run full 199-Q J-score.
5. **If ev_recall still ~0%**: there's another hop in the chain we missed → investigate.

## ISS links
- Verifies fix for: ISS-099 (P0)
- Re-tests claim from: ISS-098 (now status=done, misdiagnosis acknowledged)
- Unblocks: ISS-093 (recency-dump) only if this turns out to NOT be the full root cause
