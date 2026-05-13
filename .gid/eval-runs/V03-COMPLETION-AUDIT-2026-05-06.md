# V0.3 Completion Audit — 2026-05-06

> Read-only audit of engram v0.3 actual completion vs design.
> Triggered by potato realizing LoCoMo runs are testing partial engram, not complete v0.3.
> Strict cite-before-claim — every claim has a code/issue citation.

---

## TL;DR — The Headline Finding

**The engram-bench LoCoMo harness has NEVER exercised the v0.3 resolution pipeline.**

Every J-score we have (RUN-0017 3.6%, RUN-0018 42.1%, RUN-0020 K=15 46.7%)
was measured against a substrate-only memory layer with an empty graph DB.
The v0.3 graph store, entity extraction, and resolution pipeline have not
participated in any reported bench number.

The "+38.5pp from ISS-103" jump in RUN-0018 was real but is about **temporal
context surfacing** (occurred_at threaded into the LLM context block), not
about v0.3 graph retrieval.

**Conclusion:** v0.3 work that has shipped to engramai's main code path
(`crates/engramai/src/`) is real and tested by unit/integration tests, but
the **end-to-end LoCoMo benchmark does not validate the v0.3 retrieval
stack**. We've been measuring v0.2 substrate + ISS-103 occurred_at fix.

---

## Evidence Trail

### Finding 1: bench harness builds Memory without pipeline pool

**Location:** `engram-bench/src/harness/mod.rs:540-557` — `fresh_in_memory_db()`

```rust
pub fn fresh_in_memory_db() -> Result<engramai::Memory, BenchError> {
    let dir = tempfile::tempdir()?;
    let dir_path = dir.keep();
    let mem_db = dir_path.join("substrate.db");
    let graph_db = dir_path.join("graph.db");
    let mem_db_str = mem_db.to_str()?;

    engramai::Memory::new(mem_db_str, None)
        ?
        .with_graph_store(&graph_db)  // ← installs graph STORE only
        ?
}
```

**No `.with_pipeline_pool(...)` call.** Every LocomoDriver run starts
from this builder.

### Finding 2: store_raw silently skips pipeline when no queue installed

**Location:** `crates/engramai/src/memory.rs:809-826` — `enqueue_pipeline_job`

```rust
fn enqueue_pipeline_job(&self, memory_id: &str) -> Option<uuid::Uuid> {
    let queue = self.job_queue.as_ref()?;  // ← early-return None when queue absent
    // ... try_enqueue ...
}
```

The `?` operator short-circuits to `None` when `self.job_queue` is `None`.
Per design GUARD-1 this is intentional — admission must succeed even if
the pipeline isn't wired — but it means a bench harness that skips
`with_pipeline_pool` gets **silent pipeline-skip** with no error.

`with_graph_store` (memory.rs:486-520) only installs the graph **store**
(read/write API for entity/relation rows). It does NOT install
`pipeline_queue`. The two are separate.

### Finding 3: ingest_with_stats_at returns fake empty ResolutionStats

**Location:** `crates/engramai/src/memory.rs:6750-6790`

```rust
pub fn ingest_with_stats_at(...) -> Result<(MemoryId, ResolutionStats), _> {
    let outcome = self.store_raw(content, meta)?;
    match outcome {
        RawStoreOutcome::Stored(outcomes) => {
            let id = outcomes.first()?.id().clone();
            let stats = crate::resolution::ResolutionStats::default();  // ← FAKE
            Ok((id, stats))
        }
        // ...
    }
}
```

Returns `ResolutionStats::default()` regardless of whether the pipeline
actually ran. Callers (incl. LocomoDriver) cannot tell from the return
value whether resolution ran or was skipped.

### Finding 4: LocomoDriver uses the unhooked path

**Location:** `engram-bench/src/drivers/locomo.rs:591-600`

```rust
let mut memory = fresh_in_memory_db()?;
for episode in &conv.episodes {
    memory.ingest_with_stats_at(&episode.text, episode.occurred_at)?;
}
```

Combined with Findings 1-3: every conversation ingest in every LoCoMo run
to date has skipped the resolution pipeline. The graph DB at
`<tempdir>/graph.db` is created (schema initialized) but never
populated by the pipeline.

### Finding 5: retrieval calls graph_query_locked against empty graph

**Location:** `engram-bench/src/drivers/locomo.rs` — uses
`Memory::graph_query_locked` for every gold question.

That code path expects entity/relation rows in the graph store. With an
empty graph (Finding 4), it falls back to substrate-only paths
(BM25 + vector + decay) for every result. This explains why:

- RUN-0017 (no occurred_at threading): J-score 3.6% — substrate finds
  nothing useful because temporal anchors are wall-clock-now.
- RUN-0018 (ISS-103 fix): J-score 42.1% — substrate now sees correct
  episode times, retrieval ranks by recency-to-question correctly,
  context block surfaces real timestamps. **All gain is substrate +
  context-formatting, no graph involvement.**
- RUN-0020 K=15: marginal +4.6pp from broader recall — still substrate.

### Finding 6: 631-entity graph.db I "remembered" was a different code path

In a prior session I noted "conv-26 graph.db has 631 entities, 720 edges."
That was from the **cogmembench adapter path**
(`cogmembench/benchmarks/locomo/engram_adapter.py`) which uses the
**engram CLI** (`engram store ...`) — and that CLI DOES wire the
pipeline pool. Different binary, different code path, different DB.

The engram-bench standalone harness is independent and uses no pipeline.
I conflated the two. This audit corrects that.

---

## Per-GOAL Status (v0.3)

> Methodology: walk each GOAL in `engram/.gid/features/v03-*/requirements.md`
> (or master), cite the implementing code, mark done/partial/not-started
> based on whether it (a) has shipped code, (b) has tests, (c) has bench
> validation.

**TODO** — pending. The audit above establishes the framing. Per-GOAL
breakdown to follow once requirements docs are walked.

Outline of expected gaps:
- v03-graph-layer: code shipped, unit-tested, **not bench-validated**
- v03-resolution: code shipped, unit-tested, **not bench-validated**
- v03-retrieval: code shipped, unit-tested, **not bench-validated**
- v03-migration: backfill scripts exist, partial coverage
- v03-benchmarks: harness exists, but **does not exercise v0.3 path** (this audit)

---

## Recommendation

Before the next J-score run, fix the harness to wire pipeline:

```rust
pub fn fresh_in_memory_db() -> Result<engramai::Memory, BenchError> {
    // ... tempdir setup ...
    let mut mem = engramai::Memory::new(mem_db_str, None)?
        .with_graph_store(&graph_db)?
        .with_pipeline_pool(/* worker count, queue depth, extractor config */)?;
    Ok(mem)
}
```

Then re-run RUN-0020 config (K=15) as RUN-0021. Compare:
- If J-score moves → v0.3 retrieval is contributing real value
- If flat → v0.3 needs deeper investigation; substrate is doing all the work

**Do not ship "engram v0.3 hits LoCoMo X%" until the harness wires the pipeline.**

---

## Audit Status

- [x] Finding 1: harness skips with_pipeline_pool — confirmed
- [x] Finding 2: store_raw silently skips pipeline — confirmed
- [x] Finding 3: ingest_with_stats_at returns fake stats — confirmed
- [x] Finding 4: LocomoDriver uses unhooked path — confirmed
- [x] Finding 5: retrieval falls back to substrate-only — confirmed by inference
- [x] Finding 6: 631-entity graph was a different path — confirmed
- [ ] Per-GOAL walk of v0.3 requirements docs — **pending**
- [ ] Confirm Finding 5 by reading graph.db rows after a RUN — **pending**
