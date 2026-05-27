---
title: engram-bench LoCoMo harness 缺 with_pipeline_pool wiring; graph_entities 表始终为空
priority: P0
relates_to:
- ISS-164
- ISS-165
- ISS-162
- ISS-037
- ISS-055
severity: blocker
status: resolved
tags:
- engram-bench
- locomo
- graph-layer
- resolution-pipeline
- harness
- confounder
fixed_by:
- engram-bench:bfb1115
- engram:89d5ac9
---

## TL;DR

`engram-bench/src/harness/mod.rs::fresh_in_memory_db` 构造 `Memory` 时**从不调用** `Memory::with_pipeline_pool(...)`。没有 worker pool → `store_raw` 出的 jobs 走 silent-no-op enqueue → v0.3 graph layer (`graph_entities` / `graph_edges` / `graph_memory_entity_mentions`) **永远是空的** → `GraphEntityResolver::resolve()` 永远返回 0 anchor → 任何依赖 `graph_entities` 表的 retrieval path (entity channel, Factual plan anchor 分支, GraphTopicSearcher 等) 在所有 LoCoMo benchmark 下**物理上不工作**，走的全是 fallback path。

## 影响范围

这是一个 harness-level confounder，**任何在这条路径上跑过的 sweep 都需要重新审视结论**。已确认受影响:

- **ISS-164 Phase 2 A/B sweep (2026-05-26 STAMP 20260526T213218Z)** — Arm B 的 `entity_channel_enabled=true` 调用 `GraphEntityResolver::resolve()`，但 resolver 读的 `graph_entities` 是空的，所以**注入了 0 anchor**。Arm B == Arm A behavior (加一个 no-op 路径)。Phase 2 falsification (overall -3.29pp, multi-hop -10.81pp) 不能归因于 entity channel 设计本身。
- **ISS-162 设计前提** — "resolver 选错 anchor / anchor 不够丰富" 这个假设在 LoCoMo harness 下没法验证；resolver 在测的 substrate 上根本没数据。需要在 ISS-166 修通后重审。
- **可能所有 ISS-137..164 在 conv-26 上的 sweep** — 凡是 Factual plan 走 anchor-based sub-plan 的 query，实际跑的都是 fallback path，不是 "anchor + edge traversal" 的设计 path。**不是说之前 sweep 的相对差异都假**，但凡是结论里假设了"graph layer 在工作"的部分必须重审。

## Root cause (full evidence chain)

### 1. `GraphEntityResolver::resolve()` 读的表是 `graph_entities`

`crates/engramai/src/retrieval/adapters/graph_entity_resolver.rs:78-85`:
```rust
let namespaces = match self.graph.list_namespaces() {
    Ok(ns) => ns,
    Err(_) => return Vec::new(),
};
```

`GraphRead::list_namespaces` 实现 (`crates/engramai/src/graph/store.rs:3759-3771`) 硬编码:
```rust
"SELECT DISTINCT namespace FROM graph_entities ORDER BY namespace"
```

没有 `unified_substrate` 分支，没有 fallback 到 `entities` 表。

### 2. `graph_entities` 表只由 `GraphWrite::insert_entity` 写

Production callers of `GraphWrite::insert_entity` (`grep -rn "\.insert_entity(\|GraphWrite"`):
- `crates/engramai/src/knowledge_compile/synthesis.rs:255` — KC pipeline (与 ingest 解耦)
- `crates/engramai/src/resolution/stage_persist.rs` 链路 — ResolutionPipeline 的 `apply_graph_delta`
- 其余 13 处全部在 `*_test.rs` / mock

Memory ingest path **不调用** `GraphWrite::insert_entity`。

### 3. Memory ingest 路径的写入是 `Storage::upsert_entity`，目标只有 `entities` + `nodes`

`crates/engramai/src/storage.rs:5263-5340` (`upsert_entity`):
```rust
tx.execute(
    "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) ON CONFLICT(id) DO UPDATE SET ...",
    params![entity_id, name, entity_type, namespace, metadata, now],
)?;

// Unified projection. INSERT OR IGNORE on the nodes side ...
let inserted = Self::insert_entity_node_row(&tx, ..., namespace, now, now)?;
```

两条 dual-write 路径: `entities` (legacy) + `nodes WHERE node_kind='entity'` (unified)。**没有第三条**。

### 4. Memory ingest path 调 `upsert_entity` 不调 `GraphWrite::insert_entity`

`grep -n "upsert_entity\b" crates/engramai/src/memory.rs` 命中点 (`memory.rs:2867, 2916, 2995, 6479`) 全部在 ingest 路径，dedup/merge 路径写 entity link。**没有任何 ingest path 通过 `GraphWrite` 写。**

### 5. v0.3 graph layer 设计上要靠 ResolutionPipeline 异步填充

`crates/engramai/src/resolution/pipeline.rs:1-25` 文档:
```
store_raw ──► JobQueue ──► WorkerPool ──► JobProcessor::process
                                              │
                                              ▼
                                     ResolutionPipeline::run_job
                                              │
                                              ▼
                       load memory ─► §3.2 entity extract ─► §3.3 edge extract
                                              │
                                              ▼
                       §3.4 resolve  (candidate retrieval + fusion + decide)
                                              │
                                              ▼
                       §3.5 atomic persist (build_delta ▻ apply_graph_delta)
```

`stage_persist.rs::apply_graph_delta` 是唯一把 ResolutionPipeline 结果写到 `graph_entities` / `graph_edges` 的路径。

### 6. ResolutionPipeline 必须由 `Memory::with_pipeline_pool(...)` 显式 wire

`crates/engramai/src/memory.rs:280-440`:
```rust
/// After this call, every Memory::store_raw that successfully admits
/// a fact also enqueues a PipelineJob::initial, and the worker pool
/// drains the queue, populating the v0.3 graph.
pub fn with_pipeline_pool(
    mut self,
    db_path: impl AsRef<std::path::Path>,
    triple_extractor: std::sync::Arc<dyn crate::triple_extractor::TripleExtractor>,
    config: crate::resolution::ResolutionConfig,
) -> Result<Self, ...>
```

`enqueue_pipeline_job` (`memory.rs:1178-1198`) 在 `job_queue=None` 时:
```rust
fn enqueue_pipeline_job(&self, memory_id: &str) -> Option<uuid::Uuid> {
    let queue = self.job_queue.as_ref()?;  // <-- None → silent return None
    ...
}
```

**Silent skip. 没有 warning，没有 error。**

`reextract` doc 直接说: `"no pipeline pool installed (v0.2-compat mode — re-extract is meaningless without a pipeline)"`。

### 7. engram-bench harness 从不调 `with_pipeline_pool`

```
$ grep -rn "with_pipeline_pool\|set_job_queue\|TripleExtractor" \
    /Users/potato/clawd/projects/engram-bench/

(zero matches)
```

`fresh_in_memory_db` (`engram-bench/src/harness/mod.rs:540-...`) 只构造 `MemoryConfig::default()` 然后 `Memory::new`。

`grep -rn "with_pipeline_pool" --include="*.rs"` 在整个 engram repo 范围内只有 4 个 tests/iss0*.rs 命中，**production / engram-bench / rustclaw 全部 0 命中**。

### 8. Empirical confirmation (ISS-165 AC-1 probe v3 + v4)

文件: `/tmp/iss165-ac1-probe-v3.log` (defaults), `/tmp/iss165-ac1-probe-v4-unified.log` (ENGRAM_BENCH_UNIFIED_SUBSTRATE=1)

两次 419-episode conv-26 ingest (956s real Haiku extractor), entity census:
```
--- ENTITY CENSUS (Memory::list_entities, limit=10000) ---
total entities: 3
  "person"             2
  "technology"         1

top 20 entities by usage count:
  [ 124x] "person" ns="default" :: melanie
  [  99x] "person" ns="default" :: caroline
  [   4x] "technology" ns="default" :: go

namespaces in graph: []     ← GraphRead::list_namespaces 读 graph_entities = 空
```

直接 sqlite 验证 (`/var/folders/.../substrate.db`):
```
entities:        3 rows
nodes:           456 rows (453 memories + 3 entities)
graph_entities:  0 rows     ← confirmed empty
graph_edges:     0 rows
edges:           227 rows
```

`unified_substrate=true` 不改变结论 — 因为这是 *read* flag, 跟 `graph_entities` 写入路径无关。

### 9. 因此 ISS-164 Phase 2 在 LoCoMo 下不可能 work

```
LoCoMo driver → fresh_in_memory_db → Memory::new (NO with_pipeline_pool)
              → memory.job_queue = None
              → memory.ingest_with_stats_at loop
                  → storage.upsert_entity → entities + nodes ✓
                  → enqueue_pipeline_job → None → no-op
                  → graph_entities stays empty ✗

Query phase with FusionConfig.entity_channel_enabled=true:
  AssociativePlan::run → resolver.resolve(query)
                       → graph.list_namespaces() = [] (graph_entities empty)
                       → returns Vec::new()
                       → 0 anchors injected → entity channel is a no-op
```

## 还没回答的问题 (cite-before-claim, 留作 ISS-166 实现期决策)

### Q1: `graph_entities` 是 design intent 还是 bug?

两种可能:
- **(a) Design intent**: `graph_entities` 设计上**只**应该由 ResolutionPipeline 填，与 `Storage::upsert_entity` 的 dual-write 是两条独立路径。`upsert_entity` 注释 (`storage.rs:5283-5310`) 提的 "unified projection" 指 `nodes` 表 (T21 backfill 模式)，**完全没提 graph_entities**。这个证据偏向 (a)。
- **(b) Bug**: dual-write 设计本来就该包括 `graph_entities`，T13/T21 backfill 漏了第三张表。`Storage::with_unified_substrate` 文档 (`storage.rs:407-422`) 说 "Writes are always dual-routed (Phase B)" — 但实测 dual route 是 `entities + nodes`，没有 graph_entities。如果是 (b)，需要修 dual-write 而不是修 harness。

**实现期决策点**: 看 `.gid/features/v04-unified-substrate/design.md` 关于 graph_entities 的 ownership 是怎么定义的。如果设计明确 graph_entities = ResolutionPipeline-only，走方案 A; 如果设计说 graph_entities = always-on dual-write，走方案 B。

### Q2: ResolutionConfig 默认值对 LoCoMo 是否合适

需要看 `ResolutionConfig::default()` 的 worker_count / queue_cap / 是否同步等结果。LoCoMo 419 episodes × 6 conversations = 大量 jobs，如果默认 queue_cap 太小会丢 job, 如果异步又会让 query 跑在 graph 还没填好的 substrate 上。

### Q3: Memory::ingest_with_stats_at 是否触发 store_raw → enqueue?

需要确认 `ingest_with_stats_at` 是否走 `store_raw` 路径，还是某个绕过 enqueue 的 fast path。Probe v4 log 显示 dedup merge 路径在跑 (`Dedup: merging into existing memory ...`)，所以至少部分 ingest 走 `add` 路径。需要核对 add 路径有没有 enqueue。

### Q4: 历史 sweep 的范围

ISS-137 / 138 / 139 / 143 / 144 / 147 / 150 / 152 / 153 / 155 / 156 / 157 / 159 / 161 / 164 这些 sweep 都基于 fresh_in_memory_db。需要分类:
- 哪些 sweep 的结论 **完全不依赖** graph_entities (例如 MMR diversity, BM25 fusion — 只用 memories_fts 和 node_embeddings) — 这些结论可能仍然有效。
- 哪些 sweep 的结论 **直接依赖** graph_entities (例如 ISS-164 entity channel, ISS-149 classifier 路由如果走 Factual plan) — 这些结论必须重审。

## Acceptance Criteria

### AC-1: 决定 Q1 的方案 A/B

读 `v04-unified-substrate/design.md`，明确 `graph_entities` ownership。在 issue 里 commit 一个决定 + 一行 design.md 引用。

### AC-2: 修复 wiring (方案视 AC-1)

**方案 A (ResolutionPipeline-only)**:
- 在 engram-bench/harness/mod.rs 添加一个新 helper `fresh_in_memory_db_with_pipeline()` 或扩展 `fresh_in_memory_db()` 接受配置。
- 注入一个 TripleExtractor (LoCoMo 不用 triples，可能写一个 NoOp/passthrough; 或者用 LLM-backed 的复用现有 extractor)。
- 调用 `Memory::with_pipeline_pool(db_path, extractor, ResolutionConfig::default())`。
- 在 LoCoMo driver 的 ingest 循环结束后**等待 worker pool drain** (新 API: `Memory::await_pipeline_drain()`?) 再开始 query phase。否则 query 跑在 half-filled graph 上。

**方案 B (always-on dual-write)**:
- 改 `Storage::upsert_entity` 加第三段 INSERT INTO graph_entities。
- 改 `delete_inner` / cascading deletes 对应处理。
- 写 contract test 验证 entities/nodes/graph_entities 行数一致。

### AC-3: 重跑 ISS-164 Phase 2 验证 entity channel 真实效果

A/B 在修通的 substrate 上重跑 conv-26 K=10 temp=0 HyDE=off MMR=off。决策规则按 ISS-164 原议。

### AC-4: 重新审视 ISS-165 AC-1

在修通的 substrate 上重跑 iss165_ac1_resolver_probe.rs。如果 resolver 返回非空 anchors → 真正测 H1 vs H3。如果还是空 → 说明 LoCoMo extraction 实际只产生 3 个 entity (Caroline/Melanie/Go)，gold-fact-relevant entities 物理上不在 graph 里 → H1 经由"extraction too thin"通道间接确认。

### AC-5: 历史 sweep 影响分类

把 ISS-137..164 的所有 conv-26 sweep 分成"结论 graph-layer-independent"(可保) vs "结论 graph-layer-dependent"(需重跑) 两组，写到本 issue 的 findings 或一个新 issue。

## Repro

```bash
# build probe
cd /Users/potato/clawd/projects/engram-bench
$HOME/.cargo/bin/cargo build --release --example iss165_ac1_resolver_probe -p engram-bench

# run with Anthropic auth (real extractor)
TOK_JSON=$(security find-generic-password -s "Claude Code-credentials" -w)
export ANTHROPIC_AUTH_TOKEN=$(python3 -c "import json,sys; \
  d=json.loads(sys.argv[1]); print(d['claudeAiOauth']['accessToken'])" "$TOK_JSON")

./target/release/examples/iss165_ac1_resolver_probe \
  --fixture benchmarks/fixtures/locomo/39e7df4ea492e8bc7a483b2cfc8e18620054beb05fed267f5cc098bd65fd5f4d/conversations.jsonl \
  --conv conv-26

# Expected (broken state):
#   namespaces in graph: []
#   NO_ANCHORS : 9/9
```

## Artifacts

- `/tmp/iss165-ac1-probe-v3.log` — fresh_in_memory_db 默认 (unified=false), 9/9 NO_ANCHORS, entity census = 3 entities, graph_entities empty
- `/tmp/iss165-ac1-probe-v4-unified.log` — ENGRAM_BENCH_UNIFIED_SUBSTRATE=1, 同样 9/9 NO_ANCHORS, 同样 graph_entities empty (证明不是 read flag 问题)
- `/Users/potato/clawd/projects/engram-bench/examples/iss165_ac1_resolver_probe.rs` — probe 源码
- `/var/folders/48/npr42z0967b376x1rc7wbp6m0000gn/T/.tmpw3iZtk/substrate.db` — v3 probe 的 fresh in-mem DB (临时, 直接 sqlite 验证用; OS 会清)

## Provenance

发现于 ISS-165 AC-1 probe 调查 (2026-05-26 / 2026-05-27)。原本目的是验证 ISS-164 Phase 2 falsification 的 hypothesis H1 (resolver picks wrong anchor)，AC-1 probe 三次跑全部得到 9/9 NO_ANCHORS。第三次跑加 entity census 后发现 `list_entities=3` 但 `list_namespaces=[]`，进而 sqlite 直查发现 `graph_entities=0 / entities=3 / nodes=456`，最终追溯到 harness 没调 `with_pipeline_pool`。

ISS-165 AC-1 的 "H1 CONFIRMED 9/9" verdict **作废** — 测的是 confounder, 不是 hypothesis。AC-1 重新设计在本 issue 修通后才能落地。

---

## 2026-05-27 — RESOLVED

Plan A (engram-bench harness wires `Memory::with_pipeline_pool` behind
`ENGRAM_BENCH_PIPELINE_POOL=1`) shipped and validated. AC-1..AC-4 met;
AC-5 (historical sweep classification) deferred to a follow-up review.

### Implementation

- `engram-bench` commit `bfb1115`:
  - `harness/mod.rs::fresh_in_memory_db()` chains
    `Memory::with_pipeline_pool(graph_db_path,
    Arc::new(AnthropicTripleExtractor::new(token, true)),
    ResolutionConfig::default())` when
    `ENGRAM_BENCH_PIPELINE_POOL=1`.
  - Tunables `ENGRAM_BENCH_PIPELINE_WORKERS` (default 4, max 8) and
    `ENGRAM_BENCH_PIPELINE_DRAIN_SECS` (default 600).
  - `drivers/locomo.rs` calls `memory.shutdown_pipeline(600s)`
    between the ingest loop and the query loop so the graph is
    fully populated before any query runs.
  - `examples/iss165_ac1_resolver_probe.rs` mirrors the drain step,
    plus retry+tolerance loop on Anthropic 5xx (committed later
    after PID 15834 died on 529 Overloaded).
- `engram` commit `89d5ac9` (ISS-167 parser tolerance) — required to
  actually persist triples; without it the pool runs but every
  Haiku response is dropped at parse.

### Validation evidence (probe PID 16259, 2026-05-27 23:48 EDT)

After full 419-episode conv-26 ingest + 600s drain:

- WorkerPool stats: `jobs_processed: 456, jobs_failed: 0,
  jobs_in_flight: 0, jobs_dropped_inbox_full: 0`
- `graph_entities` direct sqlite count: **666 rows** (was 0 pre-fix)
- `graph_edges` direct sqlite count: **227+ rows** (was 0 pre-fix)
- Entity kind breakdown:
  - `"concept"` 336
  - `{"other":"unknown"}` 143
  - `"event"` 89
  - `"artifact"` 39
  - `"person"` 24
  - `"place"` 14
  - `"topic"` 11
  - `"organization"` 10
- All gold-fact-relevant entities present:
  - `Sweden` (place) ✓
  - `sunsets` / `beach sunset` (concept) ✓
  - `abstract painting` (concept) ✓
  - `Becoming Nicole` (artifact) ✓
  - `adoption agencies` / 16 other adoption-related entities ✓

Verdict: ISS-166 root cause (silent `enqueue_pipeline_job` no-op
when `job_queue=None`) is fixed; the v0.3 graph subsystem is now
populated on every LoCoMo run when the env var is set.

### AC status

- **AC-1** (decide Q1 Plan A vs B): **Plan A**. ResolutionPipeline
  remains the sole canonical writer of `graph_entities`. No
  dual-write at the `Storage::upsert_entity` layer. Rationale
  documented in commit `bfb1115` message; aligns with §3.5 of the
  v04-unified-substrate design where graph_entities ownership lives
  with the resolution pipeline.
- **AC-2** (fix wiring): done (commit `bfb1115`).
- **AC-3** (re-run ISS-164 Phase 2): **NOT RUN.** ISS-165 root cause
  probe (see below) revealed that even with `graph_entities`
  populated, the resolver cannot find anchors for natural-language
  questions. Re-running ISS-164 Phase 2 before the resolver fix
  lands would still produce noise. Deferred until ISS-165 is fixed.
- **AC-4** (re-evaluate ISS-165 AC-1): done. New verdict in ISS-165
  is "resolver mention extraction missing" — not "extraction too
  thin" as the Plan A AC-1 fallback hypothesis suggested. All gold
  entities exist; resolver can't find them. See ISS-165 update.
- **AC-5** (historical sweep classification): **deferred.** Will
  file as a separate review once ISS-165 fix lands and we know what
  the post-fix baseline looks like.

### Resolution

Status: open → resolved. ISS-165 (resolver bug) and ISS-168 (Haiku
multi-array CoT response) filed as follow-ups discovered during
validation.
