# Architecture Note: Consolidation vs Synthesis vs Knowledge Compiler

> Three independent subsystems in `engramai` aggregate / abstract memory. They are not duplicates and they do not replace each other. This note maps their boundaries so future debates don't re-derive it from scratch.

## Date

2026-05-07

## Why this exists

In a multi-session debugging thread (rustclaw, 2026-05-07) the assistant repeatedly mis-framed the relationship between these three subsystems — flipping between "KC is a wiki generator", "KC is the semantic-consolidation layer", "KC duplicates synthesis", and "KC should be deprecated". Each flip was caused by reading one module and generalising before mapping the rest of `engramai/src/`.

The verified ground truth is below. The conclusion: all three exist on purpose, each solves a different problem, and the *real* open question is narrower than "should we delete one".

## The three subsystems

### 1. `models/consolidation.rs` — Systems consolidation (synaptic-strength layer)

- **What**: Murre & Chessa ODE applied per memory; transfers memory strength between layers (sensory → short-term → episodic → semantic) over time.
- **Granularity**: One memory at a time. Does not produce new memories.
- **Effect**: Adjusts `strength` and `layer` fields on existing memory rows.
- **Trigger**: `Memory::sleep_cycle()` Phase 1 (`consolidate_namespace`).
- **Default**: Always on. rustclaw runs `sleep_cycle(7.0, None)` every ~6 h.
- **Biological analogue**: hippocampus → neocortex strength transfer (Squire & Alvarez 1995).

### 2. `synthesis/` — Semantic synthesis (insight-memory layer)

- **What**: Clusters related memories using **4-signal edge weights** (Hebbian co-activation + entity Jaccard + embedding cosine + temporal proximity), then writes a new "insight" memory back into the same `memories` table with `meta.is_synthesis = true` (a JSON field inside `meta`, not a top-level column; see `synthesis/cluster.rs:777`).
- **Granularity**: Cross-memory. Produces *new* memory rows.
- **Output**: A higher-level memory whose `entities` and `embedding` summarise the cluster; the original member memories remain.
- **Trigger**: `Memory::sleep_cycle()` Phase 2 (`synthesize`), gated by `synthesis_settings.is_some_and(|s| s.enabled)`.
- **Default in `engramai`**: **off** — every `Memory` constructor sets `synthesis_settings: None` (see `memory.rs:961, 1030, 1097, 1169`). Caller must opt in.
- **Default in rustclaw**: **on** — `rustclaw/src/memory.rs:233-236` explicitly calls `engram.set_synthesis_settings({ enabled: true, max_llm_calls_per_run: 3 })`.
- **Clusterer**: drives `infomap_rs::Infomap` directly (`synthesis/cluster.rs:17`); signal computation is inline (`compute_pairwise_signals` at line 26, `compute_composite_score` at line 85). **Does not** go through `clustering.rs`'s `EdgeWeightStrategy` trait. *Note*: this is a regression — the unified path was shipped 2026-04-19 (`2108e67`) but accidentally reverted 2026-04-22 by the ISS-023 monorepo sync. See open question #2.
- **Biological analogue**: complementary learning systems — episodic experience → abstract concept (Tulving 1972 / McClelland 1995).

### 3. `knowledge_compile/` — Knowledge Compiler v0.3 (graph-node layer)

- **What**: Selects high-importance candidate memories incrementally (watermark-based), clusters them, summarises each cluster with an LLM, and persists the result as a `KnowledgeTopic` graph node (a row in `knowledge_topics`, linked to `Entity::Topic` UUIDs in the v0.3 graph).
- **Granularity**: Cross-memory. Produces *graph nodes*, not memory rows.
- **Output**: `KnowledgeTopic { title, summary, embedding, source_memories, contributing_entities, cluster_weights, ... }` — addressable by graph traversal in the v0.3 retrieval pipeline.
- **Trigger**: `Memory::compile_knowledge(namespace)`. **Not** called by `sleep_cycle`. Manual or harness-driven only.
- **Default**: Off in rustclaw. Off in `engram-bench` until rustclaw ISS-106 added a manual call inside the LoCoMo driver.
- **Clusterer**: `EmbeddingInfomapClusterer` calls `cluster_with_infomap` from the shared `clustering.rs` engine, with a local `CosineEdges` strategy (cosine-only edge weights). Affect-bias plumbing exists but is intentionally inert until `task:retr-impl-affective` lands (see `knowledge_compile/clusterer.rs` module doc).
- **Special features**:
  - Watermark (`since last run`) → incremental, designed for repeated invocation.
  - Supersession check (new topic replaces old when embeddings are close enough) → topics evolve, not append-only.
  - `graph_pipeline_runs` ledger + `graph_extraction_failures` table → first-class operational pipeline.

## What `sleep_cycle` actually runs

Verified at `engramai/src/memory.rs:6183`:

```
Memory::sleep_cycle(days, namespace):
    Phase 1: consolidate_namespace        ── per-memory ODE
    Phase 2: synthesize (if enabled)      ── 4-signal cluster → insight memory
    Phase 3: check_decay_and_flag
    Phase 4: forget_bulk
    Phase 5: rebalance
```

**`sleep_cycle` does not call `compile_knowledge`.** They are separate entrypoints, separate pipelines, separate output tables.

## Why three, not one

| Question                            | consolidation       | synthesis                   | KC                                   |
|-------------------------------------|---------------------|-----------------------------|--------------------------------------|
| Operates on...                      | individual memory   | clusters of memories        | clusters of memories                 |
| Produces...                         | strength/layer edit | new memory (`is_synthesis`) | graph node (`KnowledgeTopic`)        |
| Consumed by...                      | recall ranking      | recall ranking              | v0.3 graph traversal / retrieval     |
| Runs incrementally?                 | yes (per cycle)     | yes (per cycle)             | yes (watermark)                      |
| Default in `sleep_cycle`?           | yes                 | yes                         | **no**                               |
| Edge-weight signals                 | n/a                 | 4-signal                    | cosine-only (v1, by design)          |

The three are not parallel implementations of the same idea. Each lives in a different layer of the read path:

- **consolidation** changes how strongly an *existing* memory is recalled.
- **synthesis** writes *new* abstract memories that compete with originals at recall time.
- **KC** writes *graph nodes* that the v0.3 retrieval pipeline can traverse independently of the memory table.

## How the LoCoMo regression confused things (rustclaw ISS-106)

Sequence of events:

1. LoCoMo harness in `engram-bench` ingested conversations and queried, but never called `compile_knowledge`. KC's `knowledge_topics` table stayed empty.
2. The v0.3 retrieval planner has an "Abstract" sub-plan that consults `knowledge_topics`. With the table empty, it always downgraded — but baseline J-score was unaffected because the other sub-plans carried the load.
3. rustclaw ISS-106 patched the LoCoMo driver to call `compile_knowledge("default")` after ingest, so the Abstract sub-plan would have data to consume.
4. J-score *dropped* after the patch (RUN-0025/26). The cause: in single-conversation LoCoMo data, every memory shares context, so cosine-only edges over-cluster into one giant component. KC produced one super-topic with `contributing_entities=0` and `source_memories=441/441`, which the Abstract sub-plan then over-weighted, displacing better candidates from other sub-plans.

What this is *not* evidence of:

- It is not evidence that KC's architecture is wrong. K1 (candidate selection), K3 (synthesis + supersession + graph write) are architecturally sound.
- It is not evidence that KC and synthesis are duplicates. They wrote into different tables and synthesis was never tested on LoCoMo either.
- It is not evidence that consolidation is missing. Consolidation runs in `sleep_cycle` Phase 1 and is unrelated.

What it *is* evidence of:

- KC's K2 (cosine-only clusterer) degenerates on single-domain corpora. Tracked in engram ISS-109.
- LoCoMo is the first realistic input KC has ever seen. Earlier unit tests had synthetic multi-topic fixtures.
- The Abstract sub-plan needs a guard against single-topic-eats-everything outcomes (separate from fixing the clusterer).

## Open questions (real ones)

1. **Should synthesis and KC merge?** Both cluster memories. KC already uses the shared `clustering.rs` engine; synthesis still drives `infomap_rs` directly. They differ in *output target* (memory row vs graph node) and *signal set* (4-signal vs cosine-only). Merging would mean: one clusterer call, two persisters. This is a design question, not a bug — needs review of the v0.3 retrieval contract before answering.
2. ~~**Did the 2026-04-18 unified-clustering ADR actually ship for synthesis?**~~ **Resolved (git-archaeology 2026-05-08):** It did, briefly, then was accidentally reverted.

   Timeline:
   - **2026-04-19** `2108e67` ("refactor: improve discovery pipeline, unified clustering, storage enhancements") — synthesis migrated to `clustering::{InfomapClusterer, MultiSignal}`. Module doc explicitly said "This module is an adapter: it loads signal data from Storage, builds `ClusterNode`s, runs the shared clusterer, and converts results back to synthesis-specific `MemoryCluster` structs."
   - **2026-04-19** `5e0d4ba` — ADR document committed describing the (now-shipped) state.
   - **2026-04-22** `3132194` ("feat: consolidate engram-ai-rust into monorepo (ISS-023)") — bulk one-shot sync from the old `engram-ai-rust` repo to `crates/engramai/src/`. The old repo never had the unified-clustering refactor, so the sync **silently reverted `synthesis/cluster.rs`** to the direct-Infomap version. The ADR document was not touched, so it kept describing the merged state.

   So the ADR is not aspirational — it describes work that was done, then lost during a directional-sync regression. Re-applying the refactor is straightforward (the diff still exists at commit `2108e67`). Whether it's worth re-doing depends on Q1 above (full synthesis ↔ KC merge vs just re-unifying the clusterer).

   This regression is also a process finding: the ISS-023 sync used "old repo wins" semantics on every file, with no diff review for files that had diverged in the monorepo. Worth tracking as a separate issue so future sync events don't repeat it.
3. **Should `sleep_cycle` call `compile_knowledge`?** Currently it doesn't. If KC is meant to keep up with new memories, scheduling it inside the sleep cycle is the natural fit. Cost: per-cluster LLM call. Benefit: graph stays warm. Decision pending.
4. **K2 fix scope.** Drop-in fixes (raise similarity threshold, require min entity-overlap, cap max cluster size) vs algorithm change (HDBSCAN, or share synthesis's 4-signal strategy). Tracked in engram ISS-109 / rustclaw ISS-107.
5. **Retrieval-side guard.** Independent of K2: when KC returns one topic that covers >X% of the corpus, the Abstract sub-plan should down-weight or skip it. This is a planner-side defence, not a KC fix.

## What earlier discussions got wrong

For the record, so the same wrong turns aren't taken again:

- ❌ "engram has no semantic abstraction" — wrong; synthesis does it, runs by default.
- ❌ "KC is a wiki generator" — wrong; KC is a graph-aware semantic-consolidation pipeline with watermark + supersession.
- ❌ "KC should replace synthesis" — wrong; they target different output layers.
- ❌ "synthesis should replace KC" — wrong, same reason.
- ❌ "KC's regression on LoCoMo proves KC's architecture is wrong" — wrong; it proves K2 degenerates on single-domain corpora.
- ❌ "deprecate KC" — wrong; KC's K1 and K3 are sound, only K2 needs work.

## Source-of-truth pointers

- `crates/engramai/src/memory.rs:6183` — `sleep_cycle` definition.
- `crates/engramai/src/memory.rs:6552` — `compile_knowledge` definition (independent entrypoint).
- `crates/engramai/src/synthesis/engine.rs:711` — `discover_clusters` (4-signal).
- `crates/engramai/src/knowledge_compile/clusterer.rs` — KC's cosine-only `EmbeddingInfomapClusterer` + design-rationale doc comments.
- `crates/engramai/src/clustering.rs` — shared Infomap engine + `EdgeWeightStrategy` trait.
- `crates/engramai/src/models/consolidation.rs` — Murre & Chessa ODE.
- `docs/adr-unified-clustering.md` — 2026-04-18 ADR for the shared engine.
- `docs/DESIGN-v0.3.md` §5bis — KC v0.3 design (K1/K2/K3).
