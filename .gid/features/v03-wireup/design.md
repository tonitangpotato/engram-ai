# v0.3 Wire-up Design

**Status**: SUPERSEDED by `.gid/features/v04-unified-substrate/design.md` (2026-05-14)
**Original status (2026-05-12)**: DRAFT — pending potato review
**Author**: claude (rustclaw session 2026-05-12)
**Scope**: Identify the remaining work to make engram's v0.3 graph engine the actual substrate, not a side-car.
**Prerequisite read**: `consolidation-autopilot-DRAFT.md`, `engramai/src/retrieval/api.rs`, `engramai/src/resolution/pipeline.rs`

---

## Supersession note (added 2026-05-14 via T42 of v04-unified-substrate)

G1–G6 of this document are rewritten in `v04-unified-substrate/design.md` to target the unified schema (`nodes` + `edges` + `nodes_fts` + `node_embeddings`) directly, rather than through the intermediate v0.3 schema (`graph_entities` + `graph_edges`).

Phase A–D of v0.4 (schema additive → dual-write → backfill → read-switch) are complete as of 2026-05-14; see `v04-unified-substrate/design.md §8` for the task ledger.

The original v0.3 wire-up plan below is preserved for historical context but should not be used to guide new implementation work. If a v0.3-era reference points here, the substantive content has moved into `v04-unified-substrate/` — refer there.

---

## 1. Background — what v0.3 actually is

v0.3 is **not** "another graph layer next to v0.2". It is a real graph engine
that v0.2's evolved schema cannot become:

| Aspect | v0.2 (schema-in-SQL) | v0.3 (graph engine) |
|---|---|---|
| Entity identity | name exact match | LLM-resolved canonical + alias table |
| Edge predicate | free-form `relation_type` string | `CanonicalPredicate` registry, enforced |
| Time model | `created_at` only (ingest-time) | bi-temporal (`valid_from/to` × `recorded_at`) |
| Supersession | DELETE/UPDATE in place | append-only with `valid_to` markers |
| Provenance | none | per-edge `pipeline_run_id` + `source_memory_id` + stage trace |
| Affect | none | `SomaticFingerprint` snapshot on each edge |
| Atomicity | per-INSERT | `GraphDelta` BLAKE3-hashed, atomic apply |
| Audit | none | append-only `pipeline_runs / resolution_traces / extraction_failures` |
| Ingest model | inline sync | async `JobQueue` + `WorkerPool` |
| Topic (L5) | KC single-signal cluster | `KnowledgeTopic` is first-class graph node |

Several thousand lines of v0.3 code are in tree under
`engramai/src/{graph,resolution,retrieval}/`. The work is mostly **wire-up**,
not new code.

---

## 2. Verified current state (2026-05-12)

### 2.1 Read path — LIVE

- `Memory::graph_query(query) → GraphQueryResponse` at `retrieval/api.rs:399`
- `Memory::graph_query_locked()` at `retrieval/api.rs:572` (locked fusion config; delegates to `graph_query`)
- Pipeline: `HeuristicClassifier → PlanKind/Intent → with_graph_read(|g| execute_plan(g, ctx, adapters)) → fuse_and_rank`
- Adapters (`retrieval/adapters/`):
  - `GraphEntityResolver`
  - `StorageEpisodicStore`
  - `HybridSeedRecaller`
  - `GraphTopicSearcher`
  - `HybridAffectiveSeedRecaller`
- Plans (`retrieval/plans/`): `episodic`, `factual`, `associative`, `bitemporal`, `affective`, `abstract_l5`, `hybrid`

### 2.2 Write path — PARTIALLY wired

- `Memory::store_raw` calls `enqueue_pipeline_job(memory_id)` at three sites
  (`memory.rs:3012, 3164, 3289`)
- `enqueue_pipeline_job` (`memory.rs:809`) only runs when
  `self.job_queue.is_some()`. If `None`, silently skips (per GUARD-1).
- `Memory::with_pipeline_pool` (`memory.rs:299`) installs:
  - `BoundedJobQueue`
  - `WorkerPool` running `ResolutionPipeline::process`
  - `SqliteGraphStore`
  - Triple extractor
- `Memory::with_graph_store` (`memory.rs:486`) installs only the read-side
  `SqliteGraphStore` — sufficient for `graph_query` but not for ingest to
  populate `graph_*` tables.

### 2.3 Callers — what is and is not wired

| Caller | `Memory::new` | `with_graph_store` | `with_pipeline_pool` | Consequence |
|---|---|---|---|---|
| `rustclaw` (production) | ✅ | ❌ | ❌ | `graph_query` not used; only `recall_from_namespace`. `graph_*` tables stay empty. |
| `engram-bench` harness (`fresh_in_memory_db`, `harness/mod.rs:554`) | ✅ | ✅ | ❌ | `graph_query` works but plans hit empty `graph_*` tables and fall back to v0.2 adapters. |
| `examples/suite3.rs:386` | ✅ | ✅ | ❌ | Same as bench harness. |
| Tests touching ingest pipeline | ✅ | ✅ | ✅ (some) | Only places where the full v0.3 write path actually runs. |

### 2.4 Production DB measurements (`/Users/potato/rustclaw/engram-memory.db`)

```
memories                24613    ← v0.2 written
memory_embeddings       24462    ← v0.2 written
memories_fts            24613    ← v0.2 written
entities                 2299    ← v0.2 written
entity_relations         6475    ← v0.2 written
memory_entities          9207    ← v0.2 written
hebbian_links           43625    ← v0.2 written

graph_entities              0    ← v0.3 schema present, no writes
graph_edges                 0    ← v0.3 schema present, no writes
graph_links                 0    ← v0.3 schema present, no writes
graph_memory_entity_mentions 0   ← v0.3 schema present, no writes
graph_entity_aliases        0    ← v0.3 schema present, no writes
graph_predicates            0    ← v0.3 schema present, no writes
graph_pipeline_runs         0    ← v0.3 schema present, no writes
graph_resolution_traces     0    ← v0.3 schema present, no writes
graph_applied_deltas        0    ← v0.3 schema present, no writes
graph_extraction_failures   0    ← v0.3 schema present, no writes
```

The read path runs against the empty side, so plans that route to v0.3
adapters degrade silently to v0.2 sources via fallback adapters.

### 2.5 Why "JOIN looks hacky" — the diagnosis was wrong

v0.2's SQL JOINs are correct relational schema. What is hacky is the
**N+1 query pattern in `entity_recall` (`memory.rs:4205`)**: walking the
"graph" by issuing one SELECT per hop, with score aggregation in Rust.
That pattern exists because there is no graph query planner; the SQL
engine is being used as a KV store. **v0.3 changes this** — plan
adapters take a single `&dyn GraphRead` handle inside `with_graph_read`
and execute multi-hop traversals with the planner doing the work.

---

## 3. Goal of this wire-up

Make every `Memory` instance in production behave the way the bench
harness already configures it — and one step further: also install the
pipeline pool so `graph_*` tables actually get populated. After that:

1. Live writes go through `ResolutionPipeline` → bi-temporal edges
   land in `graph_edges` with provenance.
2. Live reads go through `graph_query` (not `recall_from_namespace`)
   and hit a populated graph.
3. v0.2 entity/edge tables become a backfill source and a
   compatibility surface, scheduled for retirement.

---

## 4. Gap list (concrete, executable)

### G1 — Production callers don't install graph store or pipeline pool

**Where**: `rustclaw/src/memory_init.rs` (or wherever `Memory::new` is
called for the cognitive memory) — needs verification of exact location.

**Change**:
```rust
let mem = Memory::new(&db_path, Some(config))?
    .with_graph_store(&graph_db_path)?
    .with_pipeline_pool(&graph_db_path, triple_extractor, resolution_config)?;
```

**Decisions required**:
- Co-located graph DB (same file as `memories`) or separate file?
  Bench harness chose separate file (`harness/mod.rs:531-534`); FK
  handling already supports both via `with_pipeline_pool` (see
  `memory.rs:319-369`). Recommend **separate file** to match the
  v0.3 post-migration design.
- Which `TripleExtractor` impl? `RealTripleExtractor` (LLM-driven) vs
  `NullTripleExtractor` (stub). Production needs the real one;
  embeds Anthropic API cost per ingest.
- `ResolutionConfig` defaults — currently spread across
  `resolution/config.rs`. Need a `production_defaults()` constructor.

**Effort**: ~half-day if `RealTripleExtractor` already exists; longer
if we need to write the LLM client wiring.

### G2 — `recall_from_namespace` does not dispatch to `graph_query`

**Where**: `engramai/src/memory.rs:3812` (`recall_from_namespace`).

**Problem**: rustclaw and any external caller using the public `recall`
API never touches v0.3. They get the 7-channel in-Rust fusion path
(embedding + FTS + entity_recall + hebbian + temporal + somatic + ACT-R).

**Options**:

**(a) Replace `recall_from_namespace` body with `graph_query` call**.
Cleanest but breaks return type (`Vec<RecallResult>` vs
`GraphQueryResponse`) — needs an adapter.

**(b) Add a feature flag** (`MemoryConfig::use_v03_retrieval: bool`,
default false; flip to true once we've verified parity).

**(c) Keep both, route by intent**. Cheap requests stay on the 7-channel
path; structured queries (multi-hop, temporal, predicate) go to
`graph_query`. Adds complexity; probably wrong direction.

**Recommend (b)** — feature flag, default off, flip after a parity
campaign against the production DB.

**Effort**: 1 day for the flag + adapter; 2-3 days for the parity
campaign.

### G3 — `graph_*` tables are empty in production DB

**Two sub-problems**:

**G3a — Going forward**: After G1, every new ingest populates them.
No backfill needed for net-new memories.

**G3b — Historical**: 24613 existing memories never went through the
resolution pipeline. To make `graph_query` actually useful against the
production DB, we need a **backfill driver**:
- Read all `memories` rows
- For each, synthesize a `PipelineJob::initial(memory_id, episode_id)`
- Submit to a one-shot worker pool
- Let `ResolutionPipeline::run_job` extract entities + edges + persist
  to `graph_*`

Cost: 24613 memories × Anthropic call(s) per memory ≈ non-trivial.
At LLM cost ~$0.001 per memory for triple extraction → ~$25 one-shot.

**Decisions required**:
- Run backfill once, or do it lazily on-recall (cold-cache pattern)?
- Run against production DB in place, or against a copy first for
  validation?

**Effort**: 1 day to write the backfill driver (it's basically a
loop over `store_raw_existing` style). Plus actual wall-clock for
24k LLM calls (~3-6 hours at typical rate limits).

### G4 — v0.2 retirement plan

After G1 + G2 + G3 land, v0.2 tables are still being written by
`store_raw`'s pre-pipeline path (entity extraction at
`memory.rs:2690-2780`, hebbian by `association/former.rs`).

**Stages**:
1. **Dual-write** (current target after G1): every ingest writes both
   v0.2 (entities/memory_entities/hebbian) and v0.3 (graph_*). No
   reader change yet.
2. **Dual-read with v0.3 primary**: `graph_query` returns first;
   `recall_from_namespace` (the 7-channel path) is a fallback.
3. **Stop v0.2 writes**: remove `extract_entities_to_v2` call,
   stop populating `entities/memory_entities`. **Keep Hebbian**
   for now — it's orthogonal to entity/edge structure and serves
   spreading activation, which v0.3 doesn't currently replace.
4. **Drop v0.2 tables**: scheduled for v0.4; full retirement
   requires v0.3 to also subsume Hebbian (see §6).

**Decisions required**: nothing yet — stage 1 is enough to start.
Stages 2-4 are post-G3 work.

### G5 — Cortical column / multi-plan unification

Out of scope for v0.3 wire-up itself, but worth flagging: the
`retrieval/plans/*` structure (episodic, factual, associative,
bitemporal, affective, abstract_l5, hybrid) **is already** the
multi-plan-dispatch shape that the Hawkins column thesis points at.
Each plan = one column, fusion = voting. Reference frames map onto
the `Intent` enum and per-plan context.

No new abstraction needed before completing v0.3 wire-up. After
wire-up, the natural next step is enriching plan diversity (e.g.,
adding reference-frame-aware plans that share state) rather than
introducing a new "column" type.

---

## 5. Proposed execution order

Each step is doc-bounded and reversible.

1. **G1** install `with_graph_store` + `with_pipeline_pool` in
   rustclaw's `Memory::new` path. Production agent now writes v0.2
   AND v0.3 in parallel for every new memory.
   - Acceptance: `sqlite3 engram-memory.db "SELECT COUNT(*) FROM graph_edges"` grows on every new conversation.
2. **G3b** backfill driver. Run on a copy first; validate
   `graph_edges` count looks sane (~10× memory count is the
   rough target based on LoCoMo). Then run against production DB.
   - Acceptance: `graph_edges` count moves from 0 to ≥5× memory count for the consolidated namespace.
3. **G2** add `MemoryConfig::use_v03_retrieval` flag, default off.
   Wire `recall_from_namespace` to call `graph_query` when on. Run a
   parity campaign against a recorded query set.
   - Acceptance: side-by-side eval where v0.3 ≥ v0.2 on a recall-quality metric we define here.
4. **G2 flip default to on**. After parity confirmed.
   - Acceptance: rustclaw runs with v0.3 retrieval as the default for ≥1 week with no quality regression.
5. **G4 stage 2-3**. Stop writing v0.2 entity/relation rows; keep
   Hebbian; remove dead code paths.
   - Acceptance: code search confirms no `extract_entities_to_v2` call sites remain in store_raw paths.
6. **G4 stage 4**. Drop tables. v0.4 milestone.
   - Acceptance: migration applied; tests still green.

---

## 6. Open questions for potato

1. **Co-located vs separate graph DB file in production?** Bench harness uses separate. Recommendation: separate (matches post-migration v0.3 design).
2. **Which triple extractor in production?** `RealTripleExtractor` (Anthropic-backed) is required to populate `graph_edges` meaningfully. Cost per ingest is roughly one Haiku-tier call. OK to spend that on every cognitive ingest?
3. **Backfill: one-shot vs lazy?** One-shot is simpler but costs ~$25 + ~4 hours wall-clock. Lazy means cold-recall is slow until warm. Recommendation: one-shot, off-peak.
4. **Parity acceptance criterion?** What is the bar we set before flipping `use_v03_retrieval` default to on? J-score on a held-out query set? Recall@10 on a manually-curated probe set? Both?
5. **Hebbian fate?** Plan above keeps Hebbian alive after v0.2 retirement. Alternative: subsume Hebbian into `graph_edges` as a `CoActivation` predicate with a weight column. That's clean but means writing a Hebbian→graph_edges migration. Defer to v0.4?

---

## 7. Out of scope for this design

- Cortical column refactor (handled by future work after G5)
- Wiki / human-facing knowledge surface (Autopilot DRAFT's KC stage)
- Decay model changes (lifecycle.rs is independent)
- New retrieval plans (the existing 7 are sufficient for now)
- Performance tuning of `graph_query` (will be revisited post-G2 if needed)

---

## 8. Risk register

- **R1** — Backfill cost overrun. Mitigation: dry-run on first 100 memories, extrapolate, get potato approval before full run.
- **R2** — v0.3 recall quality regresses vs v0.2 on production DB (it has only been bench-tested on LoCoMo). Mitigation: G2's parity flag, gradual rollout.
- **R3** — `with_pipeline_pool` startup latency. Worker pool init is non-trivial; cold-start of rustclaw could slow. Mitigation: measure first; if >2s, lazy-init the pool on first ingest.
- **R4** — Production DB lock contention. v0.3 graph DB is separate file but shares disk. Mitigation: monitor `access_log` p99 after rollout.
- **R5** — Forgotten consumer of v0.2 tables somewhere outside `memory.rs`. Mitigation: G4 stage 3 includes a workspace-wide `grep` for `entity_relations`, `memory_entities`, `hebbian_links` before stopping v0.2 writes.

---

*End of design. Awaiting potato review of §6 open questions before
moving into implementation.*
