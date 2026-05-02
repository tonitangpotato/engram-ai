# RUN-0014 — ABORTED 2026-05-01

This run was started to validate the ISS-098 fix (user_metadata retention through `store_raw` fact path), but was aborted after investigation revealed that ISS-098's diagnosis was wrong. See ISS-098 update from 2026-05-01.

## What happened

1. `_launch.sh` started `01_ingest.py` (pid 38947) and `_watcher.sh`
2. `_watcher.sh` used `setsid` to detach the watcher — but macOS doesn't ship `setsid`, so the watcher launch failed silently
3. The ingest pid was orphaned and died on its own (subprocess.run timeouts piling up with no parent supervision)
4. By the time we noticed, no `02_retrieve.sh` had been triggered

## Why we stopped re-ingesting

ISS-098 investigation showed:
- `user_metadata` was never populated by ANY RUN-* script (incl. RUN-0014) — they don't pass `--meta`
- So re-ingesting wouldn't produce different `user_metadata` than RUN-0012
- `store_raw` fact path is fine; the bug was in the ingest script template, not the substrate
- Retrieval scoring doesn't use `user_metadata` anyway (matches via `--source`)

## What's preserved

- `locomo-conv26-full.db` partial DB — left as-is for hit@5 cross-comparison if needed
- ingest.log / watcher.log for forensics

## What to do next

- See ISS-099 for the actual ingest-script bug
- See ISS-093 for the J-score root cause (not user_metadata)
- See ISS-098 for the misdiagnosis post-mortem
