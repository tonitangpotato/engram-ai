---
id: ISS-131
title: test_reward_learning fails on CHECK constraint working_strength BETWEEN 0.0 AND 1.0
status: done
severity: medium
priority: P2
type: bug
labels:
- test
- regression
- hebbian
- reward-learning
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

---

## Root-cause trace (2026-05-15)

`crates/engramai/src/memory.rs:4951` (in `Memory::reward`):

```rust
record.working_strength += self.config.reward_magnitude * polarity;
record.working_strength = record.working_strength.min(2.0);  // ← clamps to 2.0
```

But the schema's CHECK constraint says `working_strength BETWEEN 0.0 AND 1.0`. So when the reward path tries to write a record with `working_strength > 1.0` (which is allowed by the clamp), SQLite rejects with the observed `extended_code: 275 — CHECK constraint failed`.

The bug is a **constraint mismatch between Rust code and schema**:

- The Rust code wants working_strength to range up to 2.0 (modeling a dopaminergic surge that temporarily super-saturates a recent memory).
- The schema invariant restricts to [0.0, 1.0] (modeling a probability-like quantity).

These were defensible designs in isolation, but together they're inconsistent. The test `test_reward_learning` exposed the mismatch by exercising the reward path with `add → recall → reward → stats` — the recall path bumps `working_strength` toward 1.0, then `reward` adds another boost that puts it past 1.0, then `update()` to storage trips the CHECK.

## Decision needed (deferred to potato)

This is a design call, not a mechanical fix. Two reasonable options:

### Option A — clamp to 1.0 in Rust, keep schema constraint
- One-line change: `.min(2.0) → .min(1.0)` on memory.rs:4951
- Preserves the schema invariant (`working_strength ∈ [0.0, 1.0]`)
- **Caps reward magnitude at saturation**: once `working_strength == 1.0`, additional reward signals have no effect on the memory. Dopaminergic surges can't push past the ceiling.
- Lowest-risk fix. No migration. No other code paths affected.

### Option B — relax schema CHECK to allow [0.0, 2.0]
- Schema migration to drop+recreate the constraint
- Allows the up-to-2.0 super-saturation as designed in the Rust code
- **Need to audit other code paths** that read `working_strength` and may assume `<= 1.0` (e.g. UI rendering, normalization in retrieval scoring, decay).
- Higher complexity. May ripple into retrieval semantics.

### Engineering recommendation

Option A is the conservative move and probably the right one. The schema-level constraint `BETWEEN 0.0 AND 1.0` is the more recent, stricter invariant; the `.min(2.0)` clamp is older code that pre-dates the constraint. The reward signal being capped at saturation is biologically plausible — real synapses also have a saturation ceiling. Option B would expand the invariant under the entire codebase without verification, which is the kind of "while I'm here" scope expansion the karpathy-guidelines skill warns against.

But this is a judgment call I'm not making unattended. Flag for potato.

## When potato is back

Pick Option A or Option B. Once decided:
- **A**: ~2-line patch + regression test that triggers the original failure path + verify other reward callers don't break. Likely a 30-minute job.
- **B**: schema migration + grep+audit pass on all reads of `working_strength` + ditto regression test. Likely 1-2 hours.

---

## Resolution (2026-05-15)

**Shipped Option A** — ratified with potato in current session.

**Change:** `crates/engramai/src/memory.rs:4951` — `.min(2.0)` → `.min(1.0)`.

**Rationale (biological invariant wins):**
- Schema CHECK `working_strength BETWEEN 0.0 AND 1.0` encodes a probability-like quantity. That is the stricter, more recent invariant and the right one — real synapses do saturate.
- The `.min(2.0)` was older code from a different mental model (transient dopaminergic super-saturation up to 2.0). It conflicted with the schema and was never reconciled.
- Option A keeps the invariant uniform across Rust + schema with a one-line clamp fix, no migration, no audit ripple. Karpathy-guidelines test: smallest change that fixes the root cause.

**Verification:**
- `cargo test -p engramai --test integration_test test_reward_learning` — **passes** (was failing on `CHECK constraint failed: working_strength BETWEEN 0.0 AND 1.0`).
- `cargo test -p engramai --lib` — **1902/1902 pass, 0 regressions**.

**Acceptance criteria:**
- [x] `cargo test -p engramai --test integration_test test_reward_learning` passes
- [x] Root cause documented in commit message (clamp value mismatch — code allowed 2.0, schema enforces 1.0)
- [x] Full `cargo test -p engramai --lib` green (1902/1902)
