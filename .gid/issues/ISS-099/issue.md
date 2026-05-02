---
blocks: .gid/issues/ISS-093/issue.md
---
# .gid/issues/ISS-099/issue.md (issue)
project: engram
---
id: ISS-099
title: eval-runs ingest scripts bypass cogmembench adapter — never populate user_metadata channel
status: open
priority: P0
labels: [eval, infra, hygiene, technical-debt]
created: 2026-05-01
relates_to: [ISS-098, ISS-094]
discovered_in: ISS-098 investigation
---

# eval-runs ingest scripts bypass cogmembench adapter — never populate user_metadata channel

## TL;DR

`.gid/eval-runs/RUN-NNNN/01_ingest.py` (every run from RUN-0005 through RUN-0014) is a hand-rolled `subprocess.run(engram_bin, ...)` loop that only sets `-s "locomo/{conv_id}/{dia_id}"` (the source field) and **never passes `--meta dia_id=... --meta session=...`**.

Meanwhile `cogmembench/benchmarks/locomo/engram_adapter.py::ingest_conversation` *does* pass `meta={"dia_id": ..., "speaker": ..., "session_date": ..., "session_num": ..., "turn_index": ...}` via the metadata side-channel. **But the eval scripts don't use the adapter.** So we have two parallel ingest paths and the one we actually run is the impoverished one.

This is **why ISS-098 was misdiagnosed** as a substrate `store_raw` bug — the issue author assumed the adapter was being used, when in reality the eval-run scripts bypass it entirely.

## Impact

- `user_metadata` (per-memory side-channel) has been empty across all locomo eval runs ever taken
- Any future retrieval / scoring code that wants to filter by `dia_id`, `session_num`, `turn_index` etc. via `user_metadata` will silently see nothing
- Currently no scoring depends on it (`locomo_conv26_retrieval.rs:212` reads dia_id from `record.source.rsplit('/').next()`, not from `user_metadata`), so this is dormant tech debt — not an active eval-correctness bug
- **However**, ISS-094 (cogmembench adapter dropping temporal at recall) and any future user_metadata-driven retrieval would surface this hole the moment it tried to filter on those fields

## Evidence

```bash
$ grep -l "\-\-meta" /Users/potato/clawd/projects/engram/.gid/eval-runs/RUN-*/01_ingest.py
# (empty — no run has ever passed --meta)

$ diff RUN-0012-iss091/01_ingest.py RUN-0014-iss098-clean/01_ingest.py
# only OUT_DIR differs — confirming all runs descend from one template

$ grep -A 5 'meta = {' /Users/potato/clawd/projects/cogmembench/benchmarks/locomo/engram_adapter.py
meta = {
    "dia_id": turn.dia_id,
    "speaker": turn.speaker,
    "session_date": session_date,
    "session_num": session.session_num,
    "turn_index": ti,
    ...
}
# adapter already does the right thing
```

## Two paths to fix

### Option A (cheap, minimal): patch the script template

Add to the `cmd = [...]` list in `01_ingest.py`:

```python
"--meta", f"dia_id={dia_id}",
"--meta", f"speaker={speaker}",
"--meta", f"session={sk}",
```

Pro: one-file change, no architectural movement.  
Con: keeps the two-paths-doing-the-same-thing problem.

### Option B (root fix): make eval scripts call the adapter

Replace the hand-rolled subprocess loop with a thin Python wrapper that imports `cogmembench.benchmarks.locomo.engram_adapter::EngramAdapter` and calls `adapter.ingest_conversation(conv)`. The adapter already handles `--meta`, `--occurred-at`, retry logic, etc.

Pro: single source of truth; future improvements to the adapter automatically propagate to evals.  
Con: cross-repo Python import setup, slightly more plumbing.

**Recommended: Option A as immediate band-aid (so next eval run has dia_id in user_metadata), Option B as a follow-up cleanup.**

## Acceptance criteria

- [ ] New ingest run (or re-ingest) populates `user_metadata.dia_id` / `.session` / `.speaker` for ≥99% of memories
- [ ] Sanity SQL: `SELECT COUNT(*) FROM enriched_memories WHERE json_extract(user_metadata, '$.dia_id') IS NOT NULL` returns ≈ total memory count
- [ ] Document in `.gid/eval-runs/README.md` (or equivalent) that eval ingest must use the adapter / `--meta` channel — prevent future drift

## Why P2 (not P0/P1)

- No current eval metric depends on `user_metadata` (retrieval matches via `--source`)
- Closing this is hygiene + future-proofing, not unblocking
- Will become P1 the moment any retrieval rule starts filtering on `user_metadata`

## Notes

- ISS-098's regression tests still apply and are still valuable — they ensure the substrate honors `user_metadata` if it's ever populated
- This issue is the *caller-side* counterpart to ISS-098's *substrate-side* (false-alarm) concern
- Discovered 2026-05-01 during ISS-098 investigation

---

## UPDATE 2026-05-01 — Priority upgrade to P0; this IS the J-score root cause

After digging into RUN-0013-jscore.log, **this issue IS the cause of RUN-0013's 8% J-score** (not ISS-093 / ISS-098 / retrieve algorithm). Causal chain:

1. `01_ingest.py` doesn't pass `--meta dia_id=...` → `user_metadata` empty in DB (verified: `SELECT json_extract(metadata,'$.user') FROM memories` is null for ~424/441 rows)
2. cogmembench `EngramAdapter.recall_for_question` reads `metadata.user.dia_id` → finds nothing → falls back to bare content (no dia_id prefix)
3. cogmembench `compute_evidence_recall` does substring-match `dia_id in text` on those bare contents → always 0 (verified: `SELECT COUNT(*) FROM memories WHERE content LIKE '%D1:3%'` = 0)
4. Result: **197/199 questions have evidence_recall=0%** (only 2 happen to mention dia_id in answer text by coincidence)
5. LLM judge sees retrieval context without evidence → answers "I don't know" en masse → **J-score = 8.0%**

This is *not* a retrieve algorithm bug. The retrieve pipeline may be returning correct memories — we just can't tell, because the evaluator's evidence-attribution mechanism (substring-match on dia_id) is broken at the data layer.

### Priority change: P2 → P0

- This single fix (~3 lines in `01_ingest.py`) likely unblocks J-score evaluation entirely
- Without this, every J-score number we've reported is meaningless
- Cheaper than any retrieve-algorithm work, and a hard prerequisite for it

### Concrete fix

In `.gid/eval-runs/RUN-NNNN/01_ingest.py`, in the `cmd = [...]` block, add:

```python
"--meta", f"dia_id={dia_id}",
"--meta", f"speaker={speaker}",
"--meta", f"session={sk}",
```

Re-ingest conv-26 → re-run RUN-0013-style J-score → expect dramatic delta. If J-score still ~8% after fix, *then* the retrieve algorithm needs work (ISS-093 etc.). But this fix must come first.

### Acceptance (revised)

- [ ] Patch `01_ingest.py` template with `--meta` flags
- [ ] New ingest (RUN-0015 or repurpose RUN-0014 location after wipe) populates `user_metadata.dia_id` ≥ 99%
- [ ] Re-run J-score; record delta vs RUN-0013's 8.0%
- [ ] If delta is large (>20pp), close ISS-099 and revisit ISS-093 priority
- [ ] If delta is small, escalate retrieve-algorithm work
