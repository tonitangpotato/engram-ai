# v0.5 Cognitive Substrate

**Status:** Planning (split out from v04-unified-substrate on 2026-05-14)
**Depends on:** `v04-unified-substrate` Phase A-D complete (nodes/edges/node_embeddings/nodes_fts substrate online with dual-write); ideally Phase E (legacy writes removed) before this feature lands so we're not adding cognitive ops on a half-migrated substrate.
**Supersedes scope:** v04-unified-substrate §4.11–§4.15 and §8.9–§8.13. Those sections are stubbed in the v0.4 doc with pointers here.

## 0. TL;DR

v0.4 unified the **data substrate** (every persisted thing is a `node` with `edges` between them). v0.5 unifies the **cognitive functions** that read from and write to that substrate:

- §4.11 Interoception — track somatic-style baselines + emit `anomaly_event` nodes
- §4.12 Empathy bus — agent's SOUL.md drives → `drive_alignment` / `valence_accumulator` / `action_outcome` nodes
- §4.13 Working memory — in-memory active set + metacog-triggered `wm_snapshot` persistence
- §4.14 Metacognition — reads recent `feedback_event` / `anomaly_event` nodes, emits `meta_judgment`
- §4.15 Dimensional signature — three-tier dimension layer (scalar in attributes, narrative as edges, tags as containment)

These are not new substrate primitives — they all reduce to "more `node_kind` and `edge_kind` values, more `WriteOp` variants, no new tables." The reason this is a separate feature instead of more v0.4 work is **scope discipline**: v0.4's contract is "three layers consolidated, legacy dropped." v0.5's contract is "cognitive functions implemented on top of the consolidated substrate." Conflating them stretched v0.4 to 68 tasks and blurred when "done" means done.

## 1. Why split

First-principles: v0.4 answers "does the substrate work?" v0.5 answers "what does the substrate enable?" The first question demands stability (no schema churn, parity benchmarks, observation periods); the second demands experimentation (new node kinds, new evaluator logic, iterating until the cognition feels right). Mixing them means experimental code blocks the stability gate, and stability requirements throttle experimentation.

The split also clarifies dependency on **writer-queue infrastructure** (v04 §6 / §8.15, T61–T68). Several v0.5 ops (`WriteWmSnapshot` compound atomicity, `WriteValenceAccumulator` coalesce lane) want the writer queue to exist. They can fall back to direct SQL dual-write while the queue is built, but designing them assuming the queue makes the code cleaner. The queue itself stays parked under v0.4 §8.15 until a separate feature picks it up; v0.5 ops that need it will write a direct-SQL fallback first and migrate when the queue lands.

## 2. Cognitive function specs

> **Reading order:** §2 contains the design specs (what each function does and how it maps to substrate primitives). §3 contains the action plan (tasks). §2 was transplanted from `v04-unified-substrate/design.md` §4.11–§4.15 — preserved verbatim modulo subsection renumbering, so review history (v04 reviews r1–r5) still applies.

### 2.1 Interoception + somatic markers

**Today (verified 2026-05-12)**:
- `interoceptive/` hub consolidates 5 monitoring subsystems: anomaly detection, empathy accumulator, behavior feedback, confidence calibration, drive alignment. Each emits an `InteroceptiveSignal` (signal layer), aggregated into `InteroceptiveState` (state layer), feeding `RegulationAction` recommendations (action layer).
- `anomaly.rs` maintains per-metric sliding-window baselines.
- `confidence.rs` is two-dimensional: content reliability × meta-confidence.
- Signals today live **in memory only** — they vanish on process restart. No persistence.
- Damasio's somatic-marker hypothesis is the cited model: emotional/embodied signals bias decision-making before deliberation.

**Unified** (per v04 §3 substrate):

A signal is a transient event. A *somatic marker* is the persistent association between a situation pattern and the affective state it evoked. Only the latter belongs in the substrate — signals stay ephemeral.

- **Domain state as node**: each interoceptive *domain* (`coding`, `trading`, `general`, etc.) is a `nodes` row of `node_kind='interoceptive_domain'`. Attributes carry running statistics (rolling valence, anomaly z-score, confidence calibration, alignment score) updated on every signal — small fixed-shape JSON, not a growing log.

- **Somatic-marker as node**: when a signal pattern recurs (e.g. "topic X repeatedly accompanies negative valence + high anomaly"), the hub promotes it to a `nodes` row of `node_kind='somatic_marker'`. Attributes: `{ pattern_signature, evoked_affect, sample_count, last_seen }`.

- **Marker → situation edges**: somatic markers connect to the memory/entity nodes that triggered them via `evoked_by` edges (`edge_kind='associative', predicate='evoked_by', weight=co_occurrence_strength`). This is what lets future retrieval *feel* a topic before it reasons about it.

- **Two-tier signal handling — baseline ephemeral, anomaly persistent**: signals partition into a high-frequency baseline stream and a sparse anomaly stream. The baseline stream (every ingest/recall/action emits one) is **not stored** — the writer folds each signal into the domain node's rolling statistics (`baseline_mean`, `baseline_std`, `last_n_values` capped circular buffer) and discards it. The anomaly stream — signals that cross the z-score threshold or trigger a regulation action — is **persisted** as a `node_kind='anomaly_event'` row. Attributes: `{ domain, metric, raw_value, z_score, window_stats_snapshot, triggered_regulation, rationale }`. Edges: `anomaly_event → observed_in_domain` (to the domain node), `anomaly_event → triggered_by` (to the memory/action/recall that fired it). This matches biology — you don't remember every heartbeat, but you do remember the *moment* your heart raced and what caused it.

- **Volume math**: baseline signal rate is ~1-10/sec across all subsystems (high), so dropping them is the only sane choice. Anomaly rate is ~10-100/day (sparse by definition), so persisting them is cheap and high-value.

- **Somatic markers derive from anomaly_events, not from raw signals**: marker formation walks the anomaly_event nodes — when ≥N anomaly_events on the same domain share a pattern signature, a `somatic_marker` node is created with `derived_from` edges back to the contributing anomaly_events. This is the audit trail: every marker can be traced to the specific moments that shaped it.

- **Confidence / anomaly as memory attributes**: when a signal is bound to a specific memory (write-time confidence, post-recall confidence-update), the value lands in that memory node's attributes (`confidence_at_write`, `confidence_at_recall`). Edges from the memory to the active domain node carry the signal context.

- **Anomaly baseline storage**: per-domain rolling statistics live in the domain node's attributes (`baseline_mean`, `baseline_std`, `window_size`, `last_n_values` — capped circular buffer). No separate `anomaly_baselines` table.

**Reader path (no schema dependency)**:
- "What does the system feel about topic X" → traverse from memory/entity matching X → follow `evoked_by` edges to somatic markers → read evoked_affect.
- "How is domain Y trending" → read the domain node's attributes directly.
- "What specific events shaped this somatic marker" → walk `derived_from` edges from marker to anomaly_events.
- "Why was the system anxious on 2026-05-08" → query `anomaly_event` nodes by date + domain, read their `triggered_by` edges to see the causal events.
- "Should this action be regulated" → read `nodes.attributes` of `node_kind='regulation_policy'` filtered by current domain state.

**Maps cleanly**: four `node_kind`s are introduced by §2.1 — `interoceptive_domain` (one row per domain, mutable rolling stats), `somatic_marker` (sparse, one row per recurring affect-pattern), `anomaly_event` (sparse, one row per threshold crossing), `regulation_policy` (rare, configuration nodes). Baseline signal-stream throughput stays unbounded by storage (it never touches disk — only mutates `interoceptive_domain.attributes`); anomaly + marker write rates are sparse enough to need no batching. Existing `interoceptive/hub.rs` becomes a queue producer; `interoceptive/regulation.rs` becomes a queue consumer reading `regulation_policy` + `interoceptive_domain` attributes.

### 2.2 Empathy bus

**Today (verified 2026-05-12)**:
- `bus/accumulator.rs` tracks per-domain valence trends, flags domains that need SOUL.md updates.
- `bus/alignment.rs` scores how well memories align with active SOUL drives (two strategies: keyword overlap + embedding similarity).
- `bus/feedback.rs` monitors action outcomes (success/failure rates per action type).
- `bus/subscriptions.rs` defines cross-agent notification model (agents subscribe to namespaces).
- `bus/mod_io.rs` reads/writes workspace files: `SOUL.md`, `HEARTBEAT.md`, `IDENTITY.md`. **This is the boundary** — files are external sinks/sources, not substrate.

**Unified** (per v04 §3 substrate):

The Empathy Bus is *partly* substrate-resident and *partly* I/O. Distinguish:

- **In substrate** — the *patterns* the bus learns:
  - **Drive node** (`node_kind='drive'`): each SOUL.md drive is a node. Attributes: `{ name, weight, embedding, source: 'soul'|'derived', last_reinforced }`.
  - **Valence accumulator state**: lives in the domain node from §2.1 (`attributes.valence_window`). Empathy accumulator is a *view* over the same domain node, not a parallel store.
  - **Drive ↔ memory edges** (`edge_kind='associative', predicate='aligns_with', weight=alignment_score`): every memory ingested gets scored against active drives; edges with `weight > threshold` persist. This makes "which memories matter most under drive D" a one-hop traversal.
  - **Action outcome as node** (`node_kind='action_outcome'`): each heartbeat action result is a node. Attributes: `{ action_type, success, latency_ms, notes }`. Edges: `outcome → triggered_by_drive`, `outcome → involves_memory`.

- **External (I/O, not substrate)** — file-system interactions:
  - `SOUL.md` reads → load drive set into substrate as `node_kind='drive'` rows on startup.
  - `SOUL.md` writes (drive evolution suggestions) → produced by analyzing drive nodes + valence accumulator state; written by `bus/mod_io.rs` to the file. The act of writing is logged as a `node_kind='external_write', attributes.target_file='SOUL.md'` audit node.
  - `HEARTBEAT.md` reads/writes → same pattern, logged as external_write audit nodes for traceability.

**Writer paths** (canonical names — to be implemented either via direct SQL dual-write or via the writer-queue once it lands):
- `WriteDriveAlignment { memory_id, drive_id, weight }` — fires on every ingest, low priority, batchable. Persists alignment edges with `weight > threshold` (matches `bus/alignment.rs` scoring).
- `WriteValenceAccumulator { domain, valence_delta, event_count_delta }` — per-domain valence trend update on the domain node from §2.1; one fire per affect-laden event (matches `bus/accumulator.rs`).
- `WriteActionOutcome { action_type, success, latency_ms, ... }` — fires on every heartbeat action completion (matches `bus/feedback.rs`).
- `LogExternalWrite { target_file, operation, content_hash }` — fires before `bus/mod_io.rs` touches a file; ensures every file mutation has a substrate audit trail.

**Subscription model**: cross-namespace subscriptions become `nodes` of `node_kind='subscription'` with `subscriber_namespace` and `target_namespace` attributes. Notifications walk `edges` of type `notifies` from target memory to subscription nodes. No separate `subscriptions` table.

**Why this works**: the bus's job is to make personality emerge from memory patterns. Patterns belong in the graph; the files are just where personality is *externalized for humans to read and edit*. The substrate captures the causal chain; the files are downstream artifacts.

### 2.3 Working memory

**Today (verified 2026-05-12)**:
- `session_wm.rs` (~600 LoC) — per-session ring buffer of active memory nodes ranked by recency/recall-strength.
- `dimension_access.rs` defines the typed-read API but currently has only **2 callers in production** (the rest are scaffolding). The actual WM hot path goes through `session_wm.rs` slots.
- The WM is rebuilt from scratch on every process restart — there is **no persistent WM today**.

**Unified** (per v04 §3 substrate):

The thinking question is "should WM be in the substrate at all, or only in process memory?" The first-principles answer: **WM is hot-path, in-memory by default, with substrate-resident snapshots at meaningful moments**.

Concrete design (revised — push-back accepted):

- **In-memory model**: WM is an in-process `Vec<NodeRef>` of length ~32 with recency scores. Sub-millisecond access — substrate read every cognitive cycle would dominate latency, and metacog (§2.4) cares about *what WM looked like during specific decisions*, not what WM looked like at every microsecond.
- **Demand-driven materialization** — NOT continuous, NOT pure session-replay. A `node_kind='wm_snapshot'` row is written only when *something downstream wants the WM state preserved*. The two real triggers:
  1. **Metacog feedback emits a snapshot** (§2.4 below): when metacog writes a `feedback_event` node, it captures the WM state *at that moment* as a `wm_snapshot` so future analysis can ask "what was in working memory when this judgment was made?" — atomic with the feedback write.
  2. **Anomaly event captures a snapshot** (§2.1 cross-link): when an `anomaly_event` fires, it captures WM as part of its `window_stats_snapshot` field — same atomicity guarantee.
- **No periodic snapshots, no idle-driven snapshots**: avoids the write-amplification problem and aligns with the cognitive-science motivation (we remember WM at decision moments, not continuously).
- **Snapshot shape**: `node_kind='wm_snapshot', attributes = { slot_contents: [node_id, ...], slot_scores: [...], drive_state: [...], wm_state: 'cold_start'|'warm' }`. Edges: `snapshot → captured_during` (to the metacog/anomaly event that triggered it). The `wm_state` discriminator (`cold_start` vs `warm`) signals whether the snapshot reflects fresh-process WM (likely sparse, post-restart) vs steady-state WM (representative); downstream analysis filters/groups on this.

This satisfies both pressures: the hot path stays in-memory (no substrate writes on every WM mutation), but the substrate captures meaningful states for cognitive analysis. The substrate is *participatory* in WM management rather than a *log of everything*.

### 2.4 Metacognition

**Today (verified 2026-05-12)**:
- `metacognition.rs` tracks recall accuracy, synthesis quality, channel effectiveness over time via the `MetaCognitionTracker` struct.
- Stores evaluation events (rolling window) in the `metacognition_events` SQLite table.
- Independent of `interoceptive/` today — the interoceptive hub gets `feedback` baseline signal from `bus/feedback.rs` (heartbeat action outcomes), not from metacognition. The unified design below proposes connecting them via `evaluates` edges (see §2.1 cross-reference).

**Unified** (per v04 §3 substrate):

Metacognition is *judgments about other cognitive operations*. Each judgment is an event with a target — a perfect fit for the node-edge model.

- **Feedback event as node**: each evaluation is a `nodes` row of `node_kind='metacog_feedback'`. Attributes: `{ score, dimension, evaluator, rationale, timestamp }` where `dimension ∈ {recall_accuracy, synthesis_quality, channel_effectiveness, retrieval_relevance}`.
- **Feedback → target edge**: every feedback event has an `evaluates` edge pointing to the memory/synthesis/retrieval-trace it judged.
- **Aggregate views are derived, not stored**: "current recall accuracy" is `SELECT AVG(attributes.score) FROM nodes WHERE node_kind='feedback' AND dimension='recall_accuracy' AND created_at > now - 7d`. No materialized rollup table — if the query becomes hot, add a `node_kind='metacog_summary'` written daily by the writer.
- **Retrieval trace as node** (already in `retrieval/`): each query execution is a `node_kind='retrieval_trace'` with attributes `{ query_text, plan_used, result_count, latency_ms }`. Feedback events evaluate these.

**Writer paths**:
- `WriteFeedbackEvent { dimension, score, target_id, evaluator, rationale }` — medium priority, no batching constraint (these are rare).
- `WriteWmSnapshot { feedback_event_id, slot_contents, wm_state }` — fires in the same transaction as `WriteFeedbackEvent` so the snapshot and the evaluation are atomically linked (§2.3 demand-driven trigger). `wm_state ∈ {cold_start, warm}` is captured from the in-memory ring buffer at snapshot time; a `cold_start` snapshot is a legitimate observation (the agent really had empty WM post-restart), not a data-quality bug, but downstream metacog analysis can filter on it.
- Aggregation is **read-time** (one SQL query) unless a daily summary node is materialized; that's a separate background op.

**Why this works**:
- Metacognition becomes a first-class part of the memory graph — the system can reason about its own past evaluations the same way it reasons about facts.
- "Show me memories the system was wrong about" is a traversal: feedback → evaluates → memory, filter `feedback.score < threshold`.
- Closing the loop with §2.1 interoception: low metacog scores in dimension X flow into anomaly detection on domain X, triggering somatic-marker formation ("I tend to be wrong about this kind of question") — exactly the cognitive-science motivation.

### 2.5 Dimensional signature

**Today (verified 2026-05-12)**:
- `crates/engramai/src/dimensions.rs` (1362 LoC) defines `Dimensions` — a typed signature attached to every memory row. 16+ fields: `core_fact: NonEmptyString`, narrative fields (`participants`, `temporal`, `location`, `context`, `causation`, `outcome`, `method`, `relations`, `sentiment`, `stance`), scalar dimensions (`valence: Valence`, `domain: Domain`, `confidence: Confidence`), and aggregate fields (`tags: BTreeSet<String>`, `type_weights: TypeWeights`).
- `dimension_access.rs` (237 LoC) is the typed-read API over those fields — callers ask `dims.domain()` rather than parsing JSON.
- Storage today: serialized as a JSON blob in `memories.dimensions` column. Reads load the whole blob and deserialize.
- Used by: retrieval (filter by `domain`/`valence`/`confidence`), KC (cluster by `domain`/`tags`), metacog (track per-`dimension` accuracy in §2.4), interoception (anomaly bias per `domain` in §2.1).

**Unified** (per v04 §3 substrate): Dimensions split cleanly into **three storage tiers** based on access pattern, with no semantic loss.

#### 2.5.1 Tier 1 — scalar dimensions as first-class attributes

Fields with **structured types and high query frequency** become typed attributes on the memory's node row:

```
node_kind='memory', attributes = {
  core_fact:   "<NonEmptyString text>",   -- required, denormalized from content
  valence:     -0.7,                       -- f64 in [-1, 1]
  domain:      "tech",                     -- enum string
  confidence:  "verified",                 -- enum string
  type_weights: { episodic: 0.6, ... }     -- shaped sub-object
}
```

These four scalars (`valence`, `domain`, `confidence`, `type_weights`) drive **filter predicates** in retrieval (`WHERE attributes->>'domain' = 'tech'`) and **bucket keys** in KC clustering. They are accessed on every retrieval call. Keeping them in `attributes` means a single row read returns them; no join.

`core_fact` is denormalized into `attributes` (in addition to being in `nodes.content`) because retrieval ranking sometimes needs the distilled fact *without* the full memory content — and the non-empty invariant is a node-creation-time check (writer validates), preserving the `NonEmptyString` guarantee.

#### 2.5.2 Tier 2 — narrative fields as `describes_<field>` edges to dimension nodes

Fields with **free-text values and combinatorial reuse** (the same `location: "Caroline's house"` appears on 40 memories) become **separate nodes** with edges:

```
node_kind='memory'  ──describes_location──>  node_kind='dimension_location'
                                            attributes = { value: "Caroline's house" }
```

The 10 narrative fields (`participants`, `temporal`, `location`, `context`, `causation`, `outcome`, `method`, `relations`, `sentiment`, `stance`) each get their own `node_kind`: `dimension_participants`, `dimension_location`, etc. (v04 §3.1 schema has only single-level `node_kind`; we encode field identity into the kind string rather than inventing a second discriminator.) Each unique value (e.g. `"Caroline's house"`) is a single node; every memory referencing it gets an edge with `edge_kind='containment', predicate='describes_<field>'`.

**Why `containment` and not `structural`**: a dimension edge is set-membership semantics — a memory either has this location/participant/etc. or it doesn't, and re-ingesting the same value MUST be a no-op (not a second edge with the same predicate). v04 §3.2's partial UNIQUE index on `(source_id, target_id, edge_kind, predicate) WHERE edge_kind='containment'` enforces this idempotence at the SQL layer.

**Why edges, not duplicated strings**:

1. **Discoverability** — "find every memory at Caroline's house" becomes a 1-hop edge traversal (`SELECT m.id FROM edges WHERE target_id=$loc AND predicate='describes_location'`), not a string LIKE scan over a million JSON blobs.
2. **Co-occurrence cheap** — "what locations co-occur with participant Caroline?" is a 2-hop graph query, exactly what the substrate is for.
3. **Reuse without duplication** — 40 memories at Caroline's house = 40 edges + 1 node, not 40 copies of the string. Storage cost ≈ 40 × 8 bytes (edge row) + 1 × ~30 bytes (node), vs 40 × ~30 bytes today.
4. **Resolution can merge** — v04 §4.2 ResolutionPipeline already canonicalizes entity strings; `"Caroline's house"` and `"Caroline house"` become the same dimension node via the same merge machinery.

#### 2.5.3 Tier 3 — tag set as `tagged` edges

`tags: BTreeSet<String>` becomes N edges (`edge_kind='containment', predicate='tagged'`) to `node_kind='tag'` nodes. Same rationale as Tier 2 — tag reuse is the whole point of tags, edges make reuse explicit. A `tagged` edge has no weight (presence/absence is the signal); the partial UNIQUE index on `(source_id, target_id, edge_kind, predicate) WHERE edge_kind='containment'` from v04 §3.2 prevents accidental duplicates.

#### 2.5.4 Compatibility with current `dimension_access.rs`

The 237 LoC accessor module becomes a **thin shim** post-migration:

- `dims.valence()` / `.domain()` / `.confidence()` — read directly from `nodes.attributes` (single column access, no join).
- `dims.location()` / `.participants()` / etc. — load the edges with `predicate='describes_<field>'` for the node, return the target node's `attributes.value`. For the common single-value case (most narrative fields are 0..1), the accessor returns `Option<String>` exactly as today.
- `dims.tags()` — load edges with `predicate='tagged'`, materialize the `BTreeSet`.

Callers see the same API. The `Dimensions` struct itself can be **reconstructed** on demand for code paths that still want the flat shape (e.g. legacy serialization, debug prints) — but new code traverses the graph natively. Dual-write during the migration ensures the JSON blob stays valid until callers migrate.

#### 2.5.5 Write amplification budget

Production projection (modeling RustClaw + AgentVerse load):

- **Today** (~5 P50, ~13 P95 ops per memory): `memories` row + ~4 mention rows P50.
- **Unified Tier 1+2+3** (~12 P50, ~25 P95 ops per memory):
  - Memory node (1), `describes_*` edges (4 P50 / 7 P95), `tagged` edges (2 P50 / 5 P95), new dim nodes (0.4 P50 / 1 P95), new tag nodes (0.3 P50 / 1 P95), entity mention edges (3 P50 / 8 P95), entity nodes from resolution misses (0.3 P50 / 1 P95), `belongs_to_episode` edge (0.7 P50 / 1 P95).
- **Write amplification ratio**: ~2.4× P50, ~1.9× P95.

Three properties make it tractable:

1. **All inserts batch into one transaction**, sharing one fsync. Once the writer queue (v04 §6) lands, `BATCH_MAX = 64` ops absorbs even P95 ingest in a single batch.
2. **Dedup misses decline over time** — 11% on a fresh 441-memory corpus drops to ~5% at 24k, ~2% projected at 100k.
3. **Edge UNIQUE constraint short-circuits duplicates** — re-ingested memory with identical Tier 2 values hits the partial unique index and turns into a no-op (~1ms cost).

Peak ingest projection: 50 memories/sec × 25 ops P95 = 1250 ops/sec writer throughput required. Substrate ceiling ~11000 ops/sec (v04 §6.6). **Headroom ~8.8×**.

Mitigations available if telemetry shows write-amplification becoming the bottleneck (none implemented at launch):

- Tier 2 lazy materialization (defer rare narrative fields to background pass).
- Tag node lazy creation (only promote tag → node once reuse value crosses threshold).
- Dimension node coalescing in writer (per-batch dim-node cache; already implicit in batched-tx shape).

## 3. Action plan

### 3.1 Interoception (§2.1)
- [ ] **T45** Schema: add `interoceptive_baseline` (ephemeral, derivable) and
  node_kind `anomaly_event` (persistent) variants — verify v04 §3.1 enum + add
  attribute schemas (`{moving_avg, variance, sample_count}` for baseline;
  `{trigger_node_id, observed_value, expected_value, severity}` for event).
  Decision recorded: baseline is **Tier 1 (in-memory only)** per §2.1
  push-back — does NOT persist as a node. Only the `anomaly_event` persists.
- [ ] **T46** Implement `InteroceptionService` (in-memory rolling stats by
  dimension) — pure function, no DB writes for normal observations.
- [ ] **T47** Wire anomaly detection: when delta > threshold → emit
  `WriteAnomalyEvent` (direct SQL dual-write at first; migrate to writer
  queue if/when v04 §6 lands). Backpressure-OK since anomalies are rare.
- [ ] **T48** Test: synthetic dimension stream with injected spike → exactly
  one `anomaly_event` node written, baseline stays in-memory, restart loses
  baseline (Tier 1 ephemeral contract) but `anomaly_event` survives.

### 3.2 Empathy bus (§2.2)
- [ ] **T49** Refactor `bus/` to emit via the standard write API
  (`WriteDriveAlignment` / `WriteValenceAccumulator` / `WriteActionOutcome` /
  `LogExternalWrite` canonical names). Schema additions: `node_kind='drive'`,
  `node_kind='action_outcome'`, `node_kind='external_write'`; domain node from
  §2.1 absorbs valence accumulator state via `attributes.valence_window`.
- [ ] **T50** Subscriber adapter: existing handlers re-register against the
  unified bus reader path; verify no events lost during migration via
  golden-file replay.

### 3.3 Working memory (§2.3)
- [ ] **T51** Implement in-memory `WorkingMemory` (vec of active node refs +
  recency scores) — does NOT persist by default per §2.3.
  Includes `state: WmState` field initialized to `cold_start`; flip to
  `warm` on the **first** of these events:
  (a) the session's metacog loop completes its first cycle, or (b) a prior-session
  `wm_snapshot` is loaded back into the ring buffer. Flag is read-only thereafter
  for the session lifetime. Captured into `wm_snapshot` payloads via T52 so
  downstream metacog analysis can distinguish "agent had genuinely empty WM"
  from "agent had just restarted and not yet recalled anything".
- [ ] **T52** Metacognition-driven `wm_snapshot`: when metacog decides a WM
  state is worth persisting, emit `WriteWmSnapshot` (compound op — must commit
  atomically with the triggering `feedback_event`). Until writer queue lands,
  implement as a single explicit transaction in the metacog path.
- [ ] **T53** Test: WM mutates 100x without persistence; metacog triggers
  one snapshot → exactly one snapshot node + N edges in single transaction;
  WM in-memory state still authoritative after snapshot.

### 3.4 Metacognition (§2.4)
- [ ] **T54** Implement metacog evaluator: reads recent `feedback_event` +
  `anomaly_event` nodes from substrate, produces `meta_judgment` writes
  (e.g., "retrieval plan X underperformed on entity-heavy queries").
- [ ] **T55** Wire metacog → `WriteMetaJudgment` + optional
  `WriteWmSnapshot` compound (atomicity per §2.3 / §2.4).

### 3.5 Dimensional signature (§2.5)
- [ ] **T56** Implement Tier 1 (scalar dimensions in `nodes.attributes`):
  extend `MemoryRecord` ingest path to compute `valence`/`domain`/
  `confidence`/`type_weights` and persist them as JSON fields in
  `nodes.attributes` at write time. No new table.
- [ ] **T57** Implement Tier 2 (narrative fields as `describes_<field>`
  edges): each unique narrative value becomes a `node_kind='dimension_<field>'`
  node, every memory referencing it gets an `edge_kind='containment',
  predicate='describes_<field>'` edge. Resolution-pipeline canonicalization
  applies (§2.5.2). Routes through the standard memory-write op; the writer
  expands `dimensions` inline into the same transaction as the parent memory
  INSERT — no caller-constructed batch.
- [ ] **T58** Implement Tier 3 (`tagged` edges to `node_kind='tag'`
  nodes): each tag is a node, each memory→tag is an `edge_kind='containment',
  predicate='tagged'` edge. Partial UNIQUE index from v04 §3.2
  (`idx_edges_containment_unique`) prevents dup edges; re-ingest of the
  same tag is a SQL no-op.
- [ ] **T59** Rewrite `dimension_access.rs` as a thin shim over the
  unified schema (§2.5.4): scalar accessors read `nodes.attributes`,
  narrative accessors load edges by `predicate='describes_<field>'`,
  tag accessor loads edges by `predicate='tagged'`. Bench: shim cost vs
  current accessor on a 1k-memory namespace.

## 4. Out of scope (deferred)

- **Writer queue infrastructure** (v04 §8.15, T61–T68). Parked under v04 because it's substrate-level, not cognitive-function-level. Will get its own feature when picked up. v0.5 ops use direct SQL dual-write in the interim.
- **v0.2 KC retirement** (v04 §8.14, T60). Pure cleanup, stays under v0.4 cleanup.
- **Cross-namespace cognitive aggregation** (e.g., a single drive_alignment computed across multiple namespaces). Single-namespace only for v0.5.

## 5. Status

2026-05-14: split out from v04-unified-substrate. Design specs (§2) and action plan (§3) transplanted verbatim from v04 §4.11–§4.15 / §8.9–§8.13. `requirements.md` written. 15 tasks (T45–T59) inherited.

**Next step**: this design has **not yet been reviewed** as a standalone artifact. v04's r1–r5 reviews touched the original §4.11–§4.15 prose, but the act of splitting may have introduced edge cases (e.g. cross-references that no longer resolve cleanly, or assumed-shared context with v04 that needs surfacing here). A focused r1 review pass — running the `review-design` skill against this doc with `[REVIEW_DEPTH: standard]` — should be the first action before any T45–T59 work begins.

**Blocking**: cannot start any task until v04 Phase A-D is verified complete on the target DB (substrate online + read-switch wired). See GUARD-X1.
