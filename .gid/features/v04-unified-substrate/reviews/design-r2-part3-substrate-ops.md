# Design Review r2-part3 — Substrate Cognitive Ops (§4.11–§4.16)

> **Reviewer:** claude (sub-agent, design-review skill)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` lines 559–854
> **Scope:** §4.11 Interoception, §4.12 Empathy bus, §4.13 Working memory, §4.14 Metacognition, §4.15 Dimensional signature, §4.16 v0.2 KC triage
> **Method:** 36-check review-design skill, depth=full

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 4 |
| 🟡 Important  | 6 |
| 🟢 Minor      | 3 |
| **Total**  | **13** |

**Recommendation**: Needs fixes before implementation. The 4 critical findings (FINDING-A3-1, A3-2, A3-4, A3-11) all concern undefined or contradictory type/column mappings — an implementer cannot build §4.11-§4.16 without resolving them. The design's cognitive architecture is sound; the gap is between the schema specification (§3) and the operation descriptions (§4.11-§4.16) which were written using column names and type discriminators that don't exist in the schema.

**Estimated implementation confidence**: Medium — design intent is clear and well-justified, but schema-mapping gaps would cause a round-trip for every section.

---

## FINDING-A3-1 🔴 Critical — `node_type`/`edge_type` columns referenced but not in schema

**Check #4 (Consistent naming) + Check #1 (Types fully defined)**

§4.11, §4.12, §4.13, §4.14, §4.15, §4.16 all use a two-level type discriminator (`node_type` + `node_kind`) for nodes and (`edge_type` + `edge_kind` or just `edge_type`) for edges. But §3.1 `nodes` table has **only `node_kind`** — there is no `node_type` column. Similarly §3.2 `edges` table has **only `edge_kind`** — there is no `edge_type` column.

Concrete instances:

| Section | Usage | Schema reality |
|---------|-------|----------------|
| §4.11 | `node_type='interoceptive', node_kind='domain'` | No `node_type` column |
| §4.11 | `edge_type='evoked_by'` | No `edge_type` column; should be `edge_kind` + `predicate` |
| §4.12 | `edge_type='aligns_with'` | Same — no `edge_type` |
| §4.13 | `edge_type='wm_contained'`, `edge_type='wm_snapshot_of'` | Same |
| §4.14 | `node_type='metacog', node_kind='feedback'` | No `node_type` column |
| §4.15 | `node_type='memory'`, `node_type='dimension'` | No `node_type` column |
| §4.16 | `node_type='topic', node_kind='knowledge_topic'` | No `node_type` column |

This is not just a naming inconsistency — it's an **undefined discriminator**. §3.2 edges have a deliberate two-level design: `edge_kind` (outer: structural/associative/containment/provenance/temporal/supersession) + `predicate` (inner: open string). But §4.11-§4.16 ignore this and use `edge_type` as if it were `predicate`, bypassing the outer `edge_kind` taxonomy entirely.

For nodes, the design appears to want a two-level discriminator (`node_type='interoceptive'` + `node_kind='domain'`), but only one column exists. Either:
- (a) Add `node_type` column to §3.1 (schema change), or
- (b) Flatten: `node_kind='interoceptive_domain'`, `node_kind='metacog_feedback'`, etc., or
- (c) Use `node_kind` for the top-level and put the sub-kind in `attributes`.

**Impact**: every new op in §4.11-§4.16 has undefined storage mapping. An implementer would not know which column to write or query.

**Suggested fix**: For each usage, map to the actual schema:
- `node_type='interoceptive', node_kind='domain'` → `node_kind='interoceptive_domain'`
- `edge_type='evoked_by'` → determine which `edge_kind` bucket (structural? associative? provenance?) and set `predicate='evoked_by'`
- Apply consistently across all §4.11-§4.16 sections

---

## FINDING-A3-2 🔴 Critical — New edge predicates not mapped to §3.2 `edge_kind` taxonomy

**Check #1 (Types fully defined) + Check #6 (Data flow completeness)**

§3.2 defines a closed `edge_kind` taxonomy: `structural | associative | containment | provenance | temporal | supersession`. Every edge must declare which `edge_kind` bucket it belongs to, then set `predicate` for the inner type.

§4.11-§4.16 introduce **~15 new edge predicates** but never specify which `edge_kind` they belong to:

| Section | Predicate | Probable `edge_kind` | Why ambiguous |
|---------|-----------|---------------------|---------------|
| §4.11 | `evoked_by` | associative? structural? | Causal link (marker→trigger) — could be either |
| §4.11 | `observed_in_domain` | containment? structural? | Domain grouping |
| §4.11 | `triggered_by` | provenance? | Causal chain |
| §4.11 | `derived_from` (marker←anomaly) | provenance ✓ | Clear match |
| §4.11 | `anomaly_of` (per task context) | ? | Not in design body |
| §4.12 | `aligns_with` | associative? | Drive alignment score |
| §4.12 | `triggered_by_drive` | provenance? | Action outcome provenance |
| §4.12 | `involves_memory` | provenance? structural? | Unclear |
| §4.12 | `notifies` | ? | New concept — no matching `edge_kind` |
| §4.13 | `wm_contained` | containment ✓ | Clear match |
| §4.13 | `wm_snapshot_of` | provenance ✓ | Clear match |
| §4.14 | `evaluates` | provenance ✓ | Clear match |
| §4.15 | `describes_*` (10 variants) | structural? | Dimension relationships |
| §4.15 | `tagged` | structural? containment? | Tag membership |

**Impact**: Implementers must guess the `edge_kind`. Wrong guesses break the partial UNIQUE constraints (§3.2 — only `associative` and `containment` have UNIQUE indexes). If `aligns_with` is categorized as `associative`, it gets a UNIQUE constraint (one drive-memory alignment per pair — probably correct). If categorized as `structural`, duplicates are allowed (probably wrong).

**Suggested fix**: Add a table to each §4.x section mapping every new predicate to its `edge_kind` bucket. For example:
```
§4.11 edges:
  evoked_by        → edge_kind='associative', predicate='evoked_by'
  observed_in_domain → edge_kind='containment', predicate='observed_in_domain'
  triggered_by     → edge_kind='provenance', predicate='triggered_by'
  derived_from     → edge_kind='provenance', predicate='derived_from'
```

---

## FINDING-A3-3 🟡 Important — Missing WriteOp variants for somatic_marker and regulation_policy

**Check #3 (No dead definitions) + Check #6 (Data flow completeness)**

§4.11 defines three persistent node kinds:
1. `node_kind='anomaly_event'` — has `WriteAnomalyEvent` in §6.1 ✓
2. `node_kind='somatic_marker'` — **no WriteOp variant in §6.1**
3. `node_kind='regulation_policy'` — **no WriteOp variant in §6.1**

§4.11 says "when ≥N anomaly_events on the same domain share a pattern signature, a `somatic_marker` node is created." But there's no `WriteSomaticMarker` op in the writer queue enum. Who creates it? Through which queue path?

Similarly, §4.11's reader path mentions `node_kind='regulation_policy'` for regulation decisions — "read `nodes.attributes` of `node_kind='regulation_policy'` filtered by current domain state" — but nothing in the design creates these nodes.

**Impact**: Somatic markers are the core thesis of §4.11 (Damasio's hypothesis). If the write path is undefined, the feature has no way to materialize its most important artifact.

**Suggested fix**: Add to §6.1:
```rust
WriteSomaticMarker { domain: String, pattern_signature: PatternSignature, evoked_affect: Affect, anomaly_event_ids: Vec<NodeId> },
WriteRegulationPolicy { domain: String, policy: RegulationPolicy, ... },
```
Or if regulation policies are static config (loaded from SOUL.md, not written by the system), document that explicitly and remove the reader-path reference to `regulation_policy` nodes.

---

## FINDING-A3-4 🔴 Critical — Domain node rolling-stats update has no write path but §4.11 says it updates on every signal

**Check #6 (Data flow completeness) + Check #7 (Error handling completeness)**

§4.11 says: "each interoceptive *domain* [...] is a `nodes` row [...]. Attributes carry running statistics (rolling valence, anomaly z-score, confidence calibration, alignment score) **updated on every signal**."

But §4.11 also says: "The baseline stream (every ingest/recall/action emits one) is **not stored**" and "the writer folds each signal into the domain node's rolling statistics (`baseline_mean`, `baseline_std`, `last_n_values` capped circular buffer) **and discards it**."

These two statements contradict: if the writer "folds each signal into the domain node's rolling statistics," that IS a write to the domain node — at 1-10/sec. But:

1. §6.1 has no `UpdateDomainStats` WriteOp variant
2. §6.3 priority lanes say baseline signals are high-frequency; if they go through the writer at 1-10/sec, that's 36,000 domain-node UPDATEs/hour — not "ephemeral"
3. §4.11's own volume math says "dropping them is the only sane choice" — but then says the writer folds them into the domain node

**Root contradiction**: Is the domain node updated on every signal (persistent rolling stats) or is the baseline fully in-memory (truly ephemeral)? The design says both.

**Option A**: Rolling stats are in-memory only (like WM in §4.13). Domain nodes are written rarely (e.g., on anomaly events or periodic snapshots). This matches "baseline ephemeral" but requires clarifying that domain-node attributes are NOT hot-path-updated.

**Option B**: Rolling stats are persisted on every signal via a WriteOp. This contradicts the ephemeral design and the volume math.

**Suggested fix**: Clarify that domain node attributes are an in-memory cache, persisted only:
- On anomaly event (snapshot current stats alongside the event)
- On graceful shutdown (optional checkpoint)
- On explicit introspection query

Add an `UpdateDomainSnapshot` WriteOp for the rare persistence case, or document that domain nodes are read-only after initial creation and stats live purely in `InteroceptionService` memory.

---

## FINDING-A3-5 🟡 Important — §4.12 defines 4 specific WriteOps but §6.1 collapses them into one generic variant

**Check #6 (Data flow completeness) + Check #21 (Ambiguous prose)**

§4.12 "Writer paths through §6 queue" explicitly names 4 operations:
1. `WriteAlignmentEdge { memory_id, drive_id, score }` — on every ingest, low priority, batchable
2. `WriteActionOutcome { ... }` — on every heartbeat action
3. `UpdateDriveReinforcement { drive_id, delta }` — on high-alignment recall
4. `LogExternalWrite { target, content_hash }` — before file mutation

§6.1 has only: `WriteEmpathySignal { kind: EmpathySignalKind, ... }`

Two interpretations for an implementer:
- (a) `EmpathySignalKind` is an enum with 4 variants matching the above → the `...` hides the payload discrimination. But the design doesn't define `EmpathySignalKind`.
- (b) The 4 operations from §4.12 were meant to be separate WriteOp variants and §6.1 forgot to list them.

Either way, an implementer cannot build this without guessing.

Additionally, `WriteAlignmentEdge` fires "on every ingest" at "low priority." If every memory ingest produces N alignment edge writes (one per active drive), and there are K drives, that's K writes per ingest. At 100 ingests/hour with 10 drives = 1000 alignment edge writes/hour. Not a problem at scale, but the priority/batching semantics are underspecified — does "batchable" mean Hebbian-style coalescing? Can alignment scores for the same (memory, drive) pair accumulate?

**Suggested fix**: Either expand §6.1 with the 4 specific variants from §4.12, or define `EmpathySignalKind` with its payload shapes. Document coalescing semantics for `WriteAlignmentEdge`.

---

## FINDING-A3-6 🟡 Important — §4.13 and §4.15 give contradictory dimension_access.rs migration plans

**Check #2 (References resolve) + Check #4 (Consistent naming)**

§4.13 (Working memory) ends with:
> "Dimension access: `dimension_access.rs` becomes a typed reader over `nodes.attributes.dimensions` (a fixed-shape JSON sub-object). No schema change — dimensions are already an attribute set, just typed at the accessor layer."

§4.15 (Dimensional signature) specifies a full 3-tier redesign:
- Tier 1: scalar dimensions in `nodes.attributes` (valence, domain, confidence, type_weights)
- Tier 2: narrative fields as separate `node_kind='dimension'` nodes with `describes_*` edges
- Tier 3: tags as `tagged` edges to `node_kind='tag'` nodes

§4.15.4 then says: "`dimension_access.rs` becomes a thin shim" that reads from `nodes.attributes` for scalars and traverses edges for narrative fields.

The §4.13 version ("fixed-shape JSON sub-object, no schema change") directly contradicts the §4.15 version ("3-tier, dimension nodes, describes edges"). §4.15 supersedes §4.13's casual mention, but §4.13 should not contain stale information that an implementer might follow.

**Suggested fix**: Remove the `dimension_access.rs` paragraph from §4.13 entirely (or replace with "See §4.15 for dimensional signature redesign"). §4.13 is about working memory — dimension access doesn't belong here.

---

## FINDING-A3-7 🟢 Minor — §4.14 doesn't specify metacognition's observation mechanism or recursion guard

**Check #5 (State machine invariants — self-transitions) + Check #21 (Ambiguous prose)**

§4.14 defines what metacognition *produces* (feedback events, wm_snapshots) and where it *stores* results (substrate nodes). But it doesn't specify:

1. **How metacog observes**: does it subscribe to empathy bus events (§4.12)? Poll recent retrieval traces periodically? Get triggered by specific system events? The "Today" section says `MetaCognitionTracker` feeds `interoceptive/feedback.rs`, but the unified design doesn't specify the trigger mechanism for evaluation.

2. **Recursion guard**: metacog creates `node_kind='feedback'` nodes. The `evaluates` edge points to "the memory/synthesis/retrieval-trace it judged." But nothing prevents metacog from evaluating its own feedback nodes — producing meta-meta-cognitive feedback, which it could then evaluate again. While biologically interesting, this is computationally problematic. A guard like "metacog only evaluates nodes where `node_kind NOT IN ('feedback', 'retrieval_trace')`" or a depth counter would prevent runaway recursion.

These are minor because the current `metacognition.rs` presumably has both answers, and an implementer would carry them forward. But since §4.14 is the authoritative design, they should be specified here.

**Suggested fix**: Add a "Trigger mechanism" paragraph specifying when metacog runs and what it reads. Add a "Recursion guard" note: e.g., "Metacog evaluates `retrieval_trace` and `memory` nodes only — never its own `feedback` nodes."

---

## FINDING-A3-8 🔴 Critical — §4.15 Tier 2 `describes_*` edges break §3.2 `edge_kind` closed taxonomy

**Check #32 (Conflicts with existing architecture) + Check #1 (Types fully defined)**

§3.2 defines `edge_kind` as a closed, two-level discriminator with exactly 6 values: `structural | associative | containment | provenance | temporal | supersession`. The design says: "two-level discriminator = stable outer type + open inner predicate."

§4.15 Tier 2 uses `edge_kind` = the field name: `describes_location`, `describes_participants`, `describes_temporal`, `describes_context`, `describes_causation`, `describes_outcome`, `describes_method`, `describes_relations`, `describes_sentiment`, `describes_stance` — **10 new `edge_kind` values**, none of which are in the taxonomy.

This is architecturally inconsistent. The `edge_kind` taxonomy was designed to be a *small, stable outer discriminator* with partial UNIQUE indexes defined per-kind. Adding 10 dimension-field-specific kinds destroys that design:
- The `idx_edges_live` index (`WHERE invalidated_at IS NULL`) would need to cover 16+ `edge_kind` values
- The partial UNIQUE indexes are defined per-`edge_kind` — should `describes_location` have a UNIQUE constraint?
- New dimensions added later would require schema-level changes (new indexes), contradicting §1.3's promise that "adding new node-kinds is a schema-free operation"

**Correct mapping**: `describes_*` edges should use `edge_kind='structural'` (they are structural relationships between a memory and its dimension values) with `predicate='describes_location'`, `predicate='describes_participants'`, etc. The two-level discriminator was designed for exactly this pattern.

Similarly, Tier 3 `tagged` edges should use `edge_kind='containment'` (tag membership) with `predicate='tagged'`.

**Suggested fix**: Replace throughout §4.15:
- `edge_kind='describes_location'` → `edge_kind='structural', predicate='describes_location'`
- `edge_kind='tagged'` → `edge_kind='containment', predicate='tagged'`

Also update the §3.2 taxonomy table to list these predicates as examples.

---

## FINDING-A3-9 🟡 Important — §4.15 uses wrong column names (`to_id`, `from_id`) vs §3.2 (`target_id`, `source_id`)

**Check #4 (Consistent naming)**

§3.2 defines: `source_id TEXT NOT NULL REFERENCES nodes(id)` and `target_id TEXT REFERENCES nodes(id)`.

§4.15 uses different names:
- Line 744: `SELECT m.id FROM edges WHERE to_id=$loc` → should be `target_id`
- Line 751: `UNIQUE constraint on (from_id, to_id, edge_kind)` → should be `(source_id, target_id, edge_kind, predicate)` per §3.2

§6.1 WriteOp also uses: `BumpAssociation { from_id, to_id, delta }` — this is Rust field naming, not SQL column naming, so it's fine. But §6.8 apply_write_memory uses `from_id, to_id` in SQL INSERT:
- Line 1225: `INSERT INTO edges (from_id, to_id, edge_kind)` → should be `(source_id, target_id, edge_kind)`

**Impact**: An implementer copying SQL from the design verbatim gets compile-time/runtime errors. Naming inconsistency between design sections also suggests the sections were written at different times without cross-checking.

**Suggested fix**: Global find-and-replace in SQL contexts: `to_id` → `target_id`, `from_id` → `source_id`. Keep Rust struct field names (`from_id`/`to_id`) if desired, but the SQL must match §3.2.

---

## FINDING-A3-10 🟢 Minor — §4.15.2 storage cost estimate is off by ~30x

**Check #20 (Appropriate abstraction level — wrong math)**

§4.15.2 "Why edges, not duplicated strings" claims:
> "40 memories at Caroline's house = 40 edges + 1 node, not 40 copies of the string. Storage cost ≈ 40 × 8 bytes (edge row) + 1 × ~30 bytes (node), vs 40 × ~30 bytes today."

An edge row in §3.2 has 17 columns including two TEXT UUIDs (36 bytes each), multiple TEXT fields, REAL timestamps, etc. A realistic SQLite row size for a minimal edge is ~200-300 bytes with index overhead, not 8 bytes.

Corrected math: 40 edges × ~250 bytes = ~10KB + 1 node × ~200 bytes ≈ **10.2KB**, vs 40 copies × ~30 bytes = **1.2KB** in the current JSON blob approach.

The edge approach is actually **more expensive in raw storage** than the duplicated-string approach (~8x more). The justification should rest entirely on the **query benefits** (traversal, co-occurrence, resolution), not on storage savings. The current framing misleads about the storage trade-off.

**Suggested fix**: Remove the incorrect storage-cost math. The real justification (discoverability, co-occurrence, resolution) is already well-argued in points 1, 2, and 4 of §4.15.2. Point 3 should acknowledge the storage cost increase and frame it as an acceptable price for graph queryability.

---

## FINDING-A3-11 🔴 Critical — §8.13 tasks (T56-T59) contradict §4.15 design: `node_dimensions` table vs dimension-as-edges

**Check #2 (References resolve) + Check #32 (Conflicts with existing architecture)**

§4.15 designs a 3-tier dimensional signature model where **Tier 2 narrative fields become separate `node_kind='dimension'` nodes connected by `describes_*` edges**. There is no `node_dimensions` table in §4.15.

§8.13 (Dimensional signature tasks) specifies:
- T56: "Schema: `node_dimensions` table (Tier 2 — full dimension vector per node, supersedes `dimension_access.rs`'s ad-hoc storage). Includes `dimension_kind`, `value`, `confidence`, indexed on `(node_id, dimension_kind)`."
- T57: "Dual-write: [...] writes both legacy `dimensions` table (if present) and new `node_dimensions`"
- T58: "Retrieval adapter: dimensional plan reads from `node_dimensions`"

These are two **completely different designs**:
- §4.15: dimensions are **graph-native** — dimension nodes + edges. No new table.
- §8.13: dimensions are a **new relational table** (`node_dimensions`). Not graph-native.

This means the implementation tasks would build the wrong thing. An implementer following §8 would create a separate `node_dimensions` table, directly contradicting §4.15's thesis that "a graph database whose nodes carry blob-JSON narrative fields is just SQLite with a tax."

**Impact**: Blocks implementation. The design and task plan describe different architectures.

**Suggested fix**: Rewrite §8.13 tasks to match §4.15:
- T56: "Implement dimension node + `describes_*` edge creation in WriteMemory handler"
- T57: "Dual-write: dimensions write both legacy JSON blob and new dimension nodes/edges"
- T58: "Retrieval adapter: dimension queries traverse `describes_*` edges"
- T59: (remove or repurpose — no `node_dimensions` table to cache)

---

## FINDING-A3-12 🟡 Important — §4.11 self-contradicts: claims "one new node_kind" but introduces four

**Check #4 (Consistent naming) + Check #3 (Dead definitions)**

§4.11's closing paragraph says:
> "Maps cleanly: one new `node_kind` (`anomaly_event`) beyond what the original draft proposed."

But §4.11 actually defines FOUR new node_kinds:
1. `node_kind='domain'` (under `node_type='interoceptive'`) — domain state
2. `node_kind='somatic_marker'` — persistent pattern associations
3. `node_kind='anomaly_event'` — anomaly persistence
4. `node_kind='regulation_policy'` — regulation decisions

Similarly, §4.12 adds 4 more: `drive`, `action_outcome`, `subscription`, `external_write`.
§4.13 adds 1: `wm_snapshot`.
§4.14 adds 2: `feedback` (metacog), `retrieval_trace`.
§4.15 adds 2: `dimension`, `tag`.
§4.16 adds 1: `knowledge_topic` (already in §4.4 as `topic`).

**Total new node_kinds across §4.11-§4.16: ~13**. These should be enumerated in §3.1's `node_kind` comment, which currently lists only: `'memory'|'entity'|'topic'|'insight'|'episode'|'plan'|...`. The `...` hides 13+ values.

**Suggested fix**: Expand §3.1's `node_kind` comment to include all values. Add a complete enumeration table similar to §3.2's `edge_kind` taxonomy table.

---

## FINDING-A3-13 🟡 Important — §4.15 Tier 2 dimension nodes create massive edge explosion for WriteMemory

**Check #33 (Simplification vs completeness) + Check #14 (Coupling)**

§4.15 writer path says: "dimensions enter as part of `WriteMemory` — a single op produces 1 memory node + up to ~15 dimension/tag edges + 0–15 new dimension nodes."

But §4.15.2 lists **10 narrative fields** × N values per field. For a typical memory:
- `participants`: 2-3 people → 2-3 edges + 0-3 new nodes
- `location`: 1 → 1 edge + 0-1 node
- `tags`: 5-10 → 5-10 edges + 0-10 nodes
- Other 8 narrative fields: ~1 each → ~8 edges + 0-8 nodes

Realistic total: **~20-25 edges + ~5-15 new nodes per memory ingest**. With §4.12's `WriteAlignmentEdge` (K per drive), a single memory ingest could produce **30-40 row writes** in one Batch.

This is not a design flaw per se, but the throughput analysis in §6.6 doesn't account for it. §6.6 models "per-op write cost" as one row insert. If one `WriteMemory` is actually 30-40 row inserts, the effective ceiling drops from ~11k ops/sec to ~275-367 memory ingests/sec. Still adequate for production (100/hour), but the throughput math is misleading.

**Suggested fix**: Update §6.6 to model `WriteMemory` as a compound op (~30 rows per ingest), not a single row. Adjust the throughput ceiling estimate accordingly.

---

## FINDING-A3-14 🟡 Important — §4.7 supersession doesn't address anomaly_event lifecycle

**Check #5 (State machine — no unreachable states)**

Task context asks: "when an anomaly is later resolved (false alarm), does it get superseded or just decayed?"

§4.7 (Supersession) covers: entity merge, memory correction, topic update. It does NOT mention anomaly_events.

§4.11 doesn't specify what happens when an anomaly is a false alarm. Options:
- Supersede (`anomaly_event.superseded_by = correction_node_id`) — audit trail preserved
- Soft-delete (`anomaly_event.deleted_at = now`) — lose the event
- Attribute update (`anomaly_event.attributes.resolved = true`) — cheapest
- Nothing — anomaly_events accumulate forever

Since somatic markers are derived from anomaly_events (§4.11: "when ≥N anomaly_events share a pattern signature"), unresolved false-alarm anomalies would pollute marker formation. This is a data-quality issue that compounds over time.

**Suggested fix**: Add to §4.11: "False-alarm resolution: an anomaly_event determined to be a false alarm is superseded via §4.7 (`superseded_by` pointing at a `node_kind='anomaly_resolution'` node with rationale). Superseded anomaly_events are excluded from somatic marker formation queries."

---

## FINDING-A3-15 🟢 Minor — §4.12 `node_kind='empathy_event'` in §8.10 T49 doesn't appear in §4.12 body

**Check #2 (References resolve)**

§8.10 T49 says: "events become `node_kind='empathy_event'`." But §4.12's body never defines `node_kind='empathy_event'`. §4.12 defines `action_outcome`, `drive`, `subscription`, `external_write` — but no generic `empathy_event` kind. Either T49 is using a stale name, or §4.12 forgot to define it.

**Suggested fix**: Clarify whether `empathy_event` is a distinct node_kind or an umbrella for the 4 kinds defined in §4.12. If the latter, T49 should enumerate which node_kinds it produces.

---

## FINDING-A3-16 🟢 Minor — §4.14 aggregate query uses bare `attributes.score` without JSON extraction syntax

**Check #22 (Missing helpers) + Check #29 (Ground truth verification)**

§4.14 shows: `SELECT AVG(attributes.score) FROM nodes WHERE node_kind='feedback' AND dimension='recall_accuracy'`

But `attributes` is a `TEXT` column containing JSON (per §3.1). SQLite requires `json_extract(attributes, '$.score')` or `attributes->>'$.score'` syntax. Similarly `dimension` is inside `attributes`, not a top-level column — should be `json_extract(attributes, '$.dimension')`.

Corrected query: `SELECT AVG(json_extract(attributes, '$.score')) FROM nodes WHERE node_kind='feedback' AND json_extract(attributes, '$.dimension') = 'recall_accuracy' AND created_at > ...`

This will also be slow without a generated column + index on `json_extract(attributes, '$.dimension')` for feedback nodes. The design should note whether a generated-column index is needed for metacog aggregation queries.

---

### ✅ Passed Checks

- **Check #0**: Document size — §4.11-§4.16 is 6 subsections within scope. Whole doc has 17 §4.x subsections + cross-sections, but §4.17 notes coverage closure. Not recommending split since subsections are cohesive. ✅
- **Check #1**: Types partially defined — anomaly_event, wm_snapshot, feedback have attribute schemas defined. ⚠️ (see FINDING-A3-1 for node_type/node_kind confusion, FINDING-A3-12 for incomplete enumeration)
- **Check #5**: State machine — §4.13 WM snapshot triggers are well-defined (demand-driven, not periodic). §4.11 anomaly threshold is specified (z-score). ✅ (see FINDING-A3-14 for anomaly lifecycle gap)
- **Check #7**: Error handling — §6.9 covers writer failures, batch rollback, panic recovery. ✅
- **Check #8**: String operations — no string slicing on user text in §4.11-§4.16. ✅
- **Check #9**: Integer overflow — anomaly `sample_count` and `coactivation_count` have no explicit bounds, but these are low-frequency counters. ✅
- **Check #10**: Option/None — no `.unwrap()` patterns in §4.11-§4.16 pseudocode. ✅
- **Check #13**: Separation of concerns — §4.12 cleanly separates substrate-resident patterns from I/O (file reads/writes). ✅
- **Check #15**: Configuration — z-score threshold (§4.11), MIN_SAMPLES (§R8), batch sizes are noted as tunables. ✅
- **Check #16**: API surface — `dimension_access.rs` shim (§4.15.4) preserves existing API. ✅
- **Check #17**: Goals/non-goals — §1.3 states goals. Non-goals implicit (not multi-process, not sharded). ✅
- **Check #18**: Trade-offs documented — §4.13 Option A/B/C analysis is excellent. §4.15.5 justifies edge indirection. ✅
- **Check #23**: Dependency assumptions — no external deps in §4.11-§4.16. ✅
- **Check #25**: Testability — §8.9-§8.13 tasks include specific test criteria (T48, T53). ✅
- **Check #26**: Duplicate functionality — verified `interoceptive/`, `bus/`, `metacognition.rs`, `dimensions.rs` exist as claimed. §4.11-§4.16 replaces them, not duplicates. ✅
- **Check #27**: API compatibility — §4.15.4 explicitly preserves `dimension_access.rs` API. ✅
- **Check #29**: Ground truth — verified: compiler/ has 21 modules ✓, zero external callers ✓, dimensions.rs is 1362 LoC ✓, dimension_access.rs is 237 LoC ✓. All claims match. ✅
- **Check #30**: Technical debt — §4.16.4 explicitly documents concept preservation and retirement timeline. No "clean up later" shortcuts. ✅
- **Check #31**: Shortcut detection — §4.11 two-tier design addresses root cause (high signal volume), not symptom. §4.13 Option C is a root-cause design (WM is transient; persist only when meaningful). ✅
- **Check #34**: Breaking-change risk — §4.16 confirms zero external callers before retirement. §4.15.4 preserves accessor API. ✅
- **Check #35**: Purpose alignment — all components trace to §0 TL;DR goals (unified substrate). No speculative flexibility. ✅

<!-- FINDINGS -->

## Applied

(None — awaiting human approval before apply phase.)
