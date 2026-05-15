---
id: ISS-112
kind: issue
status: open
priority: P2
severity: hardening
labels: [v04, phase-c, backfill, follow-up]
created_at: 2026-05-13
relates_to: [v04-unified-substrate]
---

# v04 Phase C backfill polish: audit durability, test gaps, merge silent-drops

Polish bucket from the T21 review (`.gid/features/v04-unified-substrate/reviews/t21-r1.md`, round 1). Deferred from the T21 commit because most are cross-cutting across T19/T20/T21/T22-T25 — better to batch after all Phase C drivers are shipped.

Do this AFTER T22-T25 are shipped, in a single cleanup commit.

## Scope

### A. Audit row durability (was FINDING-4)

**Problem**: The `backfill_runs` audit row is INSERTed BEFORE the work tx (`finished_at=NULL`) and UPDATEd AFTER `tx.commit()`. Three unprotected failure windows:

1. Crash during work tx → tx rolls back, audit row stuck `finished_at=NULL, rows_*=0` forever.
2. Crash between commit and audit UPDATE → data committed, audit row misleadingly says 0 rows.
3. Concurrent runs (same legacy_table, different namespaces) → two open audit rows, no way to tell from audit alone which run owned which inserts.

Affects: T19 (memories), T20 (memory_embeddings), T21 (entities). Will also affect T22-T25 if pattern is copied.

**Fix**: Move both audit INSERT and audit UPDATE INSIDE the work tx. Crash before commit → no audit row at all (consistent with no data). Refactor a shared helper `audit_run_open(&tx, ...) -> Uuid` / `audit_run_close(&tx, run_id, counts)` and call from inside `tx` in all drivers.

**Verification**: simulate a panic mid-work, assert audit table has no leak.

---

### B. Test gaps for T21 contracts (was FINDING-3)

Missing tests in `crates/engramai/tests/v04_phase_c_backfill_entities.rs`:

1. **Re-seed with mutated metadata**: run T21, UPDATE `entities.metadata` for an id, re-run T21. Expected: new keys land, existing keys stay (existing-wins).
2. **Metadata with reserved column keys**: `entities.metadata = '{"namespace":"other","id":"different"}'`. Expected: lands as plain attribute keys, does NOT corrupt column values. Pins behavior so any future "flatten attributes into columns" refactor breaks loudly.
3. **`entity_type = ''`** (empty string but NOT NULL): row lands, `attributes.entity_type = ''`.
4. **Pass 2 with corrupt existing attributes** (`attributes='null'`, `'[]'`, `'"string"'`): driver does not crash, legacy keys are dropped (current behavior), audit notes surface a new counter `rows_existing_attrs_not_object` (see Part C).

Same gap audit pass should be done for T19 (memories), T20 (embeddings), T22-T25 once they exist.

---

### C. `merge_attributes_existing_wins` silent drop on non-Object existing (was FINDING-7)

**Problem**: If `nodes.attributes` is `'null'` / `'[]'` / `'"string"'` (corrupt data), `merge_attributes_existing_wins` returns existing unchanged and silently drops the new keys. Zero telemetry.

**Fix**:
- Add audit counter `rows_existing_attrs_not_object` (T21 audit notes JSON).
- Don't fail the row — same defensive philosophy as malformed legacy metadata.
- Test #4 from Part B pins the behavior.

---

### D. Pass 2 `updated_at` noise on idempotent rerun (was FINDING-5)

**Problem**: Pass 2 unconditionally `UPDATE nodes SET attributes = ?, updated_at = ?` even when the merge result is byte-identical to existing attributes. Every backfill rerun bumps `updated_at` for every Pass-2 row.

**Impact**: Downstream consumers using `updated_at > X` as a "changed since X" filter see false positives after each backfill rerun.

**Fix**: Diff the merge result against existing attributes; skip the UPDATE if identical. Cheap — already have both strings in scope.

Add regression test: rerun T21 twice, assert `updated_at` is byte-identical between runs for unchanged rows.

---

### E. Audit subset-counter clarity (was FINDING-6)

**Problem**: `rows_kind_mismatch` (T21 audit notes) is a strict subset of `rows_skipped_existing`. Naming doesn't surface this; an audit reader summing `inserted + skipped + kind_mismatch` could double-count.

**Fix**: Rename to `rows_skipped_kind_mismatch` so the `rows_skipped_*` prefix signals the subset relationship. Add a comment to the notes JSON construction stating the invariant.

---

### F. T21 module docs: T13 ordering caveat (was FINDING-2, downgraded)

**Context**: T13's `dual_write_entity_to_nodes` uses `ON CONFLICT DO UPDATE SET attributes = excluded.attributes` — unconditional overwrite. This is **intentional** because T13's `Entity` is the canonical post-resolution view; the legacy `entities.metadata` snapshot is older. NOT a bug.

But the ordering caveat is non-obvious: if T21 runs first and T13 later, T13 will clobber the legacy metadata keys T21 just merged in. Operators reading the T21 docs should know this.

**Fix**: Add a `## Ordering with T13` section to T21's module docstring in `crates/engramai/src/substrate/backfill.rs`. Explain that T21 → T13 ordering loses legacy-only metadata keys by design, and that the canonical mitigation is to run T21 BEFORE re-enabling the resolution pipeline writes (i.e., Phase C completes before Phase D).

This is doc-only. No code change.

---

## Acceptance

- [ ] A: Audit INSERT+UPDATE moved inside work tx in T19, T20, T21 (and T22-T25 if shipped). Helper extracted.
- [ ] A: regression test — panic mid-tx → no audit row leaked.
- [x] B: 4 new tests added to `v04_phase_c_backfill_entities.rs`. (commit `eca36d6` shipped B-#4 as `iss112_c_corrupt_existing_attributes_surfaced_in_counter`; B-#1/#2/#3 land in the §B commit.)
- [ ] B: similar test-gap audit applied to T19, T20 (and T22-T25 if shipped).
- [x] C: `rows_existing_attrs_not_object` counter added; B-#4 test verifies it. (commit `eca36d6`, 2026-05-15) — MergeOutcome enum surfaces ExistingNotObject/NewNotObject; T21 + T22 both count under new audit notes key.
- [x] D: Pass 2 skips UPDATE when byte-identical; regression test asserts `updated_at` stability. (commit `eca36d6`, 2026-05-15) — diff-and-skip applied to T21 + T22; 2 regression tests pinned.
- [x] E: `rows_skipped_kind_mismatch` rename; invariant comment added. (commit `eca36d6`, 2026-05-15) — applied to both T21 and T22, regression test `iss112_e_kind_mismatch_emits_under_skipped_prefix` added.
- [x] F: T21 module docstring has "Ordering with T13" section. (commit `eca36d6`, 2026-05-15)

## Progress notes (2026-05-15)

- §C/D/E/F shipped in `eca36d6` (4 files, +536 / -23). All tests green.
- §B partial→done-on-T21: B-#1/#2/#3 landed in `3ddf980`. B-#4 was already covered by `iss112_c_corrupt_existing_attributes_surfaced_in_counter`. Remaining §B work is the **cross-driver test-gap audit** for T19, T20, T22-T25 (separate from T21).
- §A: **scope re-evaluated 2026-05-15, T19 portion shipped, rest closed as WONTFIX.**
  - On second pass the original §A motivation listed three failure scenarios. First-principles re-evaluation with potato (2026-05-15):
    - **Scenario 1** ("audit row stuck `finished_at=NULL, rows_*=0` after crash"): this is **not a bug, it's the crash-detector affordance**. `WHERE finished_at IS NULL` is the documented operator query to find orphan/crashed runs (see `backfill.rs` module rustdoc). Putting audit inside tx erases this. No fix needed; clarify in docs instead.
    - **Scenario 2** ("crash between commit and audit UPDATE → audit says 0 rows but data is committed"): real bug but microsecond window. Verify (`check_audit_consistency`) already detects the inconsistency post-hoc.
    - **Scenario 3** ("concurrent runs, two open audit rows"): not solvable by audit-in-tx — runs are still distinct tx with distinct rows. UUID `run_id` already disambiguates. Not in §A scope.
  - The **real** root cause that ratification was supposed to fix was independent of audit: T19 had Pass 1 in its own tx that committed *before* Pass 2 ran outside any tx — a crash between would leave `nodes` rows with stale `superseded_by = NULL`. That's a data-atomicity bug, not an audit bug.
  - **Shipped (Shape 0, minimal root fix)**: T19's Pass 1 + Pass 2 now share a single `unchecked_transaction()` so the data write is atomic. Audit unchanged (preserves crash-detector affordance). One commit, ~25 LOC + atomicity regression test in dedicated test binary `v04_phase_c_backfill_atomicity.rs` (using process-isolated fault-injection hook `test_hooks::FAULT_INJECT_BETWEEN_PASSES`).
  - **WONTFIX rest**: T20, T21, T22-T25 do not have the two-pass two-tx pattern (verified by grep — they use single-tx work loops). The audit-in-tx contract from §A is not pursued because (a) Scenario 1 is a feature not a bug, (b) Scenario 2 is a microsecond window that verify already catches, (c) Scenario 3 isn't audit-in-tx-solvable. The shared `audit_run_open/close` helper extraction is also WONTFIX — would be over-engineering: changes 8 drivers to abstract away a pattern that isn't broken.
  - Verification: T19 7/7 + atomicity 1/1 + lib 1902/1902 + all 75 integration test binaries green.

## Dependency

Blocked by: T22-T25 completion (so the audit-row durability fix can be done across ALL Phase C drivers in one pass, not piecemeal).

## Origin

T21 review round 1: `.gid/features/v04-unified-substrate/reviews/t21-r1.md` FINDINGs 2-7.
T21 commit: `78f8eb5` (engram).
