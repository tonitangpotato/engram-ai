# RUN-0026 Investigation — ISS-106 patch is a regression, not a fix

**Date:** 2026-05-06 (UTC ~17:35)
**Author:** RustClaw (session continuation; potato asked for a doc so future-me doesn't lose this)
**Status:** **Patch DOES NOT fix ISS-106.** Patch CAUSES ~22pp J-score regression (0.559 → 0.342).
**Verdict:** **Do not merge.** Stash/revert the `compile_knowledge("default")` insertion in `engram-bench/src/drivers/locomo.rs:616`. Re-investigate what L5/Abstract actually needs.

---

## The numbers (cite-before-claim — these are from on-disk JSONL summaries, not memory)

LoCoMo conv-26, 152 questions, LLM-as-judge (Anthropic Haiku), engram-bench harness:

| Run | When | What changed | J-score | multi-hop | open-domain | single-hop | temporal |
|---|---|---|---|---|---|---|---|
| **RUN-0024** | 15:31Z | baseline (occurred_at + retrieval k_seed/iss105 experiments stashed in tree) | **0.559** | 0.676 | 0.385 | 0.313 | 0.643 |
| RUN-0025 | 16:40Z | ISS-106 patch + same retrieval changes | 0.296 | 0.378 | 0.154 | 0.125 | 0.357 |
| **RUN-0026** | 17:21Z | ISS-106 patch ALONE (retrieval changes stashed back to baseline) | **0.342** | 0.378 | 0.154 | 0.156 | 0.443 |

Source files (cite-before-claim — verified to exist this session):

- `engram-bench/benchmarks/runs/2026-05-06T15-31-42Z_locomo/locomo_summary.json`
- `engram-bench/benchmarks/runs/2026-05-06T16-40-42Z_locomo/locomo_summary.json`
- `engram-bench/benchmarks/runs/2026-05-06T17-21-05Z_locomo/locomo_summary.json`
- `/tmp/run0026/run.log` (full stderr trace)

### Per-query flip analysis (RUN-0024 → RUN-0026)

Computed by joining `locomo_per_query.jsonl` on question `id`:

- Queries in both: **152** (full overlap)
- Lost (was correct, now wrong): **41**
  - multi-hop: 17
  - temporal: 15
  - single-hop: 6
  - open-domain: 3
- Gained (was wrong, now correct): **8**
  - multi-hop: 6
  - single-hop: 1
  - temporal: 1
- **Net: −33 queries**, almost all categories regress, none meaningfully improve.

The patch is uniformly worse, not a tradeoff. Categories ISS-106 *predicted would benefit* (single-hop, open-domain) regress as hard as multi-hop and temporal.

---

## What the patch was supposed to do

ISS-106 (`engram/.gid/issues/ISS-106/issue.md`) hypothesis:

> "Abstract sub-plan never contributes any candidates during LoCoMo runs because `knowledge_topics` table is empty. Calling `memory.compile_knowledge(namespace)` between ingest and query loops would populate that table; queries that classify as Abstract or hybrid-with-Abstract sub-plan would then surface compiler-synthesized topics, plugging the silent zero-hit branch in `crates/engramai/src/retrieval/plans/abstract_l5.rs:367-373`."

### What was added (the only change in this run vs RUN-0024 ingest path)

In `engram-bench/src/drivers/locomo.rs`, between the per-episode ingest loop and the per-question query loop:

```rust
// Step 2.5 (ISS-106): compile knowledge topics so the Abstract
// sub-plan has substrate to retrieve from.
memory.compile_knowledge("default").map_err(|e| {
    BenchError::Other(format!(
        "locomo replay: conversation `{}` compile_knowledge failed: {e}",
        conv.conversation_id
    ))
})?;
```

(`engram-bench/src/drivers/locomo.rs:616`, inside `replay_conversation`)

`compile_knowledge` lives in `engram/crates/engramai/src/memory.rs:6552`, calls `compile()` in `crates/engramai/src/knowledge_compile/mod.rs:131` which: selects candidates (importance ≥ 0.3, default), clusters them, calls Anthropic Haiku per cluster to synthesize topic title+summary, writes rows to `knowledge_topics` table.

The OAuth init line **does** appear once in `/tmp/run0026/run.log`:

```
[INFO] compile_knowledge: AnthropicSummarizer (OAuth) from ANTHROPIC_AUTH_TOKEN
```

So the call ran. Whether it wrote any topic rows is unverified (the bench uses `tempfile::tempdir()` in `harness::fresh_in_memory_db`, the DB is gone now).

---

## Why the patch breaks things (two effects, both bad)

### Effect 1 — ISS-106 itself isn't actually fixed

`grep "DowngradedL5Unavailable\|l5_unavailable" /tmp/run0026/run.log`:

```
3 × DowngradedL5Unavailable (Abstract sub-plan, hybrid context)
0 × Abstract.*outcome=Ok
36 × plan_kind=abstract  →  fallback ENTER trigger=abstract reason=l5_unavailable  →  outcome=downgraded_from_abstract
```

**Every single Abstract-classified query still downgrades**, both standalone-Abstract and Abstract-as-hybrid-sub-plan. Either:

- **(A)** `compile_knowledge` ran but produced 0 topics (candidates empty / clusterer empty / per-cluster synthesis errors swallowed at `knowledge_compile/mod.rs:269` log::warn), or
- **(B)** topics were written but the Abstract searcher's score floor (`min_topic_score`) drops them all below threshold (`abstract_l5.rs:380` — second downgrade branch), or
- **(C)** topics were written under one namespace but searched under another.

We can't distinguish these without instrumentation: `knowledge_compile::compile` doesn't `log::info!("topics_written={}", n)` — only the warn-on-cluster-error path is logged. The CompileReport returns `topics_written` to the caller, but the bench discards it.

**Action item:** instrument `compile_knowledge` to log `candidates_considered=N clusters_formed=K topics_written=M llm_calls=L` so the next experiment isn't blind. Tracked separately — don't add to ISS-106 fix scope.

### Effect 2 — answer quality degrades on queries that DON'T touch Abstract

This is the real damage. Sample regressions (RUN-0024 correct → RUN-0026 wrong), all from categories that should be unaffected by an empty Abstract sub-plan:

| qid | category | gold | RUN-0024 (correct) | RUN-0026 (wrong) |
|---|---|---|---|---|
| conv-26-q0 | multi-hop | "7 May 2023" | "Caroline went to LGBTQ support group on **May 7, 2023**" | "Caroline attended the LGBTQ support group on **2023-05-08**" |
| conv-26-q4 | single-hop | "Transgender woman" | "Caroline is a **trans woman**…" | "**I don't know.** The memories indicate Caroline has been on a journey…" |
| conv-26-q8 | multi-hop | "The week before 9 June 2023" | "speech at school event **last week before June 9**" | "speech at school event **on 2023-06-09**" |
| conv-26-q9 | multi-hop | "The week before 9 June 2023" | "met up with friends **last week from that conversation**" | "met up with friends **on 2023-05-29**" |
| conv-26-q10 | multi-hop | "4 years" | "known her current group of friends for **4 years**" | (not shown — also flipped) |

Patterns:
1. The model hallucinates a different specific date when the gold expects a relative phrase ("the week before X" → "on X").
2. Confidently correct → "I don't know" (q4: trans woman → refusal).
3. Off-by-one date errors (May 7 → May 8).

These queries don't classify as Abstract. So the only way ISS-106's patch can affect them is by changing **what the orchestrator's hybrid plan retrieves and feeds to the answer LLM** for non-Abstract paths.

### Mechanism (best-guess hypothesis, NOT verified)

Even when Abstract sub-plan downgrades, the orchestrator's fusion stage still ranks topic results from other sub-plans alongside memory results. `engram-bench/src/drivers/locomo.rs:533-541`:

```rust
ScoredResult::Topic { topic, .. } => {
    format!("(topic) {}: {}", topic.title, topic.summary)
}
```

**Hypothesis:** even a small number of topic rows synthesized by `compile_knowledge` get picked up by other plans (associative? hybrid Episodic with topic-aware fusion?) and crowd out specific memory rows in the top-K context block. The answer LLM then sees an LLM-summarized cluster *instead of* the raw conversation episode that contains the actual date / relationship / fact.

If the topic summary is itself an LLM hallucination of "Caroline went to a support group around early May 2023", the answer LLM will quote *the topic*, not the verbatim memory. That matches what the regressions look like — it's not retrieval missing, it's retrieval surfacing a paraphrase that is one day off / one degree more vague.

**Cannot confirm without `ENGRAM_BENCH_DUMP_CANDIDATES=1`** — that env var isn't set in the run, so we don't have per-query candidate dumps showing whether topic rows appeared in the top-K. Setting it for the next run is cheap.

---

## What is NOT the cause (ruled out)

- ❌ **Retrieval changes (k_seed / ISS-105 / ISS-104).** Stashed in `engram` repo as `stash@{0}` "iss105+k_seed-experiments-temp-park" before RUN-0026. RUN-0026 = clean engram + ISS-106 patch only. Regression persists, so retrieval changes are not the cause. (RUN-0025 had both → −0.263; RUN-0026 has only ISS-106 → −0.217. ISS-106 alone accounts for most of it.)
- ❌ **Anthropic API errors.** OAuth init succeeded; no API error lines in run.log; bench finished all 152 queries.
- ❌ **LLM-judge variance.** Same scorer, same model, same dataset, same fixture sha. Per-query verdicts diff cleanly.
- ❌ **Namespace mismatch breaking ingest.** Ingest goes through `ingest_with_stats_at` → `store_raw` with `StorageMeta::default()` → namespace `None` → fallback "default". `compile_knowledge("default")` matches. Query path `GraphQuery::new(...)` without `.with_namespace(...)` → also "default". All three agree on `default`. Per-episode dedup logs (`merge_enriched_into`) appear, confirming ingest reached storage.

---

## What we actually know vs what we're guessing

**Verified (cite-before-claim):**

- ✅ J-scores from JSONL summary files (numbers above)
- ✅ Per-query flip counts from JSONL diff
- ✅ `compile_knowledge` call site and content (read from `engram-bench/src/drivers/locomo.rs:616`)
- ✅ `compile_knowledge` ran once (log line in `/tmp/run0026/run.log`)
- ✅ Abstract sub-plan still downgrades on every call (l5_unavailable count in log)
- ✅ Retrieval changes were not active in RUN-0026 (stash present in `engram` repo)

**Unverified (hypothesis only):**

- ❓ Whether `compile_knowledge` wrote any topic rows. Need instrumentation.
- ❓ Whether topic rows leaked into non-Abstract retrieval paths. Need `ENGRAM_BENCH_DUMP_CANDIDATES=1`.
- ❓ Whether topic summaries themselves are wrong (vs raw memories). Need to read topic content from a persistent DB.
- ❓ Why Abstract still downgrades after compile ran. Need either (A) topic-count log or (B) re-run with persistent DB and `sqlite3 SELECT COUNT(*) FROM knowledge_topics`.

---

## Recommended next steps (in order)

1. **Revert the ISS-106 patch.** Either drop the `compile_knowledge` call from `engram-bench/src/drivers/locomo.rs` or guard it behind `if std::env::var("ENGRAM_BENCH_COMPILE_KNOWLEDGE").is_ok()`. Default OFF until we understand the regression.
2. **Add instrumentation in `engramai`.** In `crates/engramai/src/knowledge_compile/mod.rs`, around line 220 (after `topics_written += ...`), add:
   ```rust
   log::info!(
       "compile_knowledge done: ns={} candidates={} clusters={} topics_written={} llm_calls={}",
       namespace, candidates_considered, clusters.len(), topics_written, llm_calls
   );
   ```
   This is single-namespace, low-cost, no behavior change.
3. **Re-run with persistent DB.** Replace `fresh_in_memory_db()` with a tempfile path that doesn't get cleaned up, OR add `ENGRAM_BENCH_KEEP_DB=/tmp/run0027.db`. After the run: `sqlite3 .../graph.db "SELECT COUNT(*), namespace FROM knowledge_topics GROUP BY namespace"`. This answers (A) directly.
4. **Re-run with `ENGRAM_BENCH_DUMP_CANDIDATES=1`.** Compare the top-K context blocks for the regressed queries (q0, q4, q8, q9). If `(topic) ...` lines appear in RUN-0026 candidate dumps but not in a clean-engram baseline, hypothesis confirmed.
5. **Only then** decide on a real fix. Possibilities: (a) raise `min_topic_score` floor so weak topics drop out before fusion; (b) gate Abstract substrate on conversation length (LoCoMo single-conv has too few episodes for Haiku-summarized topics to be more accurate than raw ones); (c) skip topic compile entirely for benchmark drivers and design a smaller, deterministic substrate.

---

## Lessons (engineering, not narrative)

1. **A 22pp J-score drop after a single-line "fix" is a flashing red light.** Don't celebrate the call line in the log. The signal we cared about (Abstract `outcome=Ok` instead of `DowngradedL5Unavailable`) **never appeared**, so the fix did not do what was hypothesized. The score only confirmed the side-effect is harmful.
2. **`compile_knowledge` is opaque without instrumentation.** It returns a `CompileReport` with counts; we throw it away. Three lines of `log::info!` would have made this investigation 10× faster.
3. **Bench harness needs persistent DB option.** `fresh_in_memory_db` → `tempfile::tempdir()` cleaned at drop is great for hermeticity, terrible for forensics. Add `ENGRAM_BENCH_KEEP_DB=path` env override.
4. **The cite-before-claim skill caught me.** When potato asked "investigate", my first instinct was to recall what I "knew" about the patch. That was unsafe — I'd been swapping retrieval changes in/out for an hour. Reading per-query JSONL and the run log first, before forming a story, is what surfaced the "Abstract still downgrades" finding (which contradicts the patch's whole premise).

---

## File pointers (for next session)

- Patch site: `engram-bench/src/drivers/locomo.rs:606-621` (Step 2.5 block)
- Compile entry: `engram/crates/engramai/src/memory.rs:6552`
- Compile pipeline: `engram/crates/engramai/src/knowledge_compile/mod.rs:131`
- Abstract downgrade branches: `engram/crates/engramai/src/retrieval/plans/abstract_l5.rs:367-385`
- Bench retrieval call: `engram-bench/src/drivers/locomo.rs:634` (no `with_namespace` — implicitly "default")
- Stashed retrieval experiments: `cd engram && git stash list` → `stash@{0}: iss105+k_seed-experiments-temp-park`
- Run logs: `/tmp/run0025/run.log` and `/tmp/run0026/run.log`
- Run summaries: `engram-bench/benchmarks/runs/2026-05-06T{15-31,16-40,17-21}-*Z_locomo/`
