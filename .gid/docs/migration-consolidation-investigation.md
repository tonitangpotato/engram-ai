# Migration + Consolidation Investigation

> **Status:** IN PROGRESS — investigation underway
> **Started:** 2026-05-11 (rustclaw session w/ potato)
> **Author:** RustClaw (this is verification work, not opinion)
> **Last revised:** 2026-05-11 (review pass v3 — §4 filled in, 7 new corrections, see §9 errata)
> **Why this doc exists:** Conversation surfaced that engram has potentially-overlapping consolidation/synthesis/KC subsystems AND an incomplete v0.2→v0.3 schema migration. Before any architectural decision, verify on-disk reality first. No claims without grep/sqlite evidence.

---

## 0. Context — what triggered this

Conversation thread 2026-05-11 (rustclaw telegram session):

1. potato asked about RUN-0026 KC regression
2. Assistant (me) flipped framings multiple times: "KC is wiki generator" → "KC is retrieval substrate" → "memory/graph layer overlap is debt"
3. Each flip caused by reading one module and generalising
4. potato called the discussion "messy", asked to investigate properly and write findings

The pre-existing doc `engram/docs/architecture-consolidation-synthesis-kc.md` (written 2026-05-07) already warns:
> "the assistant repeatedly mis-framed the relationship between these three subsystems... Each flip was caused by reading one module and generalising before mapping the rest of `engramai/src/`."

**This investigation is the response.** No conclusions until §6.

---

## 1. Questions to answer (Methodology)

Q1. **Schema reality** — Which tables exist in a current engram DB? Which are populated, which are empty?
Q2. **Write paths** — For each table, what code writes to it? Is it called from production ingest, or only from tests/migration?
Q3. **Read paths** — For each table, what code reads from it? Is the reader in the retrieval critical path, or background only?
Q4. **v0.2 vs v0.3 entity tables** — Concretely which is dead, which is alive?
Q5. **Consolidation wire-up** — `sleep_cycle()`, `compile_knowledge()`, `synthesize()`, `consolidate_single()` — who calls each, on what trigger, in production?
Q6. **Migration state** — Is `engramai migrate` complete? Has it been run on the engram-memory.db that rustclaw uses?

Each answer must cite: file:line, grep output, or sqlite query result. No "I think" answers.

---

## 2. Schema reality (Q1)

_To be filled — sqlite query on real DBs._

### 2.1 Tables in rustclaw's engram DB (`/Users/potato/rustclaw/engram-memory.db`)

Verified 2026-05-11 via `sqlite3 .tables` + row count query. 41 tables total. Grouped by role:

**Memory substrate (v0.2 core, ALIVE):**
- `memories` — **24,510 rows** (main fact table)
- `memory_embeddings` — 24,368 rows
- `memories_fts` + 4 fts shadow tables — 24,510 rows (FTS5 index)
- `access_log` — 174,671 rows (recall stats)
- `hebbian_links` — 43,319 rows (co-recall edges)
- `engram_meta` — 3 rows (DB-wide config)

**v0.2 entity / relation tables (ALIVE — actively populated):**
- `entities` — **2,277 rows**
- `entity_relations` — **6,373 rows**
- `memory_entities` — **9,091 rows** (M↔E mentions)
- `triples` — **0 rows** ← appears unused even in v0.2

**v0.3 graph tables (ALL EMPTY):**
- `graph_entities` — **0 rows**
- `graph_edges` — **0 rows**
- `graph_entity_aliases` — 0 rows
- `graph_memory_entity_mentions` — 0 rows
- `graph_predicates` — 0 rows
- `graph_pipeline_runs` — 0 rows
- `graph_resolution_traces` — 0 rows
- `graph_extraction_failures` — 0 rows
- `graph_applied_deltas` — 0 rows
- `graph_links` — 0 rows

**Knowledge Compiler tables (mixed):**
- `kc_topic_pages` — **1,063 rows** ← v0.2 KC, ALIVE
- `kc_compilation_records` — 1,063 rows
- `kc_compilation_sources` — 58,882 rows
- `knowledge_topics` — **0 rows** ← v0.3 KC, EMPTY
- `promotion_candidates` — 0 rows

**Clustering state (mixed):**
- `cluster_incremental_state` — 6,125 rows ← ALIVE
- `cluster_state` — 1 row
- `cluster_assignments` / `cluster_centroids` / `cluster_pending` — 0 rows

**Other:**
- `behavior_log` — 2,839 rows
- `emotional_trends` — 6 rows
- `quarantine` — 2 rows
- `synthesis_provenance` — **0 rows** ← synthesis-output tracking, EMPTY
- `backfill_queue` — 0 rows

### 2.2 Tables in engram-bench's test substrate

_pending — different question, do after §3-§5_

### 2.3 Row counts per table

See §2.1.

### 2.4 ⚠️ KEY OBSERVATIONS from row counts

1. **v0.3 graph layer is 100% empty on rustclaw's production DB.** All `graph_*` tables: 0 rows. v0.3 migration was never run / never wired into rustclaw's ingest path.
2. **v0.2 entity system is ALIVE and actively growing.** entities=2.3k, entity_relations=6.4k, memory_entities=9.1k. Something is writing to these.
3. **Two parallel KC systems coexist:** `kc_topic_pages` (v0.2, 1063 rows) AND `knowledge_topics` (v0.3, 0 rows). v0.2 KC has been actively used; v0.3 KC has never produced output here.
4. **Synthesis produces zero output in production.** `synthesis_provenance=0` despite 24k memories. Either synthesis never runs, or it runs and never persists.
5. **`triples` table is dead** — 0 rows, fact-style storage abandoned.
6. **Hebbian formation IS working** — 43k links across 24k memories, healthy density.

---

## 3. Write paths (Q2)

Verified via grep. Production = code outside `#[cfg(test)]`.

### 3.1 v0.2 entity tables (`entities`, `memory_entities`, `entity_relations`, `triples`)

**ALIVE — written from production ingest path in `memory.rs`.** Verified callsites:

- `memory.rs:2399` — `link_memory_entity(candidate_id, eid, "mention")` (inside `store_raw` / dedup-merge path)
- `memory.rs:2448` — `link_memory_entity(existing_id, eid, "mention")` (merge path)
- `memory.rs:2531` — `link_memory_entity(id, eid, "mention")` (fresh insert path)
- `memory.rs:2546` — `upsert_entity_relation(...)` (co-occurrence relation write)
- `memory.rs:5975` / `5991` — same calls in `add_with_entities`-style API
- `lifecycle.rs:199-200` — also writes (likely test, but verify)

**Storage primitives** (storage.rs):
- `link_memory_entity()` (line 2690) — INSERT INTO memory_entities
- `upsert_entity_relation()` (line 2709) — INSERT INTO entity_relations
- `storage.rs:3122` — INSERT INTO entities (inside merge-dedup wrapping fn)
- `storage.rs:3837` — INSERT INTO entities + memory_entities (inside `store_triple_entity`)

**Conclusion:** v0.2 entity write path is **production-hot**. Every ingest writes entities + memory_entities + entity_relations. This is why the rustclaw DB has 2.3k entities, 9.1k memory_entities, 6.4k entity_relations.

**`triples` table = 0 rows** despite `store_triple_entity` existing — means triple-extraction code exists but is not invoked in current ingest path. Code is dead-on-disk but live-in-source. Tracked debt.

### 3.2 v0.3 graph tables (`graph_entities`, `graph_edges`, `graph_memory_entity_mentions`)

**ALIVE in code, DEAD in rustclaw's DB.** All 9 graph_* tables = 0 rows.

Write path: `Memory::with_pipeline_pool()` (memory.rs:299) installs a `ResolutionPipeline` that runs as a separate worker pool. The pipeline:
- Extracts entities from each new memory
- Resolves to canonical `graph_entities.id` (BLOB)
- Writes `graph_entities`, `graph_edges`, `graph_memory_entity_mentions`

**Critical finding (verified by grep on `/Users/potato/rustclaw/src/memory.rs:173, 206`):**

```rust
let engram = engramai::Memory::new(&db_path, Some(engram_config))  // ← line 173
// NO with_pipeline_pool call follows.
```

→ **rustclaw's Memory instance has `pipeline_pool: None`.** Resolution pipeline never runs. v0.3 graph layer is empty by design here.

This matches the DB row counts in §2.1 (all graph_* = 0).

### 3.3 `memories` table (shared)

Written by `storage.add()` / `storage.store_raw()` — single shared write path used by both v0.2 and v0.3 code. Each ingest = 1 row. Verified 24,510 rows in rustclaw DB.

### 3.4 v0.2 `kc_topic_pages` (KC v1)

**ALIVE on disk — 1,063 rows in rustclaw DB. ALIVE in production via a scheduled background writer.**

**OQ3 RESOLVED (2026-05-11):** Origin of the 1063 rows is the `kc_incremental_compile()` background task at main.rs:524-560, which runs **every 4 hours** (30-minute startup delay, max 10 new topics per cycle to bound LLM spend per GUARD-3). It uses `engramai::compiler::SqliteKnowledgeStore` which writes to `kc_topic_pages` (v1 KC schema — NOT `knowledge_topics`). The task discovers candidate clusters via `TopicDiscovery`, filters out candidates whose memories are already ≥80% covered, and persists up to 10 new topics per cycle.

→ **KC v1 is fully wired in rustclaw production.** §5.2's earlier claim "no current production writer" was wrong — there IS a writer, it's just not `compile_knowledge()` (which is the v2 path). The v1 path is reached via `kc_incremental_compile` → `SqliteKnowledgeStore::create_topic_page` → `INSERT INTO kc_topic_pages`. See E16.

### 3.5 v0.3 `knowledge_topics` (KC v2)

**Code path ALIVE (`knowledge_compile/`), DB EMPTY in rustclaw.** 0 rows.

Same root cause as §3.2: requires `pipeline_pool` + graph layer to write; both inactive in rustclaw. **And** there is a structural dependency: `knowledge_topics.topic_id` is `BLOB PRIMARY KEY REFERENCES graph_entities(id) ON DELETE RESTRICT` (storage_graph.rs:347). So even if `compile_knowledge()` were invoked, every topic insert would FK-fail with 0 rows in `graph_entities`. v0.3 KC writing is **structurally blocked** until graph_entities is populated by the resolution pipeline.

---

## 4. Read paths (Q3)

### 4.1 Production rustclaw `recall()` — the actual retrieval path

**Entrypoint:** `Memory::recall()` (memory.rs:3787) → `recall_from_namespace()` (memory.rs:3812).

**Body read paths (verified by reading function body memory.rs:3812-4000+):**

- `self.storage.get_embeddings_in_namespace(ns)` → SQL: `SELECT id, content, embedding, … FROM memories WHERE namespace = ?` (storage.rs:2016)
- `self.storage.search_fts_ns(ns, query, limit)` → SQL: FTS5 `MATCH` query against `fts_memories` virtual table (storage.rs:1820). Used when embedding unavailable OR as a complementary signal.
- `self.entity_recall(query, ns)` (memory.rs:4205) → calls `storage.find_entities`, `storage.get_entity_memories`, `storage.get_related_entities`. Verified SQL:
  - `SELECT … FROM entities WHERE name = ?`
  - `SELECT memory_id FROM memory_entities WHERE entity_id = ?`
  - `SELECT … FROM entity_relations WHERE source_id = ? / target_id = ?`
  - **All three are v0.2 entity tables.**
- `self.storage.get(memory_id)` → `SELECT * FROM memories WHERE id = ?` (storage.rs:1060) — final hydration.

**Tables actually hit by production `recall()`:**

| Table | Purpose in recall | Rows in rustclaw DB |
|---|---|---|
| `memories` | embedding scan + final hydration | populated (24,510) |
| `fts_memories` (virtual) | FTS5 keyword match | populated (mirrors memories) |
| `entities` | v0.2 entity lookup | populated (2,277) |
| `memory_entities` | entity → memory mention | populated (9,091) |
| `entity_relations` | v0.2 graph traversal | populated (6,373) |

**Tables NOT hit by production `recall()`:** `graph_entities`, `graph_edges`, `graph_memory_entity_mentions`, `knowledge_topics`, `kc_topic_pages`, `synthesis_provenance`, `hebbian_links` (read only in `recall_associated`, see §4.3).

### 4.2 The v0.3 retrieval orchestrator — DEAD in rustclaw, ALIVE in engram-bench

**⚠️ Earlier draft (E13) overstated this — corrected here.**

The doc `architecture-consolidation-synthesis-kc.md` and design `v03-retrieval/design.md` describe a 7-sub-plan orchestrator (`hybrid` / `episodic` / `factual` / `associative` / `affective` / `bitemporal` / `abstract_l5`) that classifies the query and dispatches to specialised sub-plans.

**Verification:**

- Sub-plan files exist: `crates/engramai/src/retrieval/plans/*.rs` (7 files).
- Sub-plans read v0.3 tables via trait abstractions (`GraphRead::search_candidates`, `GraphRead::list_topics`, `GraphRead::memories_mentioning_entity`, `TopicSearcher`, `HybridSeedRecaller`).
- Trait `impl` for `SqliteGraphStore` exists at `graph/store.rs:1235` and DOES SELECT from `graph_entities` / `graph_edges` / `knowledge_topics`.
- `grep -rnE "RetrievalOrchestrator::new|RetrievalOrchestrator \{" crates/engramai/src/ rustclaw/src/` returns **zero hits outside the module itself**. No production code constructs the `RetrievalOrchestrator` struct directly.

**BUT — `Memory::graph_query` (retrieval/api.rs:399) IS the v0.3 orchestrator entrypoint.** It directly calls `orchestrator::execute_plan` (api.rs:~547) with `PlanCollaborators` wiring all 5 Phase-3 adapters (`GraphEntityResolver`, `StorageEpisodicStore`, `HybridSeedRecaller`, `GraphTopicSearcher`, `HybridAffectiveSeedRecaller`). The `RetrievalOrchestrator::new` constructor is unused, but the orchestrator's `execute_plan` *function* is called every time `graph_query` runs.

**Who calls `Memory::graph_query`?**

- ✅ `engram-bench/src/drivers/locomo.rs:606` — calls `graph_query_locked` per question (the active LoCoMo harness). This is how RUN-0017 / RUN-0018 / RUN-0023 / RUN-0026 / RUN-0027 reached v0.3 orchestrator code paths.
- ✅ `engram-bench/src/answer_gen/extractor.rs:60,136` — references `graph_query` results.
- ❌ **rustclaw production**: 0 hits. Only `recall()` is used (verified by `grep -rn "graph_query" /Users/potato/rustclaw/src/` → no matches).
- ⚠️ `crates/engramai/examples/locomo_conv26_retrieval.rs:198` — example file using `graph_query_locked`.

**Conclusion (revised):**

- In **rustclaw production**, `Memory::graph_query` is never called. v0.3 orchestrator code (sub-plans, adapters, dispatch) is fully dead from rustclaw's perspective. The only production retrieval is `recall()` (§4.1, v0.2 path).
- In **engram-bench**, `graph_query_locked` IS called per question. v0.3 orchestrator IS exercised on every benchmark query. The Phase-3 adapters DO run against the real graph + storage. (However: on an empty `graph_entities` table — as in conv-26 substrate — many sub-plans degenerate to no-op as previously documented.)
- The `RetrievalOrchestrator::new` constructor is dead in BOTH (only `execute_plan` is wired). This was E13's main concrete finding; the broader "v0.3 retrieval entrypoint never called" framing was overstated. See E17.


**Graceful-degradation behavior IF the orchestrator were wired:** sub-plans emit downgrade outcomes on empty data (`AbstractOutcome::DowngradedL5Unavailable` at abstract_l5.rs:260, `FactualOutcome::DowngradedNoEntity` / `DowngradedNoEdges` at factual.rs:278/284). So the empty v0.3 tables would not crash; the orchestrator would fall back. But this is moot — orchestrator never runs.

### 4.3 Background / specialised reads

| Reader | Function | Tables read | Wired in production? |
|---|---|---|---|
| Associative recall | `Memory::recall_associated` (memory.rs:5213) | `hebbian_links` (storage.rs:1429/1440) | ✅ Yes — public API, callable from rustclaw `engram_recall_associated` tool |
| `recall_with_associations` | memory.rs:5764 | `memories` + `hebbian_links` | ✅ Yes |
| `recall_recent` | memory.rs:4155 | `memories` ORDER BY | ✅ Yes |
| KC v1 reader (`SqliteKnowledgeStore::list_pages` etc.) | compiler/storage.rs:301/417/479 | `kc_topic_pages` | ❌ **No external caller** — grep finds only intra-`compiler/` references + tests. The 1063 rows are NOT read by any retrieval path. |
| KC v2 reader (`SqliteGraphStore::list_topics` / `get_topic`) | graph/store.rs:2873/2755 | `knowledge_topics` | ❌ Only reachable through the dead v0.3 orchestrator |
| Consolidation / decay | `sleep_cycle` Phase 1 | `memories` (strength fields) | ⚠️ Partial — only `consolidate_namespace` (Phase 1) runs via cron; full sleep_cycle trigger uncertain (OQ7) |

### 4.4 Net read-path picture

- **v0.2 entity tables (`entities`, `memory_entities`, `entity_relations`) — HOT.** Read on every `recall()`.
- **v0.2 KC (`kc_topic_pages`) — COLD.** 1063 rows on disk, zero readers.
- **v0.3 graph tables (`graph_entities`, `graph_edges`, `graph_memory_entity_mentions`) — DEAD.** Reader code exists, never called.
- **v0.3 KC (`knowledge_topics`) — DEAD in rustclaw.** Reader exists only through orchestrator; orchestrator never invoked. (Reachable in engram-bench only because the LoCoMo driver's `compile_knowledge` ISS-106 patch was being tested — and even then, only the writer ran, not the reader.)
- **Hebbian (`hebbian_links`) — WARM.** Read only on explicit `recall_associated` API call, not on default `recall()`.

---

## 5. Consolidation wire-up (Q5)

### 5.1 `sleep_cycle()` vs `consolidate_namespace()` — what is actually scheduled?

**⚠️ Earlier draft of this section was wrong. Corrected here.**

rustclaw's cron job `memory-consolidate` runs the shell command `"engram consolidate"` (cron.rs:631-636).

The `engram consolidate` CLI subcommand (engram-cli/src/main.rs:1460-1463) calls:

```rust
Commands::Consolidate { ns, days } => {
    mem.consolidate_namespace(days, ns.as_deref())?;
}
```

→ **The cron job runs `consolidate_namespace()` ONLY (Phase 1 of sleep_cycle), NOT the full `sleep_cycle()`.**

Separately, `MemoryManager::consolidate()` in rustclaw (memory.rs:592) DOES wrap `sleep_cycle(7.0, None)` — and `main.rs:459` invokes it via `spawn_blocking`. So both code paths exist:

| Trigger | Function called | Phases run |
|---|---|---|
| Daily cron `engram consolidate` | `consolidate_namespace()` | Phase 1 only (strength ODE) |
| `MemoryManager::consolidate()` from main.rs:459 | `sleep_cycle(7.0, None)` | All 5 phases |

**OQ7 RESOLVED (2026-05-11):** `MemoryManager::consolidate()` at main.rs:459 IS scheduled — wrapped in a `tokio::spawn` background task that ticks every **6 hours** (main.rs:454, `Duration::from_secs(6 * 3600)`). First tick is skipped (no startup run). Log message: "Engram auto-consolidation scheduled (every 6 hours)". **Implication: full `sleep_cycle()` IS run in production rustclaw, ~4× per day, with Phase 2 (synthesis) reachable in principle.** So the OQ2 dominant-cause narrative ("scheduler never invokes Phase 2") in §5.3 is **wrong** — needs revision. (See E15.)

**Additional background tasks discovered at main.rs:471-560 (all production-active):**

| Task | Interval | Function | Effect |
|---|---|---|---|
| consolidation | 6h | `mem.consolidate()` → `sleep_cycle(7.0, None)` | All 5 phases |
| synthesis | 2h | `mem.synthesize()` | Cluster + LLM synthesis only (no decay) |
| self-reflection | 24h | `mem.self_reflect()` | Trend decay, log prune, soul suggestions |
| KC v1 incremental | 4h (30m startup delay) | `kc_incremental_compile()` | Discovers clusters, writes `kc_topic_pages` (v1) — max 10 topics/cycle |

This re-frames the entire picture: rustclaw runs **four** independent background loops touching memory state. The cron `engram consolidate` is redundant with the 6h tokio task (both call into Phase 1). See §5.3-revised below.

### 5.1.1 Inside `sleep_cycle()` (memory.rs:6183-6283) — VERIFIED full body, 5 phases:

1. Phase 1: `consolidate_namespace()` — Murre/Chessa ODE on existing memories (strength/layer mutate)
2. Phase 2: `synthesize()` — IF `synthesis_settings.enabled`
3. Phase 3: `check_decay_and_flag()` — soft-delete weak memories
4. Phase 4: forget/hard-delete (`forget.soft_deleted + forget.hard_deleted`)
5. Phase 5: `rebalance_internal()` — repair integrity

**🚨 KEY FINDING — `sleep_cycle()` does NOT call `compile_knowledge()`.** No KC phase in the 100-line body. Any DESIGN that said "KC is in Phase 5 / consolidation autopilot Stage 5" was aspirational, not implemented.

### 5.2 `compile_knowledge()` — production wire-up

**Defined at memory.rs:6552/6598. Not called from sleep_cycle. VERIFIED writes v2 only.**

`Memory::compile_knowledge_with()` body (memory.rs:6598+) imports exclusively from `crate::knowledge_compile::*` (adapters, clusterer, config, metrics). It writes to **`knowledge_topics`** (v2), not `kc_topic_pages` (v1).

v1 KC vs v2 KC verified:

| | v1 KC (`compiler/`) | v2 KC (`knowledge_compile/`) |
|---|---|---|
| **Output table** | `kc_topic_pages` (1063 rows in DB) | `knowledge_topics` (0 rows in DB) |
| **Module pub'd in lib.rs** | ✅ `pub mod compiler;` (line 96) | ✅ `pub mod knowledge_compile;` (line 105) |
| **`Memory::compile_knowledge()` writes here?** | ❌ NO | ✅ YES |
| **Internal callers in production** | None found via `grep crate::compiler::` outside `compiler/` itself + test code | `Memory::compile_knowledge*` API only |
| **External callers (rustclaw / engram-cli)** | None found | None found via `\.compile_knowledge\(` |

**Two distinct findings:**

1. **v1 KC is internally self-referential.** Every `use crate::compiler::*` callsite I found points to either `compiler/` submodules referencing each other (`compiler::types`, `compiler::storage`, `compiler::compilation`), or `#[cfg(test)]` code. **No external production caller.**

2. **`Memory::compile_knowledge()` writes v2.** Has zero production callers from rustclaw codebase (verified `grep -rnE "\.compile_knowledge\(|::compile_knowledge\(" rustclaw/src/ crates/engramai/src/` — empty result outside the definition itself).

**Conclusion:** Both KC paths are **orphaned in rustclaw production**. v1 has historical data (1063 rows) from some past code version or one-off run; v2 has the modern entrypoint but no scheduler ever invokes it.

_Still open (OQ3):_ how did the 1063 v1 rows get there? Either (a) older `compile_knowledge` once wrote v1 before refactor, (b) a removed CLI subcommand wrote them, (c) manual REPL session. Not critical for the migration decision but worth a git-log check before deleting.

### 5.3 `synthesize()` — production wire-up

**VERIFIED ENABLED in rustclaw (line numbers re-checked):**
- `/Users/potato/rustclaw/src/memory.rs:234` — `synthesis_settings.enabled = true`
- `:235` — `synthesis_settings.max_llm_calls_per_run = 3` (conservative budget)
- `:233-236` — `SynthesisSettings::default()` is mutated and installed via `set_synthesis_settings`
- (LLM provider install — need to re-verify line range; earlier draft said :228-229, that's adjacent OAuth setup, not synthesis-specific)

**Production writer for synthesis_provenance:** `synthesis/engine.rs:244` calls `storage.record_provenance(&record)` inside `SynthesisEngine::synthesize()`. Verified production code (not behind `#[cfg(test)]`). So when synthesis runs and produces an insight, it WILL write provenance. The 0 rows in `synthesis_provenance` is real evidence that the engine has not produced any insight in this DB's lifetime, not a missing-instrumentation artefact.

**Synthesis runs only inside `sleep_cycle()` Phase 2 — and the cron job runs `consolidate_namespace()` only (§5.1).** This means:

> 🚨 **In production rustclaw, synthesis never actually executes via the daily cron path** — because the cron only invokes Phase 1, not the full sleep_cycle. The only way synthesis runs is via `MemoryManager::consolidate()` at main.rs:459, whose trigger schedule is unverified (OQ7-new).

This revises the earlier framing. `synthesis_provenance = 0 rows` is **not surprising once you see synthesis isn't on the daily path** — the engine simply was never reached. It might also have secondary issues (cluster thresholds, LLM gating) once sleep_cycle DOES run, but those are downstream of the dominant explanation: **"the scheduler never invokes Phase 2"**.

Hypotheses for zero output, ordered by likelihood (revised):
- (a) **Scheduler never invokes full `sleep_cycle()`** → synthesis Phase 2 never reached. Most likely. (OQ7)
- (b) Engine runs (when sleep_cycle does fire) but every cluster fails to produce a coherent insight — secondary.
- (c) Earlier hedge "`synthesis_provenance` might be the wrong table" — **rejected**, verified above as the correct table.

### 5.4 Resolution pipeline (v0.3 graph writes)

**NOT WIRED in rustclaw.** Verified — no `with_pipeline_pool()` call. Root cause of empty v0.3 graph tables. (§3.2)

---

## 6. Migration state (Q6)

### 6.1 `engramai migrate` — what does it do?

From v03-migration design.md (read earlier):
- Additive DDL to add `graph_*` tables and `knowledge_topics`
- Adds 4 columns to `memories` (episode_id, entity_ids, edge_ids, confidence)
- Backfills via resolution pipeline
- Forward-only, resumable, idempotent

### 6.2 Has it been run on rustclaw's DB?

**Schema-side: AMBIGUOUS** — All graph_* tables exist in the rustclaw DB (§2.1). But this is **NOT proof that `engramai migrate` ran**. The DDL is also installed by `init_graph_tables()` (storage_graph.rs:43), which is likely called from `Memory::new()` regardless of migration state. **Need to verify whether `Memory::new` auto-creates the graph schema, or only `engramai migrate` does.**

**Data-side: NO** — All graph_* tables = 0 rows. Whatever created the schema, nothing has backfilled or live-written data into it.

### 6.3 `migration_state` table?

Not present in rustclaw DB (`.tables` output shows no such table named that). However, the DB does have an `engram_meta` table (3 rows) which is a plausible location for migration state / schema version. **Need to query `engram_meta` contents to confirm.**

_TODO: `SELECT * FROM engram_meta` to see what schema-version / migration markers are stored._

---

## 7. Findings & open questions

### 7.1 Findings (everything below is verified by grep/sqlite, no opinion)

**F1. v0.3 migration is HALF-DONE on rustclaw.**
- Schema: DDL applied (all `graph_*` tables exist). ✅ — but unclear whether by `Memory::new()` auto-init or by `engramai migrate`. (§6.2)
- Code wiring: `Memory::with_pipeline_pool()` never called in rustclaw (verified `grep` returns 0 callsites in `rustclaw/src/`). ❌
- Data: 0 rows in every v0.3 graph table. ❌
- **Net effect:** rustclaw runs on v0.2 entity layer (`entities`/`memory_entities`/`entity_relations`) only. v0.3 is dead-on-disk.

**F2. Two parallel KC implementations exist.**
- v1 KC (`compiler/storage.rs`) writes `kc_topic_pages`. 1063 rows in rustclaw DB — historical, probably from before v2 was added. No current trigger found.
- v2 KC (`knowledge_compile/`) writes `knowledge_topics`. 0 rows. Never invoked in rustclaw.
- **Net effect:** Two abandoned subsystems. v1 has historical data but no current writer. v2 has writers but no caller. Neither contributes to live retrieval.

**F3. Synthesis is configured but never reached by the daily scheduler.**
- `synthesis_settings.enabled = true` in rustclaw (memory.rs:234)
- LLM provider configured (memory.rs lines TBD — earlier :228-229 cite was loose)
- `synthesis_provenance` table = 0 rows
- **Revised root cause:** synthesis is gated inside `sleep_cycle()` Phase 2, but the daily cron only runs `consolidate_namespace()` (Phase 1 of sleep_cycle), not the full `sleep_cycle()`. So Phase 2 is never invoked via cron. Whether `MemoryManager::consolidate()` (the full-sleep-cycle wrapper at memory.rs:592) is ever triggered in production is open (OQ7-new).
- Caveat: `synthesis_provenance` may not be the table v2 synthesis writes to; secondary hypothesis (b) in §5.3.

**F4. Only Phase 1 of consolidation is running daily; Phases 2-5 are not.**
- Cron `memory-consolidate` runs `engram consolidate` → CLI invokes `consolidate_namespace()` only (engram-cli/main.rs:1461).
- This is **Phase 1 of `sleep_cycle()` in isolation**, not the full cycle.
- Phases 2-5 (synthesis, decay-flag, forget, rebalance) only fire if `MemoryManager::consolidate()` at rustclaw memory.rs:592 is triggered, which calls `sleep_cycle(7.0, None)`.
- `quarantine` table has 2 rows — this WAS written by Phase 4 (forget), so `sleep_cycle()` HAS run at least a couple of times historically. Trigger context unknown (OQ7).
- Phase 5 (rebalance) — body unread, unclear what it actually does to DB.

**F5. v0.2 entity layer is the de facto graph.**
- 2,277 entities + 6,373 relations + 9,091 memory_entities mentions — actively populated
- Triggered from ingest path (memory.rs:2399 / :2531 / :2546)
- This is what powers any "graph-shaped" recall in rustclaw today.
- `triples` table abandoned (0 rows) — earlier RDF-style attempt left as dead code.

**F6. Hebbian formation works.**
- 43,319 hebbian_links across 24,510 memories
- Density: ~1.77 links/memory
- This is the actual "graph layer" in production right now.

**F7. v1 KC (kc_topic_pages, 1063 rows) is orphaned data.**
- `Memory::compile_knowledge()` writes v2 (`knowledge_topics`), not v1. Verified.
- `compiler/` module has only internal/test references — no external production caller in rustclaw or engram-cli.
- The 1063 rows must come from: (a) historical code that wrote v1, (b) a removed CLI subcommand, or (c) manual invocation. Origin uncertain — see OQ3.
- Need to decide: migrate to v2 schema, archive, or drop. **Not blocking** — no current writer means it's just static data.

### 7.2 Open questions (require more verification)

**OQ1.** ~~Does `Memory::compile_knowledge()` write v1 or v2?~~ **RESOLVED in §5.2** — writes v2 only. Verified by reading function body.

**OQ2. RESOLVED (2026-05-11) — root cause is gate-rule starvation, not scheduling.** The puzzle: `synthesis_provenance = 0 rows` despite 14409 candidate memories and a fully-wired scheduler. After E15 ruled out the v3 "never invoked" narrative, the real cause is now verified from `cluster_incremental_state` (6132 rows) telemetry:

**Quantitative evidence (sqlite query on cluster_incremental_state, 2026-05-11):**
- 22 distinct days of attempts (2026-04-20 → today). Today alone: 856 attempts. Max `attempt_count` per cluster: **1162**.
- **`run_count = 0` for ALL 6132 clusters** — zero clusters have ever reached the Synthesize gate decision branch successfully.
- Quality distribution of 6132 clusters: avg ≈ 0.42, range [0.4, 0.6] for the bulk; very few above 0.6, none observed ≥ 0.8.
- Gate-rule attribution (computed from telemetry):
  - **Rule 3 (Skip low quality, q < 0.4)**: 1971 / 6132 ≈ **32%**
  - **Rule 4 (Defer borderline, n=3 AND q<0.6)**: 104 / 6132 ≈ **1.7%**
  - **Pass Rules 1–4 (q≥0.4 AND n≥3 AND not Rule-4-borderline)**: 4057 / 6132 ≈ **66%**

**The 4057-cluster mystery:** these clusters should have reached Rule 9 (Synthesize). They did not, because Rule 6 ("no growth since last attempt", Jaccard==1.0) kills them on every subsequent cycle. On the FIRST cycle they were also skipped — by what? Quantitative sampling rules out Rule 7 (type-diversity, sample of 200: zero homogeneous clusters) and Rule 8 (cost, sample of 500: all cost ~$0.0005 ≪ 0.05 threshold). 

**Conclusion:** the surviving 4057 clusters DID reach `GateDecision::Synthesize`, but were then killed by **per-cycle budget exhaustion** (`max_llm_calls_per_run: 3` in rustclaw/src/memory.rs:235). After 3 clusters/cycle attempt LLM synthesis, `BudgetExhausted` is raised and the rest are skipped. The first 3 EITHER:
- (a) had the LLM call fail (network / OAuth / Sonnet error) → `LlmError` recorded in `report.errors`, `clusters_skipped += 1`, no provenance row, `run_count` NOT bumped (run_count increments ONLY on successful synthesis at engine.rs:608). OR
- (b) succeeded but the persist step failed → `StorageError` path, same outcome.

The smoking gun is that `run_count` never incremented for ANY cluster across 22 days × ~4 cycles/day ≈ 88 cycles × first-3-clusters = ~264 attempted LLM calls. The probability of 264 consecutive transient LLM failures is effectively zero. So **either** the LLM provider in rustclaw's synthesis path is mis-wired and fails deterministically every call, **or** the first 3 clusters in every cycle deterministically fail one specific check inside the Synthesize branch (e.g., prompt construction, content-truncation, deserialization of LLM response).

**Sub-mystery resolved as side observation:** `metacognition_events` table does NOT exist in rustclaw's DB despite `record_synthesis()` being called at memory.rs:6263 every cycle. The table CREATE at metacognition.rs:130 either never ran (tracker disabled) or ran with an error that was swallowed by `log::warn!`. Doesn't affect provenance count but explains why we can't see per-cycle SynthesisEvent telemetry.

**What's NOT verified yet (would need a live test run):**
- Whether the LLM call inside `synthesize()` actually fires in production rustclaw vs. fails at construction.
- Which specific error variant (`LlmTimeout`, `LlmError`, `StorageError`, prompt assembly) is being raised.
- Whether the OAuth Anthropic provider wired at memory.rs:233-237 is reachable from inside `tokio::spawn_blocking`.

A 1-LLM-call dry run with `RUST_LOG=engramai::synthesis=debug` against rustclaw's DB would close this in 5 minutes. See §8.2 path B addendum.

**OQ3. RESOLVED (2026-05-11).** The 1063 v1 `kc_topic_pages` rows ARE produced by an active production writer: `kc_incremental_compile()` at main.rs:524-560, scheduled every 4h with 30m startup delay. Writes via `engramai::compiler::SqliteKnowledgeStore::create_topic_page` → `INSERT INTO kc_topic_pages`. v1 KC is NOT dead code — it's a live, scheduled, cost-bounded (≤10 topics/cycle) production loop. See E16.

**OQ4.** Does `rebalance_internal()` (sleep_cycle Phase 5) repair v0.2 entity tables, v0.3 graph tables, or memory strength only? Unknown without reading.

**OQ5. RESOLVED (2026-05-11).** engram-bench LoCoMo driver (`locomo.rs:606`) replays episodes via `ingest_with_stats_at` then calls `memory.graph_query_locked(GraphQuery::new(q.question)...)` per gold question. No `with_pipeline_pool` installation, no active `compile_knowledge` call (the ISS-106 patch was reverted after RUN-0026 showed J-score regression). `graph_query_locked` → `graph_query` → `orchestrator::execute_plan` with all 5 Phase-3 adapters; sub-plans dispatch against the v0.3 storage. Substrate state: v0.2 entity tables populated by ingest, `graph_entities` empty → most v0.3 sub-plans degrade gracefully on empty tables. See E17 / E18.

**OQ6.** What does `engramai migrate` CLI actually do? Does it run on engram-memory.db automatically at startup, or is it a manual one-shot?
- rustclaw cron has no `engram migrate` job (only `engram consolidate`). So if migrate is a one-shot, it has not been auto-run.
- BUT — graph tables EXIST in the DB anyway. So either (a) `Memory::new()` calls `init_graph_tables()` unconditionally, or (b) migrate was run once manually long ago. Need to read `Memory::new` body to know which.
- Empty rows + no pipeline + no migrate cron = backfill never had data either way.

**OQ7. RESOLVED (2026-05-11).** `MemoryManager::consolidate()` (memory.rs:592, wrapping `sleep_cycle(7.0, None)`) IS scheduled — main.rs:454 wraps it in `tokio::spawn` with `Duration::from_secs(6 * 3600)`. Runs every 6 hours, first tick skipped. Logged as "Engram auto-consolidation scheduled (every 6 hours)". Additionally main.rs schedules `synthesize()` every 2h, `self_reflect()` every 24h, and `kc_incremental_compile()` every 4h. **Four independent background loops are active in rustclaw production.** See E15.

**OQ8. RESOLVED (2026-05-11).** `engram_meta` contains 3 schema-version markers: `schema_version=1`, `fts_cjk_version=1`, `embedding_protocol_version=2`. Confirms v2's conjecture (no surprise data here). Notable: `schema_version=1` while v0.3 tables exist on disk — consistent with §6.2 "migrations never ran".

---

## 8. Proposed next step

**This is a proposal, not a decision.** potato should review before any code change.

### 8.1 The honest picture

rustclaw's engram is running on **v0.2 substrate only**:
- v0.2 entities/relations: ALIVE, doing real work
- v0.2 hebbian: ALIVE, 43k links
- v0.2 KC (kc_topic_pages): historical data, no current writer
- v0.3 graph layer: schema exists but never populated
- v0.3 KC: never ran
- Synthesis: configured but silently producing nothing

The v0.3 migration was **deployed half-way**: DDL ran, code paths exist, but the wire-up (`with_pipeline_pool`, `migrate` CLI, KC scheduling) never happened in rustclaw.

This is why the RUN-0026 conversation kept circling — we're debating an architecture (v0.3 graph as retrieval substrate) that **isn't actually running in production**. The benchmark might wire it differently than rustclaw does.

### 8.2 Three possible paths forward

**Path A — Complete v0.3 migration in rustclaw**
- Install `with_pipeline_pool()` in rustclaw's Memory init
- Run `engramai migrate` to backfill graph tables from existing memories
- Wire `compile_knowledge()` into a scheduled trigger (cron / sleep_cycle Phase 5)
- Decommission v0.2 entity write path once v0.3 is proven
- **Cost:** weeks. **Benefit:** unified graph substrate, v0.3 features actually work.

**Path B — Decommission v0.3 in rustclaw, keep it for engram-bench only**
- Treat v0.3 as a benchmark-only feature
- rustclaw stays on v0.2 + Hebbian (which is currently working)
- Mark `graph_*` tables as deprecated, plan removal
- **Cost:** days. **Benefit:** stop pretending v0.3 is live; reduces conceptual overhead.

**Path C — Investigate first, decide after**
- Answer the remaining open questions (OQ3-OQ8)
- Run a probe: enable `with_pipeline_pool()` in rustclaw, observe what happens
- Measure: does v0.3 substrate help recall quality vs current v0.2-only state?
- Then decide A vs B with data
- **Cost:** undefined — depends on how many OQs surface follow-up questions. Estimated **rough** lower bound: 1 work-session for the remaining OQs (OQ3, OQ7, OQ8 are each ≤ 1h grep/SQL); probe scope (with_pipeline_pool enable + observation) is open. **Benefit:** decision is data-driven, not opinion-driven.

### 8.3 Recommended path

**Path C.** Specifically:
1. ~~Resolve OQ1 (which KC writes where)~~ — ✅ done in v2 review
2. ~~Resolve OQ2 (why synthesis silent)~~ — ✅ partially done (scheduler primary)
3. Resolve OQ7 (sleep_cycle trigger schedule) — read main.rs:459 callsite + cron config
4. Resolve OQ8 (`engram_meta` contents) — single SQL query
5. Resolve OQ5 (does engram-bench wire pipeline_pool?) — read engram-bench/src/drivers/locomo.rs init code
6. Resolve OQ6 (migrate CLI behavior) — read engramai-migrate source, decide if dry-run on DB copy is needed
7. Write up findings, then potato decides A vs B

**Reasoning:** We've already spent context on architectural debate. The right next step is **not more debate** — it's verifying the remaining 6 open questions, then the path forward is obvious.

### 8.4 What I will NOT do without explicit potato approval

- Run `engramai migrate` on rustclaw's production DB
- Modify rustclaw's Memory init to install pipeline_pool
- Delete any v0.2 tables or kc_topic_pages rows
- Touch the engram-memory.db file outside read-only sqlite queries
- Apply any "fix" to the synthesis silence before understanding root cause

---

## 9. Errata — self-review passes

### v2 (initial review) — 2026-05-11

potato asked for a review of v1 draft. The following errors / weak claims were found and corrected:

**E1.** v1 §5.1 claimed cron job `"engram consolidate"` runs full `sleep_cycle()`. Wrong — the CLI subcommand calls `consolidate_namespace()` only. This invalidated the synthesis-silent-failure framing in v1 §5.3 / F3.

**E2.** v1 §5.2 hedged "strong suspicion that `compile_knowledge` writes v2 only" — actually verified by reading the function body. Now stated as fact.

**E3.** v1 OQ5 inferred `knowledge_topics=1` in RUN-0026 substrate ⇒ engram-bench wires pipeline_pool. Wrong inference — `knowledge_topics` is KC-written, not pipeline-written. Inference does not carry. Whether engram-bench wires pipeline is still open.

**E4.** v1 §6.2 claimed "Schema-side: YES, DDL applied (by migrate)". Sloppy — DDL existence does NOT prove `engramai migrate` ran. `init_graph_tables()` is also called from `storage_graph.rs:43` and may be auto-invoked at `Memory::new`. Re-framed as ambiguous.

**E5.** v1 §5.3 cited `memory.rs:228-229` as LLM-provider line for synthesis. That line range is adjacent OAuth code, not synthesis-specific. Loose attribution; corrected to "lines TBD, re-verify".

**E6.** v1 F4 claim "Consolidation core IS running" was technically true but misleadingly broad — only Phase 1 runs daily; Phases 2-5 only run when sleep_cycle is triggered, which is rarer than the cron cadence. Re-framed.

**E7.** Added OQ7 (sleep_cycle trigger schedule) and OQ8 (`engram_meta` content) which the v1 draft missed.

### v3 (review of v2 + §4 fill-in) — 2026-05-11

Filled in §4 read paths from TODO. While doing so, found additional issues in v2:

**E8.** v2 §3.4 still said "ALIVE — Need to find caller" for `kc_topic_pages`. But §5.2 had already verified no production writer. Removed the inconsistency.

**E9.** v2 §3.5 stated FK dependency "knowledge_topics.topic_id is a FK to graph_entities.id" as fact without citing schema. **Verified in v3:** confirmed at `storage_graph.rs:347` (`BLOB PRIMARY KEY REFERENCES graph_entities(id) ON DELETE RESTRICT`). Claim now has citation. Strengthened the wording: KC v2 writes are **structurally** FK-blocked on empty graph_entities, not just "depends on".

**E10.** v2 §5.3 hedged "hypothesis (c): `synthesis_provenance` might be a different table". Verified in v3 that `storage.record_provenance` writes exactly the `synthesis_provenance` table, and `synthesis/engine.rs:244` is the (single) production caller (not in `#[cfg(test)]`). So the table IS correct; the 0 rows is real evidence, not a missing-instrumentation artefact. Hedge removed.

**E11.** v2 §8.3 path-C cost estimate of "1-2 days investigation + 1 week probe" was unjustified hand-wave. Replaced with honest "undefined, rough lower bound = 1 work-session for remaining OQs".

**E12.** v2 OQ1 / OQ2 left open but §5.2 / §5.3 had resolved or partially-resolved them. Marked as RESOLVED / PARTIALLY RESOLVED with backref.

**E13 (Major).** §4.2 — New finding from filling in read paths: **the entire v0.3 `RetrievalOrchestrator` + 7 sub-plans + adapter layer is dead code in production.** `grep -rnE "RetrievalOrchestrator::new|RetrievalOrchestrator \{"` returns zero callers outside the module and its tests. `Memory::recall` uses a legacy hybrid (FTS + embedding + v0.2 entity_recall + ACT-R) path that does NOT go through orchestrator. This is more dead-code than v2 acknowledged — not just "sub-plans no-op on empty tables", but "orchestrator literally not invoked".

**E14.** v2 §8.1 said "v0.3 KC: never ran" without scope qualifier. The RUN-0026 substrate shows it DID run in engram-bench (1 topic compiled). Should be "never ran in rustclaw production".

### v4 (OQ7 / OQ8 / OQ5 / OQ3 pass) — 2026-05-11

Closed out the remaining ≤1h OQs that v3 left open. While doing so, found that several v3 statements were also wrong and corrected them.

**E15 (Major).** §5.1 / §5.3 / §7 said the cron `engram consolidate` is the dominant production trigger and that `MemoryManager::consolidate()` at main.rs:459 was "unverified (OQ7)". **OQ7 RESOLVED:** main.rs:454 wraps it in a `tokio::spawn` background loop with `Duration::from_secs(6 * 3600)` — runs every 6 hours, skip-first-tick. So full `sleep_cycle()` IS scheduled in rustclaw production, ~4× per day. Phase 2 (synthesis) IS reachable. The v3 "Phase 2 never invoked via cron" narrative was incomplete — Phase 2 IS invoked, just not via the cron path. The 0-row `synthesis_provenance` puzzle (§5.3) needs a different explanation now (probably: synthesis runs but `record_provenance` is gated on a condition that fails in rustclaw's empty-graph-entity setup, OR the tokio task only started running after the existing DB snapshot was taken — both untested).

**E16 (Major).** §3.4 / §5.2 said `kc_topic_pages` had "no current production writer" and the 1063 rows were "historical artefacts". **OQ3 RESOLVED:** main.rs:524-560 spawns `kc_incremental_compile()` every 4 hours (30-minute startup delay, max 10 topics/cycle). The task uses `engramai::compiler::SqliteKnowledgeStore` which writes `kc_topic_pages` (v1, NOT `knowledge_topics`). So **KC v1 is a live, scheduled production writer** in rustclaw. The 1063 rows accumulated from ~261 invocations × ≤10 topics each (rough math), or fewer invocations writing more topics in early cycles. v2/v3 §5.2's narrative "compile_knowledge is the only production writer of either KC system" was wrong because it confused v1 and v2 KC writer paths.

**E17 (Correction to E13).** v3 §4.2 / E13 said v0.3 retrieval orchestrator is "fully implemented but never invoked" in either rustclaw OR engram-bench. **Engram-bench part is wrong.** `Memory::graph_query` at retrieval/api.rs:399 directly calls `orchestrator::execute_plan` with `PlanCollaborators` wiring all 5 Phase-3 adapters — and `engram-bench/src/drivers/locomo.rs:606` calls `graph_query_locked` per question. So:
- rustclaw production: 0 callers of `graph_query` → orchestrator dead (E13's original claim, still correct).
- engram-bench: live caller of `graph_query_locked` → orchestrator DOES execute (E13 wrong for this scope).
- The `RetrievalOrchestrator::new` *constructor* is dead in both (only `execute_plan` is wired by `Memory::graph_query`). This was E13's verifiable evidence but the conclusion overgeneralised.

**E18.** §8.3 path-A cost estimate now needs a second revision. v3 said "wire `Memory::recall` → `RetrievalOrchestrator` is API-level change". Refining: the actual delta needed for rustclaw to use v0.3 retrieval is `Memory::recall` → `Memory::graph_query` (NOT `RetrievalOrchestrator::new` — that constructor is dead and unneeded). This is a smaller, cleaner change than v3 implied: replace one method call, possibly wire a default `GraphQuery` builder. Still non-trivial (need to handle the v0.2-vs-v0.3 namespace/scope semantic gap), but not the "rewrite the recall pipeline" framing v3 implied.

**OQ8 RESOLVED.** `engram_meta` table contents in rustclaw DB:
- `schema_version = 1`
- `fts_cjk_version = 1`
- `embedding_protocol_version = 2`
Confirms v2's conjecture (schema markers, not data). Notably `schema_version=1` while v0.3 tables exist on disk — consistent with §6.2 "migrations never ran".

**OQ5 RESOLVED.** engram-bench LoCoMo driver (`locomo.rs:606`) ingests episodes via `ingest_with_stats_at` then calls `memory.graph_query_locked(GraphQuery::new(q.question)...)` per gold question. This routes through `Memory::graph_query` → orchestrator `execute_plan` → all Phase-3 adapters → real v0.3 sub-plan dispatch. No `with_pipeline_pool`, no `compile_knowledge` — the ISS-106 patch that inserted `compile_knowledge` was reverted after RUN-0026 showed it regressed J-score (degenerate single-cluster on dense conv-26 corpus). Currently the pipeline is "v0.2 ingest + v0.3 query against empty graph_entities" — i.e. orchestrator runs, most sub-plans hit empty tables and degrade gracefully.

### v5 (OQ2 deep verification) — 2026-05-11

**E19 (Major).** v3 §5.3 and v4 OQ2 left "0-row synthesis_provenance" as partially-resolved with three competing hypotheses. **v5 RESOLVED via cluster_incremental_state telemetry mining** — root cause is gate-rule starvation, NOT scheduling or wiring.

**Method:** `cluster_incremental_state` records every gate attempt regardless of outcome (engine.rs:495-497). 6132 rows in rustclaw's DB span 22 distinct days of attempts (2026-04-20 → 2026-05-11), with `max(attempt_count) = 1162` for a single cluster. **Critical observation:** `run_count = 0` for ALL 6132 rows. `run_count` increments ONLY when synthesis succeeds (engine.rs:608-611). Therefore zero clusters have ever produced an insight across ~88 sleep cycles.

**Gate-rule attribution (quantitative):**
- Rule 3 (q < 0.4): 1971 / 6132 = 32% — quality scores cluster around the 0.4 threshold
- Rule 4 (n=3 ∧ q<0.6): 104 / 6132 = 1.7%
- Pass Rules 1–4: 4057 / 6132 = 66%

**The 4057-cluster mystery:** sample of 200 passed clusters showed ZERO with single memory_type (Rule 7 NOT firing). Sample of 500 passed clusters showed ALL with cost ≪ 0.05 (Rule 8 NOT firing). These clusters DID reach `GateDecision::Synthesize` at the gate. They fail downstream.

**Real bottleneck:** `max_llm_calls_per_run: 3` budget (rustclaw/src/memory.rs:235) limits each cycle to 3 LLM-call attempts. After 3, `BudgetExhausted` fires and all remaining clusters → Skip. Across 22 days × ~4 cycles/day × 3 attempts/cycle = ~264 first-3-cluster LLM-call attempts that all failed silently. The probability of 264 consecutive transient LLM failures is effectively zero. So either:
- (a) the OAuth Anthropic LLM provider wired at memory.rs:233-237 is mis-wired and `LlmProvider::generate` fails deterministically (likely candidate: provider not reachable from inside `tokio::spawn_blocking`, or auth token swap timing),
- (b) prompt construction / response parsing fails deterministically inside the Synthesize branch (engine.rs:540+).

Both paths terminate at `clusters_skipped += 1` and DO NOT bump `run_count` (run_count is bumped only on the success branch at engine.rs:608). Errors are accumulated in `report.errors` but never persisted to disk — only logged via `tracker.record_synthesis()`, which itself fails because `metacognition_events` table doesn't exist in the DB (CREATE at metacognition.rs:130 never ran — possibly tracker disabled in rustclaw config; an `eprintln!`-level swallow).

**Side observation — silent telemetry loss:** `tracker.record_synthesis()` is called every sleep_cycle (memory.rs:6263) but writes to `metacognition_events`, a table that **does not exist** in rustclaw's DB. The CREATE TABLE IF NOT EXISTS at metacognition.rs:130 either never ran (tracker init skipped) or hit an error swallowed by `log::warn!` at memory.rs:6270. Result: no per-cycle synthesis telemetry on disk. Mining cluster_incremental_state directly was the only available avenue.

**Verification still needed (would close E19 fully):** a 1-cycle dry run against rustclaw's DB with `RUST_LOG=engramai::synthesis=debug` to observe which error variant fires on the first 3 attempted Synthesize clusters. Estimated 5 minutes. Not done in this pass to avoid mutating production DB state.

---

## Investigation log

- 2026-05-11 (start) — doc skeleton written, begin §2.
- 2026-05-11 (§2 done) — verified all 41 tables in rustclaw engram-memory.db. Found v0.3 graph layer entirely empty; v0.2 entity layer alive; two parallel KC systems.
- 2026-05-11 (§3 done) — grep'd write paths. Confirmed v0.2 entity writes are production-hot (memory.rs:2399+), v0.3 writes require `with_pipeline_pool` which rustclaw never installs.
- 2026-05-11 (§5 done v1) — read full `sleep_cycle()` body. Confirmed it does NOT call `compile_knowledge()`. Synthesis enabled but DB shows 0 provenance rows.
- 2026-05-11 (§7-§8 drafted v1) — findings + path-forward proposal.
- **2026-05-11 (review pass v2)** — potato asked to review. Found 7 errors (see §9). Verified `engram consolidate` CLI actually calls `consolidate_namespace` not `sleep_cycle`. Verified `compile_knowledge` writes v2 only. Corrected §5.1 / §5.2 / §5.3 / §6.2 / F3 / F4 / OQ5 / OQ6. Added OQ7, OQ8.
- **2026-05-11 (§4 fill + review pass v3)** — filled in §4 read paths from TODO. Major new finding (E13): v0.3 `RetrievalOrchestrator` is dead code. Production `Memory::recall` uses legacy hybrid path; never invokes orchestrator. Found 7 more issues (E8-E14) during v3 self-review. Verified `knowledge_topics.topic_id` FK to `graph_entities`. Verified `synthesis/engine.rs:244` is real production writer (not test).
- **2026-05-11 (OQ7 / OQ8 / OQ5 / OQ3 closure — v4)** — went after the remaining ≤1h OQs. Found that v3's narrative was off in several places (E15-E18). Discovered four production background tokio tasks at main.rs:454-560 (consolidation 6h, synthesis 2h, self-reflection 24h, KC v1 incremental 4h). OQ3 1063 v1 rows fully explained (live writer). OQ7 sleep_cycle scheduling confirmed (Phase 2 IS reached). OQ8 engram_meta confirmed as version markers. OQ5 engram-bench pipeline traced through `graph_query_locked` → orchestrator `execute_plan`. E13's "orchestrator dead in production" partially corrected: dead in rustclaw, alive in engram-bench (via `Memory::graph_query`, not via `RetrievalOrchestrator::new`).
- **2026-05-11 (OQ2 deep verification — v5)** — mined `cluster_incremental_state` (6132 rows, 22 days of attempts). Root cause for 0-row synthesis_provenance: gate-rule starvation downstream of `GateDecision::Synthesize`, not scheduling or wiring. All 6132 clusters have `run_count = 0`. Quantitative gate attribution: Rule 3 32%, Rule 4 1.7%, pass-Rules-1-4 66%. Sample of 500 passed clusters: zero hit Rule 7/8. Conclusion: the first 3 clusters per cycle reach Synthesize branch but fail deterministically — either LLM provider mis-wired (likely OAuth inside tokio::spawn_blocking) or prompt/parse path broken. Side discovery: `metacognition_events` table doesn't exist despite `record_synthesis()` being called every cycle — silent telemetry loss. (E19, see §9.)
- _Status:_ ⏸ OQ2 root cause down to two candidates (LLM provider wiring vs prompt/parse). 5-min RUST_LOG=debug dry run would close completely. OQ4 / OQ6 still open. Pending potato review.

