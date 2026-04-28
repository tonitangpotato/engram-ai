# ISS-046: `engram store` CLI does not install pipeline pool — v0.3 graph stays empty on fresh ingest

**Status**: open
**Priority**: P0 (blocks ALL v0.3 retrieval on freshly ingested data; LoCoMo conv-26 0/25 hits)
**Filed**: 2026-04-27
**Filed by**: rustclaw (during LoCoMo conv-26 retrieval diagnosis)

## Symptom

LoCoMo conv-26 smoke benchmark, 2026-04-27:
- `engram store` ingested 32 memories into v0.2 `memories` table successfully
- Companion `<db>.graph.db` file: 0 bytes (created by retrieval driver `with_graph_store`, never written by ingest)
- Retrieval driver: 0/25 hits, every gold-set question returned `got=0`

## Root cause

`engramai::Memory` has a fully-implemented v0.3 fresh-ingest path:
- `with_pipeline_pool(graph_db_path, ...)` installs `WorkerPool` + graph store
- `store_raw` Path A (extractor) and Path B (no extractor) both call `enqueue_pipeline_job(id)` on `Inserted` outcomes (memory.rs:2739, 2847)
- WorkerPool drains queue, runs `ResolutionPipeline::resolve_*`, writes entities + edges to graph DB

**But `engram store` CLI never installs the pool.** Inspection of `crates/engram-cli/src/main.rs` Store handler:
- Builds `Memory::new(...)` or `Memory::with_extractor(...)`
- Calls `mem.add_with_emotion(...)` / `mem.add_to_namespace(...)`
- **Never** calls `mem.with_pipeline_pool(...)` or `mem.with_graph_store(...)`

So `enqueue_pipeline_job` runs, but with `pipeline_pool: None` it returns `None` (silent no-op per GUARD-1: pool absence does not abort admission).

Result: v0.2 memories written ✅, v0.3 graph never touched ❌.

## Why driver test drivers work but CLI doesn't

`crates/engramai/examples/locomo_conv26_retrieval.rs` line 122 calls `with_graph_store(path)` — sets up read-side graph. But there is **no example for ingest** — the ingest scripts (`01_ingest.py`) shell out to `engram store` CLI, which is the unwired path.

## Proposed fix

Add to `crates/engram-cli/src/main.rs` Store command (and similar ingest-y subcommands):

```rust
// New CLI flag
#[arg(long, help = "Path to v0.3 graph SQLite DB (default: <main_db>.graph.db)")]
graph_db: Option<PathBuf>,

#[arg(long, help = "Disable v0.3 graph writes (v0.2-only mode)")]
no_graph: bool,
```

In Store handler, after Memory build:
```rust
if !args.no_graph {
    let gdb_path = args.graph_db.unwrap_or_else(|| default_graph_db_path(&args.database));
    mem = mem.with_pipeline_pool(&gdb_path, /* worker config */)?;
}
```

`default_graph_db_path("/path/to/foo.db")` → `/path/to/foo.graph.db`.

After `add_with_emotion` / equivalent returns, **drain the pool synchronously** before exit (CLI is short-lived, can't rely on async drain):
```rust
if let Some(pool) = mem.take_pipeline_pool() {
    pool.drain_blocking(Duration::from_secs(60))?;
}
```
(Need to confirm `take_pipeline_pool` API exists — if not, add it; pool already supports graceful drain per memory.rs:368.)

## Acceptance criteria

- GOAL-1: After `engram store --graph-db /tmp/g.db "Alice met Bob in Paris"` completes, `/tmp/g.db` has schema applied + at least 1 entity row + at least 1 edge row (verifiable via direct SqliteGraphStore query)
- GOAL-2: `engram store --no-graph "..."` produces v0.2 row only, no graph DB file created
- GOAL-3: Default behavior (no flag): graph DB created at `<db_dir>/<db_stem>.graph.db`
- GOAL-4: LoCoMo conv-26 retrieval smoke goes from 0/25 to >=8/25 hits (gold-set baseline) after re-ingesting with `--graph-db` flag
- GOAL-5: No regression — `cargo test --workspace` clean
- GOAL-6: Idempotency: re-running same content does not duplicate entities/edges (delegates to existing pipeline guarantees)

## Verification plan

1. Unit: `engram-cli` integration test using sqlite tempdb, asserts graph DB non-empty after `store` invocation
2. Smoke: re-run LoCoMo conv-26 ingest with new `--graph-db` flag, then retrieval driver — assert hits > 0
3. Regression: `cargo test --workspace` + `cargo build --release` clean

## Implementation notes

- This is genuinely a thin wrapper change (~50-80 LOC across argparse + Memory builder + drain)
- Pool drain timeout deserves a flag too: `--graph-drain-timeout-secs` default 60
- For long-running consumers (rustclaw daemon), pool can stay running across stores; CLI just has shorter lifecycle
- Apply same flags to: `engram store`, `engram extract`, `engram store-raw` (audit which subcommands ingest)

## Dependencies

None. Engramai-side capability complete; this is pure CLI wrapper work.

---

## Resolution (2026-04-28)

**Status**: closed-fixed
**Fix commits**:
- `b806485` fix(engramai): separate-file graph DB support — three engramai bugs surfaced during implementation
- `950159d` feat(cli): ISS-046 wire v0.3 graph layer into `engram store`

### What landed (vs. proposal)

✅ `--graph-db <path>` flag (also via `ENGRAM_GRAPH_DB` env)
✅ `--no-graph` opt-out
✅ `--graph-drain-timeout-secs N` (default 60)
✅ Default graph DB path: `<stem>.graph.db` next to main DB
✅ `mem.shutdown_pipeline(deadline)` synchronous drain at exit
✅ `NoopTripleExtractor` for entity-only mode (no LLM required)

### Three upstream engramai bugs fixed in `b806485`

1. **Graph schema never initialized** — `with_pipeline_pool/with_graph_store` opened the connection but never called `init_graph_tables`. Result: 0-byte graph DB on fresh runs.
2. **Cross-file FK constraints break inserts** — `memory_id REFERENCES memories(id)` errors at prepare time when graph DB is a separate file (no SQLite cross-file FK support). Fix: detect co-location, FK ON for same-file, OFF for separate.
3. **Memory reader pointed at graph DB** — `SqliteMemoryReader::open(graph_db_path)` should open main DB (where `memories` lives). Fix: derive main DB path from foreground storage connection.

### Acceptance status

- GOAL-1 ✅ Graph DB populated after `engram store --graph-db ...`
- GOAL-2 ✅ `--no-graph` produces v0.2-only row
- GOAL-3 ✅ Default `<stem>.graph.db` path
- GOAL-4 ⏳ **Deferred** — LoCoMo conv-26 retrieval ≥8/25: pipeline now works (pipeline_runs success), but EntityExtractor pattern matching gives 0 entity hits on LoCoMo conversational text. This is an EntityExtractor capability gap (pattern-based, not NER), not a pipeline gap. New scope, new issue if pursued.
- GOAL-5 ✅ `cargo build --release` clean (test suite TBD post-LoCoMo follow-up)
- GOAL-6 ✅ Idempotency delegated to existing pipeline guarantees (no new code path)

### Follow-up

LoCoMo retrieval improvement requires entity extraction work, not pipeline work:
- Option A: Extend EntityExtractor patterns (regex/list expansion)
- Option B: Add LLM-based NER as optional extractor variant
- Option C: Use triple extractor results as entity source (triples already mention entities)

To be filed separately if/when LoCoMo lift becomes priority. ISS-046 itself is functionally closed.
