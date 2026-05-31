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
