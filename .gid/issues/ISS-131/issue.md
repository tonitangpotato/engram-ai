---
id: ISS-131
title: test_reward_learning fails on CHECK constraint working_strength BETWEEN 0.0 AND 1.0
status: open
severity: medium
priority: P2
type: bug
labels: [test, regression, hebbian, reward-learning]
relates_to: []
created: 2026-05-15
---

# Summary

`crates/engramai/tests/integration_test.rs::test_reward_learning` panics at line 98 with:

```
SqliteFailure(Error { code: ConstraintViolation, extended_code: 275 },
  Some("CHECK constraint failed: working_strength BETWEEN 0.0 AND 1.0"))
```

## Evidence (reproduction)

Verified on engram HEAD `f2edff2` (clean — no working-tree changes):

```bash
cargo test -p engramai --test integration_test test_reward_learning -- --nocapture
```

→ `FAILED. 0 passed; 1 failed`

Stashed working-tree changes (ISS-112 §C/D/E/F refactor in `substrate/backfill.rs`)
before reproducing — failure is **not** caused by Phase C backfill polish work.
Re-confirmed after `git stash pop`: failure is pre-existing and independent.

## Hypothesis

A reward-learning code path multiplies / accumulates `working_strength` past `1.0`
without clamping, and the schema-level CHECK constraint trips. Likely culprits:

- `crates/engramai/src/association/` — Hebbian update code that mutates `working_strength`
- Reward signal application (`apply_reward` or similar) — gain factor may push over 1.0
- Test setup may apply rewards repeatedly without re-clamping

## Why filing instead of fixing now

- Unrelated to current Phase C backfill polish work
- Needs targeted investigation of Hebbian / reward update math, not backfill plumbing
- Pre-existing — does not block ISS-112 commit batch
- Test is on `--test integration_test`, not in the lib (`cargo test -p engramai --lib`
  still passes 1902/1902), so day-to-day dev isn't blocked

## Fix sketch

1. Trace the write path that sets `working_strength` in the reward-learning test
2. Identify whether the bug is (a) missing clamp, (b) wrong delta, or (c) test seeding above 1.0
3. Add a clamp `working_strength = working_strength.clamp(0.0, 1.0)` at the write site,
   OR fix the math if the value should never go out-of-bounds in the first place
4. Add a regression test that exercises the exact same sequence as `test_reward_learning`
5. Run full `cargo test -p engramai --tests` to confirm green

## Acceptance criteria

- [ ] `cargo test -p engramai --test integration_test test_reward_learning` passes
- [ ] Root cause documented in commit message (clamp vs math fix vs seeding fix)
- [ ] Full `cargo test -p engramai --tests` green
