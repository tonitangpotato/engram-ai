# RUN-0017: J-score real test (post ISS-087/088/099)

**Started:** 2026-05-05 10:09 EDT
**Conversation:** conv-26 only (199 questions)
**Mode:** retrieve+judge against existing DB (no re-ingest)

## Hypothesis

After ISS-087 (occurred_at threading), ISS-088 (temporal grounding for
extracted facts), and ISS-099 (dia_id meta passthrough), the conv-26
DB now contains:
- 458 memories all anchored to 2023 epoch (occurred_at honored)
- 17 causal facts with absolute date markers inline (e.g. "last week (2023-06-19)")
- 458/458 rows tagged with metadata.user.dia_id (D{session}:{turn})

J-score should be measurably higher than RUN-0013's 8.0% / 1.0% ev_recall,
which was run *before* the dia_id fix.

## Setup

- engram binary: /Users/potato/clawd/projects/engram/target/release/engram
  (built May 1 15:38, sha unknown — pre-current HEAD d54a3e1 which is just
  a refactor split, no behavior change)
- engram repo HEAD: d54a3e1 (refactor: split engram-bench into standalone repo)
- Last behavior-affecting commits in DB:
  - f6bd93b ISS-088 temporal grounding
  - 815b319 ISS-087 occurred_at threading
  - (ISS-099 dia_id fix in cogmembench, applied to DB build)
- cogmembench HEAD: 4fe0913 + uncommitted reindex --ns→-p patch
- DB built: 2026-05-01 22:19 (post-dia_id-fix)
- DB backup: conv-26.db.before-RUN-0017

## Reference numbers

| RUN | J-score | ev_recall | Notes |
|-----|---------|-----------|-------|
| RUN-0009 | 52.7% recall@5 | n/a | substrate-only hit@5 (different metric) |
| RUN-0013 | 8.0% (16/199) | 1.0% | pre-dia_id-fix |
| RUN-0017 | ? | ? | **this run** |

## Outcome thresholds

- < 15%: marginal — extractor coverage (60 facts / 419 turns) is the next blocker
- 15–30%: ISS-087/088/099 working; retrieval is next bottleneck
- \> 30%: stack healthy; ISS-085 J-score arc can close

---

## Result: ABORTED at 28/199 (3.6% / ev_recall=0%)

Killed at question 28. Trend was strictly worse than RUN-0013.

## Real blocker discovered: ISS-103

Investigation found 457 of 458 conv-26 memories were soft-deleted on
2026-05-02 02:19, ~4 hours after ingest. Cause:

- ISS-087 set `created_at` = gold session date (2023)
- Ebbinghaus decay computes age as `now - created_at`
- 2026 - 2023 = ~3 years → effective_strength < 0.1
- `check_decay_and_flag` auto-soft-deleted everything

ISS-087/088/099 are individually correct but ISS-087 collides with
the lifecycle subsystem. **No retrieval improvement is possible
until ISS-103 is fixed** — the ground-truth rows are deleted.

## Recommendation

1. Fix ISS-103 (separate `created_at` from `occurred_at` — Option A)
2. Re-ingest conv-26
3. Re-run this eval (RUN-0018)

