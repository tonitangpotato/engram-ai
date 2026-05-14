---
title: ISS-117 migration silently deletes cross-NS hebbian rows when id-order ≠ (ns,id)-order
status: open
severity: high
priority: P1
filed: 2026-05-13
filed_in_session: T29.4-part-5
relates_to: [ISS-117]
blocks: [T29.4-part-5, T29.4-part-6]
---

## Summary

`Storage::migrate_hebbian_canonical_rows` (storage.rs:1304) — the
ISS-117 idempotent migration that collapses pre-ISS-117 double-direction
rows into canonical `(min(id), max(id))` shape — **silently deletes
cross-namespace rows** whose `(source_id, target_id)` was canonicalised
by the writer using `(namespace, id)` tuple order rather than raw `id`
string order.

Step 2 of the migration:

```sql
DELETE FROM hebbian_links WHERE source_id > target_id
```

assumes canonical = `min(id_str) → source, max(id_str) → target`. But
`record_cross_namespace_coactivation` (storage.rs:4132) canonicalises by:

```rust
let (id1, id2, ns1, ns2) = if (ns1, id1) < (ns2, id2) {
    (id1, id2, ns1, ns2)
} else {
    (id2, id1, ns2, ns1)
};
```

When `ns_a < ns_b` but `id_in_ns_a > id_in_ns_b` (e.g. `hub` in `ns_hub`
co-activated with `a` in `ns_other` — `ns_hub < ns_other` so `hub` is
source, but `hub > a` lexicographically), the row passes the writer as
`("hub", "a")` then the migration deletes it as "non-canonical".

## Reproducer

```rust
let mut s = Storage::new(&db).unwrap();
seed(&mut s, "hub", "ns_hub");
seed(&mut s, "a", "ns_other");
for _ in 0..3 {
    s.record_cross_namespace_coactivation("hub", "ns_hub", "a", "ns_other", 3).unwrap();
}
// Row exists: ("hub", "a", strength=1.0, ns="ns_hub:ns_other")

drop(s);
let _s2 = Storage::new(&db).unwrap();  // re-opens → migrate_hebbian_canonical_rows runs

// hebbian_links is now EMPTY — row was deleted because "hub" > "a".
```

Confirmed 2026-05-13 in probe test during T29.4 part-5 development.

## Impact

- Every Storage reopen after ISS-117 (4163f36, 2026-05-13)
  silently deletes any cross-NS hebbian rows where the
  alphabetically-higher id lives in the alphabetically-lower
  namespace.
- Production engram-memory.db: unknown — depends on actual id
  distribution per ns. Need to audit before promoting unified
  substrate.
- Same-NS rows are unaffected because `record_coactivation_ns`
  canonicalises by raw id (storage.rs:~3870 same min/max id pattern
  as ISS-117 expects).

## Why ISS-117 tests didn't catch it

`iss117_record_cross_namespace_coactivation_forms_single_canonical_row`
(tests/iss117_canonical_hebbian.rs:230) uses ids `x_in_ns1` and
`y_in_ns2` — `x < y` lex, so the writer canonical (ns1<ns2 → x first)
matches the migration canonical (x<y → x first). The test passes
by coincidence of test-data choice, not by correctness.

## Options

**A. Fix migration (recommended root fix)**

Make migration aware of namespace ordering. Two sub-options:

  A1. Migration uses the same `(ns, id)` tuple ordering as the
      writer. Need to JOIN memories for namespace, or add a
      derived check.
  A2. Stop using id-order as the canonical key for cross-NS rows.
      Treat them as their own canonical-form set. The
      `WHERE source_id > target_id` check only applies to
      same-namespace rows.

**B. Stop deleting in migration; collapse via UPSERT**

Migration merges metrics into one side but doesn't DELETE the
non-canonical row. Leaves dupes; readers must dedupe. This breaks
ISS-117's "one row per pair" promise.

**C. Hoist canonical-key fix into the writer**

Change `record_cross_namespace_coactivation` to canonicalise by
raw id too, then store namespace pair as
`"ns_lo:ns_hi"`/`"ns_hi:ns_lo"` consistently. Existing rows would
still be wrong (already migrated/deleted), so a one-time
backfill is needed.

## Acceptance criteria

1. New regression test (in `iss117_canonical_hebbian.rs` or
   sibling): pre-ISS-117 cross-NS double-direction rows, after
   migrate, collapsed into ONE row per pair with NO data loss
   regardless of id-vs-ns ordering.
2. New regression test: post-ISS-117 cross-NS row survives a
   Storage reopen (currently fails).
3. `get_cross_namespace_neighbors`, `discover_cross_links`,
   `get_all_cross_links` all return correct rows after reopen.
4. Decision recorded in `.gid/features/v04-unified-substrate/design.md`
   §5 or new sub-section.

## Blocker for T29.4

T29.4 part-5 (`get_cross_namespace_neighbors`) and part-6
(`get_all_cross_links`) read-switch tests cannot reliably assert
contract until the underlying data is preserved across reopens.
Part-4 (`discover_cross_links`) tests pass because they don't
reopen Storage between writer and reader — the bug only manifests
on the second `Storage::new`. Re-running the part-4 suite with
explicit reopens between writer and reader **will fail**.

Recommended sequencing:

1. Fix ISS-118 (this issue, root fix).
2. Add reopen-between-write-and-read assertion to T29.4 part-4
   tests; it should still pass.
3. Resume T29.4 part-5 and part-6.

## Files

- crates/engramai/src/storage.rs:1304-1352
  (`migrate_hebbian_canonical_rows`)
- crates/engramai/src/storage.rs:4115-4205
  (`record_cross_namespace_coactivation`)
- crates/engramai/tests/iss117_canonical_hebbian.rs:230
  (test that fails to exercise the bug)
