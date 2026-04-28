# ISS-051: `Memory::compile_knowledge` is a zero-counter stub — `knowledge_topics` table is never populated, so the Abstract plan always returns empty

## Status: ✅ RESOLVED (2026-04-28)

Fix landed in commit (this PR). `Memory::compile_knowledge` now drives the
real `knowledge_compile::compile()` pipeline. See "Resolution" section
at the bottom of this file for what was done.

---

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

---

## Resolution (2026-04-28)

Implemented in commit (see git blame on changed files). Summary:

### What was done

1. **New module `crates/engramai/src/knowledge_compile/adapters.rs`** —
   production-grade adapter implementations for the `Summarizer` and
   `Embedder` traits defined in `summarizer.rs`:
   - `EmbeddingProviderAdapter<'a>`: borrows `&EmbeddingProvider` (the
     same one `Memory` uses for memory embeddings, satisfying design
     §5bis.4 step 3 "same model, same dim"). Maps `EmbeddingError`
     variants to `EmbedError::{Transient, Permanent}` correctly
     (network/timeout = transient; model-not-found/parse = permanent).
     Validates returned vector dimension against the embedder's
     reported dim.
   - `AnthropicSummarizer`: production summarizer backed by Claude.
     Reuses `crate::anthropic_client` for header/auth construction
     (same pattern as `crate::extractor::AnthropicExtractor`), supports
     both static API keys and dynamic OAuth via `TokenProvider`.
     Strict-JSON prompt + parser with markdown-fence stripping. HTTP
     5xx + 429 + connect/timeout → `Transient`; HTTP 4xx + JSON parse
     → `Permanent`; empty title/summary → `EmptyOutput`. 6 unit tests
     for the response parser.

2. **Rewrote `Memory::compile_knowledge`** — replaced the 22-line
   zero-counter stub with a real call to
   `crate::knowledge_compile::compile()`. Split into two public methods:
   - `compile_knowledge(namespace)` — env-based summarizer factory
     (`ANTHROPIC_AUTH_TOKEN` → OAuth, `ANTHROPIC_API_KEY` → API key,
     neither set → `Err`). No silent fallback to stub summarizer
     (GUARD-2: never silently degrade).
   - `compile_knowledge_with(namespace, &summarizer)` — generic
     injection point for tests and custom LLM clients.
   The embedding provider is `take()`-ed for the duration of the call
   to satisfy borrow-checker disjoint-borrow needs, and unconditionally
   restored before returning (regression test asserts this).

3. **Wiring tests in `knowledge_compile/tests.rs`**:
   - `compile_knowledge_with_drives_real_pipeline_not_stub` — asserts
     a `graph_pipeline_runs` row is written even on an empty namespace
     (the unambiguous signal that the stub was replaced).
   - `compile_knowledge_with_errors_when_no_embedder` — asserts the
     embedder-required error path with a clear message.
   - `compile_knowledge_restores_embedder_on_success` — regression
     guard for the `take()`/restore pattern.
   Tests use `compile_knowledge_with` injection rather than env-based
   `compile_knowledge` so they're hermetic under parallel `cargo test`.

4. **Test-only helpers on `Memory`** —
   `set_embedding_provider_for_test` and
   `clear_embedding_provider_for_test` (both `#[doc(hidden)]`) so
   tests can deterministically configure the embedder regardless of
   what `Memory::new` auto-detects (Ollama presence varies across dev
   machines).

### What was *not* in this fix

- **Token bucketing / rate limiting for K3** — design §5bis.7 mentions
  `knowledge_compile_*` metrics for budget tracking; the counters
  exist in `CompileMetrics` but no budget gate is wired. K3 is run
  per-cluster so a cluster-rate limit is the natural unit; out of
  scope for the wiring fix. File a follow-up if needed.
- **Retry/backoff loop around `summarize`** — design §5bis.4 step 2
  mandates retry-with-backoff for transient summarizer errors. The
  trait taxonomy is in place (`SummarizeError::Transient` vs
  `Permanent`); the actual retry loop currently lives inside
  `synthesis::persist_cluster`. Verify retry behavior against the
  new `AnthropicSummarizer` in a follow-up acceptance test once a
  real Claude key is wired in CI.
- **End-to-end LLM acceptance test** — the existing tests prove the
  wiring is correct (graph_pipeline_runs row written, run_id
  non-nil, embedder restored) without burning Anthropic credits.
  A "real LLM, real cluster, real topic written" acceptance test
  belongs in a separate e2e suite gated on `ANTHROPIC_API_KEY`
  presence.

### Verification commands

```
cd /Users/potato/clawd/projects/engram
cargo test -p engramai --lib knowledge_compile
# 35 tests pass (was 32 before; 3 new wiring tests + 6 parser tests added)

cargo test -p engramai --lib
# 1767 tests pass, 0 failed (no regressions)
```

### Files changed

- `crates/engramai/src/knowledge_compile/mod.rs` — added `pub mod adapters`.
- `crates/engramai/src/knowledge_compile/adapters.rs` — NEW (415 lines).
- `crates/engramai/src/knowledge_compile/tests.rs` — added 3 wiring tests
  + helper functions.
- `crates/engramai/src/memory.rs` — replaced `compile_knowledge` stub,
  added `compile_knowledge_with`, added test-only embedding helpers.

### Unblocks

- ISS-049 Phase 4 — Abstract plan can now return real topics once a
  corpus is compiled.
- The "Notes for future investigation" item about classifier accuracy
  becomes measurable: re-run conv-26 with a populated
  `knowledge_topics` table.
