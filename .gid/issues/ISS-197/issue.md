---
title: 'Phase E ordering inversion: legacy `memories` READ paths must be cut over before write-deletion (T34a blocked)'
status: open
priority: P0
severity: blocker
labels: v04-unified-substrate, phase-e, phase-ordering
created: 2026-05-31
relates_to: [ISS-196]
---

# Summary

**Phase E (T34–T37) cannot proceed as designed.** The design frames Phase E
as a pure *write*-deletion pass over ~63 prod legacy-write statements, on the
premise that "the narrow file blast radius is what makes Phase E tractable as
a deletion pass rather than a multi-week refactor" (design §5.5.1). That premise
is **false**: the blast radius is the *read* side, not the write side.

Removing the legacy `memories` INSERT in `Storage::add` (T34a) orphans **62+
production `SELECT FROM memories` sites in `storage.rs` alone** (plus 3 in
`lifecycle.rs`, 1 in `graph/store.rs`). Those reads were **never cut over to
`nodes`** — T29.7 explicitly *deferred* the remaining `SELECT FROM memories`
read-switch to Phase F, on the reasoning (recorded in working memory note
`iss…T29.7`) that:

> "the remaining 'SELECT FROM memories' sites don't need a read-switch before
> Phase F because T12 dual-write keeps the row-sets in sync."

**That reasoning makes Phase F (read-cutover) a prerequisite of Phase E
(write-deletion), not a successor.** Phase E removes the very dual-write that
T29.7 relied on to keep `memories` populated for those readers.

# Evidence

T34a was attempted as a pure deletion (delete `INSERT INTO memories` from
`Storage::add`, keep `insert_memory_node_row` survivor). Result:

```
test result: FAILED. 1971 passed; 104 failed; 4 ignored
```

104 failures span `association`, `knowledge_compile`, `lifecycle`, `memory`,
`resolution`, etc. — all of them follow the pattern: write a record via
`Storage::add` (now → `nodes` only), then read it back via a method still
doing `SELECT FROM memories` (→ empty). Representative:
`resolution::memory_reader::tests::fetch_returns_stored_record` — the test
comment itself states "namespace is read from the `memories.namespace`
column".

Revert verified clean: `git checkout storage.rs` → `cargo test -p engramai
--lib` = **2075 passed / 0 failed**. **No data or code damage; nothing
committed.**

## Production `SELECT FROM memories` read-site inventory (pre-test-boundary)

- `storage.rs` (before `#[cfg(test)]` @L8741): **62 sites** — hot paths
  including `get_all`, `get_by_ids`, `get_by_type`, `search_fts` (one arm
  still on `memories m JOIN memories_fts`), `list_namespaces`, namespace
  subqueries in edge inserts (L1661–1685), supersede/delete rowid lookups.
- `lifecycle.rs`: **3 sites** (L143/171/315 — existence + metadata reads).
- `graph/store.rs`: **1 site** (L6815 `SELECT entity_ids, edge_ids`).
- `substrate/backfill.rs` (9), `substrate/verify.rs` (1),
  `substrate/triple_backfill.rs` (1): **legitimate** — these read `memories`
  *as the migration source*; they SHOULD keep reading `memories` until T39
  DROP, and are out of scope for this cutover.

# Root cause

A phase-dependency that the v04 design got backwards:

- design §5.5 (Phase E) = delete legacy **writes**
- design §5.6 (Phase F / T39) = drop legacy tables + (implicitly) cut over
  remaining reads

But T29.7 deferred the `memories` *read*-switch into "Phase F prep". So the
true dependency chain is:

```
read-cutover (memories → nodes for the 62+3+1 hot sites)
        ↓  MUST PRECEDE
write-deletion (Phase E T34a … remove INSERT INTO memories)
        ↓  MUST PRECEDE
DROP (Phase F T39)
```

The design's Phase E acceptance criterion #5 ("call the entry point under
cutover `unified_substrate=true` and assert legacy row count unchanged")
silently assumes reads already route to `nodes` under the flag — but
`Storage::new` stays legacy in the test arms (T32 note), and the 62 storage.rs
read methods are flag-independent: they hard-code `FROM memories`.

# Proposed fix (for potato's disposition)

Insert a **read-cutover sub-phase between T34-pre and T34a** (call it Phase E-0
or fold T29.7 back in as a Phase E prerequisite):

1. Enumerate the 66 prod `FROM memories` read sites (62 storage + 3 lifecycle
   + 1 graph store; backfill/verify/triple_backfill excluded).
2. Switch each to `FROM nodes WHERE node_kind='memory'` (with the existing
   column→attribute projection already used by `insert_memory_node_row` /
   the T29.x read-switches). The `nodes` row carries the same fields, so each
   site is a SELECT-source rewrite + column-name remap, not a logic change.
3. Per-site: keep behaviour byte-identical under the legacy `Storage::new`
   path too (since `nodes` is dual-written today, reads from `nodes` return
   the same rows). This means the read-cutover is independently shippable and
   testable BEFORE any write is deleted — the dual-write still backstops it.
4. Only after all 66 reads pass on `nodes` → proceed to T34a write-deletion
   as originally planned (now genuinely a pure deletion).

This is larger than "a deletion pass" but it is the **root fix**: it removes
the hidden phase-ordering bug rather than patching around it. Estimated 66
read-site rewrites, mechanical but must be verified individually.

# What was NOT done (stop-condition honoured)

Per overnight authorization ("任何不确定先停记 issue 不硬干"): I did **not**
force T34a through by mass-rewriting tests or bypassing the read paths. The
attempt was reverted, suite is green at 2075, and this issue captures the
finding. No commits beyond the pre-existing `e7b3509` (T34-pre, already
landed and green).

# Acceptance criteria

- [ ] AC-1: All 66 prod `FROM memories` hot-read sites (storage 62 +
      lifecycle 3 + graph store 1) switched to read from `nodes`, verified
      byte-identical under the dual-write (legacy) path with full lib suite
      green (2075+).
- [ ] AC-2: backfill/verify/triple_backfill `FROM memories` reads explicitly
      confirmed retained (migration sources, drop at T39).
- [ ] AC-3: T34a re-attempted as pure deletion → `cargo test -p engramai
      --lib` fully green with zero test rewrites needed.
- [ ] AC-4: design §5.5 amended to document the read-cutover prerequisite and
      correct the "deletion pass not a refactor" framing.
- [ ] AC-5: PHASE-E-PLAN.md updated with the E-0 read-cutover sub-phase
      inserted before T34a.

---

# E-0 EXECUTION OUTCOME (2026-05-31)

Phase E-0 read-cutover executed in cohorts under the live dual-write, each
verified `cargo test -p engramai --lib` = **2075 passed / 0 failed**.

## Commits

- Bucket A (`SELECT *` + decoder swap): `99bfaae` · `1cd72be` · `6e7bb2e` ·
  `149412f` (fetch helpers + insight-predicate fix + merge_enriched_into
  nodes-mirror — a Phase B dual-write gap discovered en route).
- Bucket D (FTS-join, was half-cut from T29.6): `cd12c66` — `search_fts` +
  `search_fts_ns` both arms switched from `SELECT m.* FROM memories m JOIN nodes`
  to pure `SELECT n.* FROM nodes n JOIN nodes_fts`.
- Bucket B (scalar reads): `c94411c` · `98ac819` (health/housekeeping incl. the
  `cleanup_*` orphan-prune DELETEs whose `NOT IN (SELECT id FROM memories…)`
  subqueries would have pruned EVERY dependent row post-write-deletion) ·
  `50f8157` · `d4d3634`.

## Predicate correction (important)

The read predicate is **`node_kind IN ('memory','insight')`**, NOT just
`'memory'` as AC-1 originally implied. The legacy `memories` table held
synthesis **insights** too (`store_raw` writes `node_kind='insight'`).
Excluding insight broke parity (store_insight + merge tests). id-keyed point
reads need no kind filter (id is unique PK).

## Inventory correction

The original "66 hot-read sites" over-counted. The real **in-scope hot-read**
set was ~20 methods. The remainder were mis-classified:
- FTS-companion `SELECT rowid FROM memories` (retire with their paired
  `memories_fts` DELETE) — out of scope.
- Legacy `else`-arms of already-cut `unified_substrate` reads — out of scope.
- Read-modify-write paired with a `memories` write (dedup content-evolution
  L6276, `append_merge_provenance` L7216) — these need a nodes-mirror like
  `merge_enriched_into` got, OR retire with their write — out of read-cutover
  scope.
- Enrichment-status (`get_unenriched_memory_ids`, depends on the memories-only
  `triple_extraction_attempts` column + paired UPDATE) — out of scope.
- v1→v2 migration source (`list_v1_candidates_page` L8508/8517) — out of scope.
- `lifecycle.rs` L143/171/315 are TEST assertions on the legacy table, not prod
  reads.
- `graph/store.rs` L6815 `entity_ids,edge_ids` — `nodes` has NO such columns
  (unified models these as real `edges` rows); advisory cache, out of scope.

## Two NEW blockers (documented, NOT forced — see PHASE-E-PLAN §8.5/§8.6)

- **§8.5:** hebbian-dedup namespace subqueries (`merge_bidirectional_hebbian`
  ~L1661-1685) run unconditionally during migration BEFORE `nodes` exists →
  212 `no such table: nodes` failures → reverted, stays on memories (advisory).
- **§8.6:** `get_deleted_at` type mismatch — `memories.deleted_at` TEXT/RFC3339
  vs `nodes.deleted_at` REAL/epoch (soft_delete dual-writes both formats);
  `Option<String>` read of REAL errors → left on memories. **T39 prerequisite:
  reconcile the deleted_at type/format.**

## AC status

- [x] AC-1: all in-scope prod hot-read sites switched to `nodes`, byte-identical
      under dual-write, suite green 2075/0 — with the scope/predicate corrections
      above (insight included; FTS-companion/RMW/migration-source/test reads are
      out of scope by design, not skipped).
- [x] AC-2: backfill/verify/triple_backfill reads confirmed retained.
- [ ] AC-3: T34a re-attempt — **BLOCKED on a THIRD prerequisite (see below).**
- [ ] AC-4: design §5.5 amendment — pending.
- [x] AC-5: PHASE-E-PLAN.md §8 updated with E-0 sub-phase + §8.5/§8.6 blockers +
      status log.

---

# T34a RE-ATTEMPT OUTCOME (2026-05-31) — THIRD BLOCKER FOUND

After E-0 read-cutover landed green (2075/0), T34a was re-attempted as the
narrowly-scoped pure deletion: gate the `INSERT INTO memories` **and** its
companion `INSERT INTO memories_fts` in `Storage::add` behind `if !unified`,
keep the `insert_memory_node_row` survivor, touch nothing else (RMW UPDATE
paths L6276/L7216 and `soft_delete` left writing `memories`, as scoped).

Result: **`cargo test -p engramai --lib` → 2052 passed / 23 failed.**

## Failure signature — uniform `FOREIGN KEY constraint failed`

All 23 failures are the **same** root cause, NOT read-cutover gaps:

```
lifecycle::tests::test_list_namespaces  panicked … lifecycle.rs:557
  Result::unwrap() on Err: "storage error: FOREIGN KEY constraint failed"
```

The failures cluster in `lifecycle::tests` (hebbian/sleep/forget/health),
`memory::confidence_tests::test_broadcast_*` (hebbian spreading), and
`knowledge_compile::candidates::tests` — every one of them adds a record via
`Memory::add` (→ `Storage::add`, now `nodes`-only under unified) then writes a
**retained child row** whose FK still `REFERENCES memories(id)`.

## Root cause — retained FK-child tables still point at `memories`

ISS-196 re-pointed **only** `access_log.memory_id` → `nodes(id)` (storage.rs
`migrate_access_log_fk_to_nodes`, L1185+). But three other **retained** child
tables — written during `add`/enrichment — still declare FKs into `memories`:

- **`hebbian_links`** (storage.rs L1087-1088): `source_id`, `target_id`
  `REFERENCES memories(id)`. Written by `record_coactivation*` during `add`'s
  co-activation step → **this is the FK firing** for the lifecycle/broadcast
  failures (those tests don't touch entities).
- **`memory_entities`** (L1139): `memory_id REFERENCES memories(id)`. Written
  during entity enrichment (`test_find_entity_overlap`, health/cluster tests).
- **`synthesis_provenance`** (L1168-1169): `insight_id`, `source_id`
  `REFERENCES memories(id)`. Written when synthesis insights land.

With `PRAGMA foreign_keys=ON` (storage.rs:447) and `memories` no longer
populated under unified mode, the legacy FK check fires on these child inserts
**before** the unified `edges`/`nodes` dual-write can matter. These tables
*do* dual-write to unified `edges`/`nodes` (T14/T16), but the legacy FK is
still load-bearing because the legacy child table still exists with its old FK.

## The true T34a prerequisite (broader than read-cutover)

T34a's premise — "delete the `memories` write, `nodes` mirror already exists" —
is correct for **reads** (E-0 fixed those) but incomplete for **referential
integrity**. Deleting the `memories` write orphans every retained child table
whose FK still targets `memories`. So T34a additionally requires:

> **Re-point ALL retained child-table FKs from `memories(id)` to `nodes(id)`**
> (the exact ISS-196 `access_log` table-rebuild pattern, applied to
> `hebbian_links`, `memory_entities`, `synthesis_provenance`), so the child
> inserts during `add`/enrichment validate against the populated `nodes` row.

This is a bounded, mechanical migration (3 idempotent table rebuilds mirroring
`migrate_access_log_fk_to_nodes`), but it is **NOT** what T34a was scoped to,
and forcing it inside the T34a deletion commit would conflate two concerns.
Recommend: a new sub-task **T34a-pre (FK re-point)** preceding T34a, OR widen
ISS-196 to cover all retained child tables (ISS-196 currently only did
`access_log`).

## Stop-condition honoured

Per "任何不确定先停记 issue 不硬干": did **not** mass-rewrite the 23 tests or
disable FK enforcement to force T34a green. Uncommitted T34a edit reverted
(`git checkout storage.rs`), tree clean at `225cd3a`, suite green 2075/0. No
commits, no data touched. This finding captured for potato's disposition.

## Open question for potato

Should the FK re-point be **(a)** a widening of ISS-196 (it already owns the
`access_log` re-point and the same rationale applies verbatim), or **(b)** a
new T34a-pre sub-task in PHASE-E-PLAN §8? Either way the work is identical: 3
more `migrate_*_fk_to_nodes` idempotent rebuilds before T34a's deletion.
