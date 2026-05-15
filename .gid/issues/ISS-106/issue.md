---
id: ISS-106
title: Abstract sub-plan is dead code on LoCoMo (knowledge_topics never populated)
kind: issue
status: blocked
priority: high
labels:
- retrieval
- benchmark
- silent-bug
- upside
- blocked-by-clustering
relates_to:
- ISS-105
- ISS-104
- .gid/issues/ISS-111/issue.md
blocked_by: .gid/issues/ISS-109/issue.md
---

# ISS-106: Abstract sub-plan is dead code on LoCoMo

## TL;DR

While analyzing ISS-105 (Hybrid sub-plan K hardcoding), I discovered that
the **Abstract sub-plan never contributes any candidates** during LoCoMo
benchmark runs. Reason: `knowledge_topics` table is empty in every
benchmark conv-DB. Every Abstract call hits `hits.is_empty()` and
returns `DowngradedL5Unavailable`.

This means **all J-score gains from RUN-0017 → RUN-0023 (~50pp total)
came from Factual + Episodic + Affective only.** Abstract is unmeasured
upside.

## Evidence (verified 2026-05-06)

```bash
$ cd /Users/potato/clawd/projects/cogmembench/.engram_dbs
$ sqlite3 conv-26.graph.db "SELECT COUNT(*) FROM knowledge_topics"
0
```

Code path that goes silent:
`crates/engramai/src/retrieval/plans/abstract_l5.rs:367-373`

```rust
// L5 substrate-empty branch: searcher returned nothing.
if hits.is_empty() {
    return AbstractPlanResult {
        candidates: Vec::new(),
        outcome: AbstractOutcome::DowngradedL5Unavailable,
        elapsed: started.elapsed(),
    };
}
```

`PlanTrace.downgrades` records this, but it's per-query — nobody
aggregates it across a benchmark run, so the pattern was invisible.

## Why this is high-priority

1. **Real upside.** Abstract is one of 4 fuse-contributing sub-plans
   (Factual / Episodic / Abstract / Affective). LoCoMo has been running
   on 3 sub-plans without realizing it. Multi-hop and topic-related
   queries are exactly where Abstract should help — and multi-hop is
   currently the weakest category (RUN-0023: 56.8%, lowest).

2. **Silent failure mode.** `DowngradedL5Unavailable` is logged
   per-query but never surfaces in the run summary. We've been blind
   to this for ~50pp of "improvement" runs.

3. **It tells us something about the bench harness.** cogmembench
   adapter ingests memories but never triggers knowledge consolidation
   (the KnowledgeCompiler that fills `knowledge_topics`). This is
   either:
   - **By design** (LoCoMo has no time-of-use to trigger compilation)
     → consolidation must run as part of ingestion / one-shot at end
   - **A bug** (compiler is supposed to run but doesn't)

## Investigation needed (before fix)

1. **Confirm the root cause** in cogmembench adapter:
   `cogmembench/src/adapters/engram/` — find where memories are
   ingested, check if `KnowledgeCompiler::compile()` (or equivalent)
   is ever called.

2. **Decide the fix point**:
   - If LoCoMo ingestion is single-shot, run compiler once after
     ingest before queries start
   - If it's multi-pass, run compiler at conversation boundaries
     (every N memories, or every session-end marker)

3. **Measure the upside**: after fixing, re-run RUN-0023 (K=50)
   and compare. Hypothesis: multi-hop bucket gains the most because
   topics enable cross-conversation reasoning.

## Out of scope for this issue

- Fixing Abstract sub-plan's K formula (that's ISS-105 — but ISS-105
  can defer Abstract until ISS-106 is resolved, since `k_topics` size
  doesn't matter when the table is empty).
- Generic "downgrade observability" (we should aggregate downgrade
  reasons in run summaries — separate hygiene issue, file later).

## Acceptance criteria

- [ ] Identify why `knowledge_topics` is empty post-ingest in
      cogmembench LoCoMo runs
- [ ] Fix so that consolidation runs as part of (or after) ingestion
- [ ] Verify `SELECT COUNT(*) FROM knowledge_topics` > 0 in a
      post-ingest conv-DB
- [ ] Re-run benchmark (K=50, comparable to RUN-0023): document
      delta in multi-hop and overall J-score
- [ ] Add downgrade-reason aggregation to run summary (optional but
      recommended — prevents recurrence of silent-failure pattern)

## Discovered

While writing `ISS-105/ROOT-CAUSE-ANALYSIS.md` §5.2 — wrote "E ≈ 5"
as the expansion factor for Abstract, then went to measure E
empirically and found E = 0 because the topic table is empty.

---

## Update 2026-05-06: RUN-0024 confirms ISS-106 is the next blocker

**RUN-0024 results (ISS-105 fix shipped, K=50, n=152):**

- overall: **0.5592** (vs RUN-0023 0.5329, +2.6pp)
- multi-hop: **0.6757** (vs 0.5135, +16.2pp) ← ISS-105 working as designed
- temporal: 0.6429 (vs 0.6714, -2.9pp, noise)
- single-hop: 0.3125 (vs 0.3125, 0pp) ← stuck
- open-domain: 0.3846 (vs 0.3846, 0pp) ← stuck

**Diagnosis:** single-hop and open-domain didn't move at all.
Inspection of `/tmp/run-0024.log` shows queries like
`What did Caroline research?`, `What is Caroline's identity?` all
trigger `plan_kind=abstract` → `outcome=downgraded_from_abstract`.
These are exactly the buckets Abstract is supposed to serve. Until
`knowledge_topics` is populated, both buckets are capped where they
are. ISS-105 confirmed the retrieval pipeline is healthy; ISS-106 is
the only remaining structural lever before approaching ship gate
(0.685).

**Run artifact:** `engram-bench/benchmarks/runs/2026-05-06T15-31-42Z_locomo/locomo_summary.json`

## Patch draft (NOT YET APPLIED — awaiting potato approval)

**Root cause confirmed:** `engram-bench/src/drivers/locomo.rs::replay_conversation()`
ingests episodes via `ingest_with_stats_at` (line 595) then jumps
directly to `graph_query_locked` (line 617). It never calls
`Memory::compile_knowledge(namespace)`. `knowledge_topics` table
stays empty → every Abstract sub-plan returns
`DowngradedL5Unavailable`.

**Patch site:** `engram-bench/src/drivers/locomo.rs`, between line
605 (end of ingestion `for` loop) and line 607 (start of
"Step 3: for each gold question").

**Proposed insertion (~10 lines):**

```rust
    // Step 2.5 (ISS-106): compile knowledge topics so Abstract sub-plan
    // has a substrate. Without this, Abstract returns
    // DowngradedL5Unavailable for every query (single-hop and
    // open-domain buckets get no Abstract contribution).
    //
    // Namespace: "default" (matches Memory's default namespace for
    // ingest_with_stats_at — confirmed via memory.rs).
    //
    // Cost: 1 LLM call per topic cluster (Haiku, ~200ms each).
    // Typical conversation produces 5–15 clusters → 1–3s overhead per
    // conversation. Acceptable for the 10-conv LoCoMo benchmark.
    memory
        .compile_knowledge("default")
        .map_err(|e| {
            BenchError::Other(format!(
                "locomo replay: conversation `{}` knowledge compile failed: {e}",
                conv.conversation_id
            ))
        })?;
```

**Required env vars at run time:**
- `ANTHROPIC_AUTH_TOKEN` (OAuth, preferred) OR
- `ANTHROPIC_API_KEY` (API key)
- Optional: `ENGRAM_KNOWLEDGE_COMPILE_MODEL` (default `claude-haiku-4-5-20251001`)

**Cost estimate for full LoCoMo run:**
- 10 conversations × ~10 topic clusters × Haiku call (~$0.001 each)
  ≈ $0.10 per benchmark run
- Latency: ~10–30s additional ingestion time per run

**Verification plan after applying:**
1. Build clean: `cargo build -p engram-bench --release`
2. Smoke run (n=5 queries, single conv) → confirm `knowledge_topics`
   table has rows (`sqlite3 ... "SELECT COUNT(*) FROM
   knowledge_topics"`)
3. Inspect log: confirm at least some queries log
   `outcome=ok` for `plan_kind=abstract` (not all downgraded)
4. Full RUN-0025 (K=50, n=152) → compare J-score per category
5. Hypothesis: single-hop and open-domain both move; multi-hop
   stays around 0.68 (already at ceiling for this fix)

**Rollback plan:** if RUN-0025 J-score ≤ RUN-0024 J-score, the patch
is wrong — Abstract substrate hurts more than helps. Revert is
trivial (delete the inserted block).

**Why a patch in the bench driver and not Memory itself:** Memory's
ingestion API is intentionally split from compilation (compilation is
costly + LLM-bound). Auto-compiling on every ingest would change
production semantics. Bench drivers are the right place to compose
"ingest then compile" because they own the benchmark contract
(steady-state evaluation, not streaming production).

## Update 2026-05-15: ISS-111 no longer blocks

ISS-111 (clusterer super-cluster collapse) — the underlying bug
that made RUN-0026 worse than RUN-0024 when the compile_knowledge
patch was applied — has been fixed and closed (commit `7f41bf7`,
mutual k-NN edge strategy). The patch above can now be re-applied
without producing a poison-pill super-topic.

This ISS-106 itself is still **blocked** pending the same decision
that gates v0.4 T31 (LoCoMo parity campaign): we don't want to burn
API budget on a J-score baseline that v0.4 substrate flip will
invalidate. Folded into T31 — when that bench run happens, the
ISS-106 patch will be active and its effect measured at the same
time as the ISS-111 verification and the substrate-equivalence check.
One run, three answers.
