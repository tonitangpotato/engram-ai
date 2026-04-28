# ISS-051: `Memory::compile_knowledge` is a zero-counter stub — `knowledge_topics` table is never populated, so the Abstract plan always returns empty

- **Status**: open
- **Severity**: major (the entire Abstract / L5 retrieval lattice path is dead — every Abstract-classified query returns 0 candidates and downgrades; this masks any genuine Abstract-plan ranking bug under a false "0 hits = expected for now" reading)
- **Filed**: 2026-04-28
- **Discovered during**: ISS-049 Phase 4 acceptance run on LoCoMo conv-26. The retrieval orchestrator wired the real `GraphTopicSearcher` adapter; Abstract-classified queries returned 0/0 across the entire run. Investigation traced this back to an empty `knowledge_topics` table, which traced back to `Memory::compile_knowledge` being an explicit A.1 stub that has never been replaced with the real `knowledge_compile::compile` body.
- **Related**: ISS-049 (which exposed the gap by wiring the adapter end-to-end), v03-resolution design §5bis (Knowledge Compiler), `task:res-impl-knowledge-compile` (the design-time owner of the missing implementation), GOAL-2.10 (Abstract retrieval surface).

---

## Summary

`Memory::compile_knowledge` (memory.rs:6134) is a placeholder that returns a
zero-counter `CompileReport` and never invokes the real K1→K3 pipeline:

```rust
// memory.rs:6134-6155 — current state
pub fn compile_knowledge(
    &mut self,
    namespace: &str,
) -> Result<CompileReport, Box<dyn std::error::Error>> {
    log::debug!(
        "compile_knowledge({namespace}): A.1 stub — knowledge_compile module \
         not yet implemented; returning zero-counter report"
    );
    Ok(CompileReport {
        run_id: uuid::Uuid::new_v4(),
        candidates_considered: 0,
        clusters_formed: 0,
        topics_written: 0,
        topics_superseded: 0,
        llm_calls: 0,
        duration: std::time::Duration::ZERO,
    })
}
```

The TODO comment immediately above the body documents the gap:

> TODO(`task:res-impl-knowledge-compile` / §A.2): replace this body with a
> real `crate::knowledge_compile::compile(namespace)` invocation that drives
> K1→K3 and persists topics.

Consequences:

- `knowledge_topics` table is empty (verified: 0 rows in
  `/tmp/iss049-phase4-locomo-graph.db`).
- `GraphTopicSearcher::search_by_embedding` returns `Ok(vec![])` for every
  query — no error, just no data.
- `AbstractPlan::execute` succeeds, produces zero candidates, and the
  orchestrator surfaces `RetrievalOutcome::L5NotReady { missing_topic_domains: [] }`
  (or `Ok` with empty results if the early-return path is taken — both look
  identical to a caller measuring `hits @ K`).
- The Abstract retrieval lattice path is completely dead end-to-end. If the
  classifier routes a query to Abstract, that query's `hit@K` is structurally
  zero regardless of corpus content.

This is **not** a retrieval-side bug. The retrieval surface (ISS-049 Phase 3)
is wired correctly and would surface real topics if any existed. The bug is
that ingest never produces topics.

---

## Reproduction

```
# Fresh DB, ingest a corpus
engram --database /tmp/iss051.db --graph-database /tmp/iss051-graph.db \
  store "Caroline lives in Berlin" --ns test
engram --database /tmp/iss051.db --graph-database /tmp/iss051-graph.db \
  store "Caroline works at OpenAI as a researcher" --ns test
# … ingest more memories …

# Trigger compile (no-op stub)
engram --database /tmp/iss051.db --graph-database /tmp/iss051-graph.db \
  compile-knowledge --ns test
# Reports: candidates=0 clusters=0 topics_written=0 — A.1 stub log line emitted.

# Inspect graph DB
sqlite3 /tmp/iss051-graph.db "SELECT COUNT(*) FROM knowledge_topics;"
# → 0

# Issue an Abstract-classified query (e.g., "what does Caroline do for a living?")
# Returns []
```

---

## Root Cause

`crate::knowledge_compile::compile` exists as a module skeleton with the
trait abstractions (`Clusterer`, `Summarizer`, `Embedder`) and the K1→K3
stage scaffolding documented in `mod.rs`, but the real K1 (candidate
selection from `memories` since the watermark) → K2 (clustering) → K3
(synthesis + persist via `KnowledgeTopic` upsert with supersession) is
not implemented. `Memory::compile_knowledge` was deliberately landed as a
stub during ISS-046 / earlier resolution-pipeline scaffolding so the
public API (`pub fn compile_knowledge`) could stabilize independently of
the internal pipeline implementation.

That decoupling was correct at the time. The cost is that **every other
v0.3 component now assumes the table will be populated by some path**,
and the Abstract retrieval plan has no fallback for "no topics in this
namespace yet" beyond returning empty.

---

## Fix

Replace the stub body with a real call to `knowledge_compile::compile`,
which itself needs three pieces wired:

1. **K1 — candidate selection.** Read from `memories` since the
   per-namespace watermark (stored where? `compile_runs` table? open
   question — design §5bis.2 says "watermark", but doesn't specify
   storage). Filter by importance threshold (config-driven default).

2. **K2 — clustering.** Default = embedding-space Infomap, per design
   §5bis.3. The `Clusterer` trait already exists in `knowledge_compile`;
   wire the production impl.

3. **K3 — synthesis & persist.** Per cluster: aggregate participating
   entities → LLM summary call (Anthropic-backed `Summarizer` impl) →
   embedding → `Entity::Topic` UUID minted (or supersession of an
   existing topic — see design §5bis.4) → atomic `knowledge_topics`
   upsert via `GraphWrite::write_topic`.

Each of K1/K2/K3 is a non-trivial piece of work. **Recommend filing
sub-tasks** rather than treating this as a single unit:

- ISS-051a — K1 candidate selection + watermark storage
- ISS-051b — K2 clustering (Infomap impl behind `Clusterer` trait)
- ISS-051c — K3 synthesis (Summarizer wiring + topic upsert)

Until at least K1+K3 land (K2 can use a trivial "one cluster per N
memories" stub for v0.1 of this), Abstract is dead and **the example /
benchmarks should explicitly note that Abstract-classified queries are
expected to return 0 until ISS-051 lands**.

---

## Verification

After fix:
- `compile_knowledge` returns non-zero counters when run against a corpus
  with ≥ N memories above the importance threshold.
- `knowledge_topics` table has ≥ 1 row per cluster.
- A query that previously got Abstract-classified and returned `[]` now
  returns at least one ranked topic (or memories joined via topic).
- LoCoMo conv-26 hit@5 for Abstract-classified subset is non-zero.

---

## Out of scope

- The full v0.2 → v0.3 KnowledgeCompiler migration (deletion of
  `crate::compiler`). That's an end-of-migration cleanup, separately
  scoped.
- LLM cost budgeting for K3 synthesis. Design §5bis.7 mentions
  `knowledge_compile_*` metrics; wire the counters but don't gate on
  budget for v0.1.

---

## Notes for future investigation

- Check whether v0.2 `crate::compiler` is currently writing to a *different*
  topics table that could be used as a temporary read-source for Abstract.
  If so, an interim adapter could unblock Abstract while ISS-051a/b/c land.
  (Probably not — design explicitly says v0.2 writes its own `topics` rows
  separate from the v0.3 `knowledge_topics` rows.)
- LoCoMo eval has a known classifier-quality issue (mentioned during
  ISS-049 retro): the current heuristic classifier may be over-routing
  to Abstract for queries that should go to Factual or Episodic. Once
  ISS-051 lands and Abstract returns real candidates, re-measure the
  classifier accuracy on the same conv-26 set.
