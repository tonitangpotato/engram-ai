---
id: ISS-129
title: 'T26c rerun: conservative retry config for the 7306 remaining memories'
status: open
priority: P2
severity: minor
created: 2026-05-15
depends_on: [ISS-128]
relates_to: [ISS-127, ISS-128]
labels: [substrate, backfill, v04, retry]
---

# Problem

T26c (2026-05-14) terminated early after ~7h, leaving the v0.4 substrate
backfill incomplete on the production memory corpus:

| metric | value |
|---|---|
| total non-deleted memories | 14,881 |
| attempted | 12,889 (86.6%) |
| succeeded | 7,575 |
| **failed (exhausted max_retries)** | **5,314** (41.2% of attempted) |
| never attempted | 1,992 |
| triples written to clone DB | 27,423 |

Full archive: `.gid/features/v04-unified-substrate/operational-runs/T26c-partial-2026-05-14.md`

The 7,575 successful memories' triples are clean (mean conf 0.826, 0.5%
sentence fragments, 10/27423 self-loops, healthy predicate spread —
matches the T26b sample that received PROCEED). But the 5,314 failures
need a rerun before v0.4 Phase D / T39 (legacy table drop) can proceed
with confidence.

# Why a rerun is not "just run the same command again"

The original config (`--rps 15`, default `max_retries=3`, default backoff)
hit a ~41% failure rate that the 100-memory T26b sample (also `--rps 15`,
same config, ~3 min wall clock) did not reproduce. Hypothesis: a
sustained-load failure mode — burst-pattern rate limit, TPM ceiling, or
connection-staleness. None of these is verified because **ISS-128 means
we have no persisted failed memory_ids and no captured HTTP error
bodies**.

Re-running with the same config risks the same outcome.

# Fix

1. **Block on ISS-128 landing first.** Without persisted failure IDs the
   rerun has to re-iterate the entire 14,881-memory corpus and rely on
   idempotent skip — workable but inelegant, and we still won't have
   ground truth on what the original failures actually were.
2. **Conservative config for the rerun:**
   - `--rps 8` (down from 15; halves sustained load)
   - `max_retries=5` (up from default 3; absorbs transient errors)
   - `base_backoff=5s` (up from default; smoother retry)
   - Run attended during daytime so a human can catch new failure modes
     within the first hour, not 6 hours in
3. **Capture stderr to a file.** The original `run.log` only had the
   startup line — stderr was lost. This is partly an ISS-128 concern but
   also an operator-script concern.
4. **Target the remaining 7,306 only.** With ISS-128 in place, the
   driver can be given a list of memory_ids = (5314 failed ∪ 1992
   untouched) and skip everything else.

# Acceptance

1. After ISS-128 lands, a rerun against `engram-memory-t26c.db` produces
   triples for ≥95% of the 7,306 target memories (success rate ≥95%, vs
   58.8% on the original run).
2. Post-rerun, total triple-bearing memories ≥ 14,000 (out of 14,881),
   confirming the corpus is substantively covered.
3. Quality script (`t26c-review.sh`) shows numbers no worse than the
   current partial run (mean conf ≥0.80, sentence fragments ≤1%,
   self-loops ≤0.1%).
4. After acceptance criteria 1–3 pass, the clone DB triples are merged
   to prod via:
   ```sql
   ATTACH '/Users/potato/rustclaw/engram-memory-t26c.db' AS clone;
   INSERT OR IGNORE INTO triples SELECT * FROM clone.triples;
   DETACH clone;
   ```
   …or equivalent helper.

# Scope

In scope: rerun, merge to prod, mark T26c done in v0.4 design §8.4.

Out of scope:
- Diagnosing the original 41% failure rate post-hoc. The
  ISS-128-persisted error bodies from the *rerun* are the data we'll
  use. The original run's failures are lost.
- Changing the triple-backfill driver's retry strategy as a
  permanent default — these knobs are run-specific tuning.

# Discovery context

Filed 2026-05-15 00:21 EDT after T26c (PID 18943) terminated early.
Decision agreed with potato: accept partial as v0.4 reference fixture,
do not merge to prod, queue rerun behind ISS-128.
