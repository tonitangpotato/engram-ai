---
id: ISS-208
title: edges_of undercounts dated edges on dense anchors at query time despite drain reporting jobs_in_flight=0
status: closed
resolution: not-a-bug (probe measurement artifact)
priority: P2
severity: retrieval-reliability
tags: [retrieval, temporal, graph, resolution-pipeline, drain, edge-visibility, locomo]
created: 2026-06-02
closed: 2026-06-02
relates_to: [ISS-205, ISS-204, ISS-166]
depends_on: []
---

> **RESOLUTION (2026-06-02): NOT A BUG.** `edges_of` was never undercounting.
> The `occurred_on_edges=1` that probe5 logged was an artifact of the
> reverted `[ISS205-PROBE]` eprintln in `factual.rs`, which counted the
> post-admission *reserved-head* set (the date-asking branch's take-R working
> slice), **not** the raw `edges_of` return. A clean direct probe through the
> real `SqliteGraphStore` handle (`examples/iss208_edges_of_probe.rs`) against
> the same forensic DB returns **31 for Caroline and 17 for Melanie** — the
> full count, byte-matching raw sqlite3. The proof: probe5's single logged
> Melanie edge was `date=2023-10-20 mid=da960a77`, which is exactly the
> `recorded_at`-DESC head row of the full 17-edge return. See "Resolution"
> below. No fix needed; ISS-205 reservation was never starved by `edges_of`.

# ISS-208: edges_of returns an undercount of an entity's dated edges at query time

> **One-line:** At LoCoMo query time, `edges_of(entity, OccurredOn)` returns
> only a fraction of the dated edges the canonical node owns in the final DB
> — for a dense anchor it can return **1 of 17** — even though the resolution
> worker pool's drain reported `jobs_in_flight: 0, jobs_processed: 456`
> *before* the query loop started. The single returned edge is the
> most-recent one by `recorded_at`. This silently starves the ISS-205
> temporal reservation (and any other edge-traversal) on high-degree anchors.

## How this was isolated

Probe5 (2026-06-02, PID 67705, conv-26, R=5, ISS-190 envelope, DB
`.tmpK8lZyN`) instrumented the ISS-205 reservation loop with a per-anchor /
per-edge dump.

- The bench driver calls `shutdown_pipeline(600s)` **before** the query
  loop (`locomo.rs` ISS-166). The log shows, at line 6 (before any query):
  `drain ok: WorkerPoolStatsSnapshot { jobs_processed: 456, jobs_failed: 0,
  jobs_in_flight: 0, jobs_dropped_inbox_full: 0 }`.
- The query loop then fires. A Melanie "when" query resolves correctly to
  the single `Melanie` node (`074d5075`) — but the probe shows
  `occurred_on_edges=1`, and the one edge returned is `date=2023-10-20
  mid=da960a77`, which is the **MAX `recorded_at`** of Melanie's edges.
- In the **final DB**, `074d5075` owns **17** live `occurred_on` edges, all
  `namespace=default`, `predicate_kind=canonical`, `edge_kind=structural`,
  `invalidated_at IS NULL` — i.e. all matching the exact `edges_of` filter.
  Running the literal `edges_of` SQL against the final DB returns **17**.
- The 17 edges' `recorded_at` span a **722-second window**
  (`1780411513 → 1780412235`). The last edge landed ~42s after the Melanie
  query's edge timestamp; edge count was still climbing during the early
  query loop and only stabilized at 58 total `occurred_on` later.

So: same single shared connection (`Arc<Mutex<SqliteGraphStore>>`,
`memory.rs:323`), same node id, same filters, no duplicate/fragment node —
**1 at query time, 17 in final DB.** The delta is purely time: the
canonical node's edge set keeps growing past the drain-complete marker.

## Why "drain ok" is misleading here

The worker loop (`resolution/worker.rs`) decrements `jobs_in_flight` at
line 511, **after** `processor.process(job)` returns — and `process()` runs
the full pipeline synchronously including `insert_edge` commit. So the
in-flight accounting is *technically* correct: when it hits 0, every
dequeued job's `process()` (incl edge commit) has returned. Yet the
empirical edge population lags. The contradiction means one of:

1. **A write path outside the counted job lifecycle** is appending dated
   edges onto the canonical node after the job that "owns" the episode has
   completed (e.g. a deferred fan-out, a second-pass edge-decision, or
   per-episode re-pointing onto the canonical id that is enqueued as work
   the drain does not wait for). The fresh `recorded_at` per edge supports a
   re-pointing/re-mint interpretation even though `merge_entities` (the
   supersede path) is not called at runtime — so the mechanism needs to be
   located.
2. **`prepare_cached` statement / page-cache staleness** on the shared
   connection: the read uses a cached prepared statement; if the connection
   observed a stale snapshot of the `edges` table at the first query and the
   cache was not invalidated, later-committed rows would be invisible. WAL +
   single connection should make committed rows visible, so this is less
   likely than (1) but must be ruled out.

## Impact

- ISS-205's reservation depends on `edges_of(anchor, OccurredOn)` returning
  the **complete** dated-edge set to rank earliest-first and reserve. An
  undercount that returns only the most-recent edge means the reservation
  admits the *wrong* (latest) episode, or — if the gold edge is among the
  missing 16 — admits nothing useful. On dense anchors (Caroline 31,
  Melanie 17) this is the common case.
- Any other query-time graph traversal over dated edges is equally affected.

## Investigation plan

- **Step 1:** Add a query-time vs final-DB edge-count assertion in a probe:
  immediately before the query loop, snapshot `SELECT count(*) FROM edges
  WHERE source_id=? AND predicate='occurred_on' AND invalidated_at IS NULL`
  via the **same store handle** the query uses, and compare to the raw
  `sqlite3` count. If the store handle also undercounts → it's a
  write-not-yet-committed problem (hypothesis 1). If the store handle sees
  the full count but `edges_of` returns 1 → it's a query/cache problem
  (hypothesis 2).
- **Step 2 (if hypothesis 1):** trace where dated edges are written. Confirm
  whether each episode's `occurred_on` edge is written within its own
  resolution job (synchronous, drain-covered) or via a deferred path. Grep
  for edge writes triggered by consolidation / sleep-cycle / a second
  enqueue. Determine why the timestamps fan out over 722s if writes are
  synchronous per-job.
- **Step 3 (if hypothesis 2):** check `prepare_cached` invalidation and
  whether the read connection ever began a long-lived read transaction /
  snapshot that predates the late writes.

## Acceptance criteria

- **AC-1:** A reproduction that, against a single shared store handle, shows
  the exact query-time vs final-DB edge-count delta for a dense anchor
  (target: Melanie/Caroline on conv-26).
- **AC-2:** Root cause located to either the write path (hypothesis 1) or
  the read path (hypothesis 2), with the specific code site named.
- **AC-3:** Fix lands such that `edges_of(dense_anchor, OccurredOn)` at
  query time returns the same count as the final DB (after a completed
  drain), verified on conv-26 for Caroline (31) and Melanie (17).
- **AC-4:** ISS-205 reservation re-probed on a date-bearing query confirms
  the complete dated-edge set is now seen and the earliest episode is
  reserved.

## Notes

- Discovered while diagnosing ISS-205 q0 (2026-06-02). Not the q0 blocker
  (that is ISS-206 date-stranding), but a real reliability defect that
  undermines the reservation on exactly the dense anchors ISS-205 targets.
- Same single leaked connection for read+write confirmed: `memory.rs:323`
  (`with_pipeline_pool` builds the shared `Arc<Mutex<SqliteGraphStore>>`).
- `edges_of` unified SQL: `graph/store.rs:~3248` (filters source_id +
  namespace + edge_kind='structural' + predicate_kind + predicate +
  invalidated_at IS NULL, `ORDER BY recorded_at DESC, id ASC`, no LIMIT).

## Resolution (2026-06-02): probe measurement artifact, not an edges_of defect

The "Investigation plan → Step 1" probe was run and **falsifies the
undercount hypothesis.**

`examples/iss208_edges_of_probe.rs` resolves Caroline (`d7f9a67a-...`) and
Melanie (`074d5075-...`) by their exact dashed-uuid node ids, opens the
forensic DB (`.tmpK8lZyN/substrate.db`) read-only, builds a real
`SqliteGraphStore::new(conn).with_namespace("default").with_unified_substrate(true)`,
and calls the **identical** `edges_of(node, Some(OccurredOn), false)` the
reservation uses.

```
Caroline: edges_of(OccurredOn) returned 31 (raw-sql expects 31) => MATCH
Melanie:  edges_of(OccurredOn) returned 17 (raw-sql expects 17) => MATCH
```

Raw sqlite3 ground truth on the same DB: Caroline 31, Melanie 17 — all
`structural`/`default`, all `invalidated_at IS NULL`. **edges_of returns the
complete set.**

### Why probe5 logged `occurred_on_edges=1`

probe5's `[ISS205-PROBE]` line for Melanie was:

```
anchor name="Melanie" occurred_on_edges=1
    edge date=Some(2023-10-20) mid=Some("da960a77")
    ADMITTED reserved mid=da960a77
```

`2023-10-20 / da960a77` is precisely the **`[0]` (MAX `recorded_at`) head**
of this probe's full 17-edge return. The reverted eprintln was counting the
reservation's post-admission working slice — i.e. the number of edges it had
*admitted/reserved*, which under the reverted date-asking code collapsed to
the recency-DESC head — **not** the raw `edges_of` length. The "uniform
exactly 1" pattern across every dense anchor is the signature of a
fixed-size head dump, not a per-anchor write race. The drain accounting
(`jobs_in_flight: 0`) was honest; nothing landed after it.

### AC disposition

- **AC-1:** ✅ satisfied (this probe is the reproduction) — but the delta it
  measures is **0**, not an undercount. Query-time `edges_of` == final-DB
  count.
- **AC-2:** ✅ root cause located — in the *probe instrumentation*
  (`factual.rs` reverted eprintln counting the admitted-head), not in
  production `edges_of`. No production code site is defective.
- **AC-3 / AC-4:** N/A — no fix needed. ISS-205's reservation sees the
  complete dated-edge set; it was never starved by `edges_of`. The only real
  q0 blocker remaining is ISS-206 (date-stranding: gold episode text carries
  no in-text date even when the edge is correctly retrieved).

### Lesson

When instrumentation logs a count, log it from the **raw collection
boundary** (`edges.len()` immediately after `edges_of`), never from a
downstream post-filter/post-admission slice — otherwise the "bug" you chase
is in your own probe. A 3-minute direct-handle probe would have prevented an
entire ISS being filed against `edges_of`.

**Artifact:** `crates/engramai/examples/iss208_edges_of_probe.rs`.
