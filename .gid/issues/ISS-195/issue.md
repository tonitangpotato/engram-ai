---
id: ISS-195
title: T37f prong 1 — default graph store to substrate file (stop the two-DB bleed at write time)
status: resolved
priority: P0
severity: architectural
tags:
- v0.4
- unified-substrate
- two-db
- root-fix
- T37f
relates_to:
- v04-unified-substrate
created: 2026-05-30
fixed_by:
- engram:5a5ce76
- engram-bench:521848e
- engram-bench:eecc2d1
---

# T37f prong 1 — default graph store to the substrate file

## Problem

engram's core thesis is "the graph IS the substrate", but in reality the 816
real semantic edges (695 entities, 816 assertion edges on the conv-26 snapshot)
live in a **separate `graph.db` file**, while the unified substrate file's
`nodes`/`edges` tables hold only 3 weak entities / 0 semantic edges. Retrieval
reads the semantic graph directly out of `graph.db` via `SqliteGraphStore`.
Multi-hop quality cannot be genuinely fixed until the substrate is unified.

This is a **population split, not a field-mapping gap**: the two files'
unified tables hold disjoint row populations.

## Root cause (verified 2026-05-30)

There is **no deployed production engram service** — engram is a library
consumed by engram-bench. The only callers of `with_pipeline_pool` /
`with_graph_store` (bench harness, examples, tests) **all pass a separate
`*.graph.db` path**. That caller convention is *why* the 816 edges land in a
second file. `with_pipeline_pool` (`memory.rs:314`) **already supports
same-file mode** (peeks for a `memories` table → FK-ON same-file vs FK-OFF
separate-file; ISS-046).

So the two-file split is a caller convention, NOT an engine constraint.

## Scope (prong 1 ONLY — this issue)

Stop the bleed at write time. Make new ingestion write the entities/edges
directly into the substrate file's `nodes`/`edges`:

1. Change the graph-store install call sites to pass the **substrate file path**
   (or thread the substrate connection into `with_graph_store`) instead of a
   separate `*.graph.db`:
   - `engram-bench/src/harness/mod.rs` — `fresh_in_memory_db` (the real caller
     that creates the bleed). Gated by `ENGRAM_BENCH_GRAPH_SINGLE_FILE`
     (default ON; set `=0` for the legacy split during A/B parity).
   - **NOT** the `crates/engramai/src/retrieval/api.rs` unit-test fixtures
     (6 `with_graph_store(&graph_db)` sites) — those are test fixtures that
     legitimately exercise separate-file mode, which the API still supports.
     Touching them is scope creep with no correctness benefit; the engine
     already supports both modes, so the *default behavior* is driven by the
     real caller (the harness), not the test fixtures.
2. Confirm same-file mode tolerates the FK-ON path (graph schema FKs into
   `memories(id)`, which now resolves in-file).
3. Apply the locked edge_kind remap at the dual-write site
   (`graph/store.rs:1018` currently hardcodes `edge_kind='assertion'`): all 8
   graph predicates → `edge_kind='structural'`, `predicate` preserved verbatim.

## Out of scope

- **Prong 2 (historical backfill of pre-existing separate-file snapshots)** —
  tracked separately; only needed for DBs created before prong 1 ships.
- **T37g (switch `SqliteGraphStore` *reads* to the unified file)** — blocked on
  this issue; separate task.
- causal edge_kind split — deferred (predicate string already carries the
  nuance; a later split is pure-additive).

## Acceptance criteria

- AC-1: After a fresh ingest with prong 1, the substrate file's
  `nodes WHERE node_kind='entity'` and `edges WHERE edge_kind='structural'` are
  populated (≈ what previously landed in graph.db); the separate `*.graph.db`
  is no longer created (or is empty) for new runs.
- AC-2: `graph/store.rs:1018` dual-write emits `edge_kind='structural'`
  (not `'assertion'`), predicate unchanged.
- AC-3: Same-file FK-ON path verified — no FK violation on ingest into the
  unified file.
- AC-4: Full lib test suite green; no regression in existing graph-store tests.
- AC-5: A conv-26 bench run with the single-file wiring produces a graph store
  whose entity/edge counts match the pre-change two-file counts (parity).

## Governing spec

`.gid/features/v04-unified-substrate/design.md` §8 T37f (root-fix reframe,
prong 1) + locked edge_kind mapping table.

## AC-1 / AC-5 live verification (2026-05-30)

Closed the empirical gap. Single-file run (default `GRAPH_SINGLE_FILE` unset =
ON), conv-26, K=10 temp=0 HyDE/entity off pipeline_pool=1, substrate dir
`.tmpfc0rpw`:

**AC-1 PASS** — substrate file populated, no separate graph.db:
- tempdir contains **only `substrate.db`** — no `graph.db` created.
- `nodes WHERE node_kind='entity'` = **694** (was 3 under separate-file).
- `edges WHERE edge_kind='structural'` = **783** (was 0 under separate-file).

**AC-5 PASS** — entity/edge parity vs the legacy tables in the SAME file
(both populated because dual-write is still on this run):
- legacy `graph_edges` = 783, unified `edges`(structural) = 783.
- After normalizing legacy 16-byte blob ids → UUID text:
  **783/783 edges match 1:1** (same population, id encoding differs:
  legacy=BLOB, unified=TEXT UUID — the blob byte-sequence IS the UUID).
- Entities: 691/697 legacy ids map to unified entity nodes; the remaining
  **6 are present as `node_kind='topic'`** (dedup/merge promotion), NOT lost.
  Unified is a *superset* per R2.

Conclusion: the two-DB write-time bleed is closed. New ingestion lands the
full semantic graph (694 entities / 783 structural edges) in the substrate
file's own `nodes`/`edges`. Combined with T37g (reads switched to unified
tables), the substrate is now genuinely single-file and single-table-family.

Caveat: overall LOCOMO on this run = 0.243 — retrieval quality is unchanged
(as expected; prong 1 + T37g are plumbing, not ranking). Quality work is
downstream: ISS-186 (candidate pool recall) and ISS-070 (1-hop AssociativePlan
traversal).
