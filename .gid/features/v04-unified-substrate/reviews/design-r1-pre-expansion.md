# Design Review r1 — v04-unified-substrate

> **Reviewer:** claude (sub-agent, coder)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md`
> **Reference:** `.gid/features/v03-wireup/design.md`
> **Method:** 36-check review-design skill, depth=full

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 5     |
| 🟡 Important  | 7     |
| 🟢 Minor      | 4     |
| **Total**  | **16**|

**Recommendation:** Needs fixes before implementation. The 5 critical findings (FINDING-1,2,3,5,6,7) block Phase A schema creation — the terminal schema as written would lose data, break FTS, and have ID type mismatches. The important findings block Phase B/C (dual-write atomicity, field mapping gaps, task sizing). None require fundamental redesign — the architecture is sound, but the schema and migration specs need a detail pass.

**Estimated implementation confidence:** Medium — architecture is correct, but field-level mappings need completion before any code is written.

---

## FINDING-1 🔴 Critical — Hebbian link semantics lost in migration mapping

**Check:** #29 (Ground truth verification) + #6 (Data flow completeness)
**Location:** §4.3 (Hebbian co-activation) + §3.2 (`edges` schema)

**Issue:** The design maps `hebbian_links → edges(kind='associative', predicate='co_activated')` and only preserves `weight`. But the actual `hebbian_links` table (verified in `storage.rs:364-375`) has **7 data-carrying columns** beyond the PK:

| Current column | Type | Mapped to `edges`? |
|---|---|---|
| `strength` | REAL | → `edges.weight` ✅ |
| `coactivation_count` | INTEGER | ❌ **LOST** |
| `temporal_forward` | INTEGER | ❌ **LOST** |
| `temporal_backward` | INTEGER | ❌ **LOST** |
| `direction` | TEXT | ❌ **LOST** |
| `signal_source` | TEXT | ❌ **LOST** |
| `signal_detail` | TEXT | ❌ **LOST** |
| `namespace` | TEXT | → `edges.namespace` ✅ |
| `created_at` | REAL | → `edges.created_at` ✅ |

These are not dead columns. Active code paths:
- `decay_hebbian_links_differential()` applies **different decay rates per `signal_source`** (corecall=0.95, multi=0.90, default=0.85). This is a root-cause-aware decay model, not a simple multiplier.
- `coactivation_count` is used to track link formation frequency.
- `temporal_forward/backward` and `direction` encode asymmetric temporal co-activation (A before B vs B before A).

The `edges` schema has no column for any of these. `edges.attributes` or `edges.summary` as JSON catch-all is not in the schema either — edges have no general-purpose JSON bag like `nodes.attributes`.

**Impact:** Backfill (Phase C, T24) would silently discard Hebbian signal differentiation. Post-migration decay would treat all associative edges identically, regressing the differential decay behavior.

**Suggested fix:**
1. Add `attributes TEXT NOT NULL DEFAULT '{}'` column to `edges` table (mirrors `nodes.attributes` pattern) for kind-specific fields.
2. Map: `signal_source → attributes.signal_source`, `coactivation_count → attributes.coactivation_count`, `temporal_forward/backward → attributes.temporal_forward/backward`, `direction → attributes.direction`.
3. Update §4.3 pseudocode and §5.3 backfill spec (T24) to include these fields.
4. Update §4.6 (decay) to note that `decay_hebbian_links_differential` must be preserved using the `attributes.signal_source` discriminator.

---

## FINDING-2 🔴 Critical — `memories.layer` column missing from `nodes` schema and migration

**Check:** #6 (Data flow completeness) + #29 (Ground truth verification)
**Location:** §3.1 (`nodes` schema), §5.3 (Phase C backfill), §6 Q7

**Issue:** The `memories` table (verified `storage.rs:340-357`) has a `layer TEXT NOT NULL` column with active enum values `core`, `working`, `archive` (verified `types.rs:138-158`, `MemoryLayer` enum). This field is **NOT NULL** and is a first-class query discriminator — Memory Chain Model uses it to partition memories by consolidation tier.

§6 Q7 acknowledges this column exists but marks it as an **open question** ("Becomes `nodes.attributes.layer` (JSON) or stays as a top-level column? Recommend top-level — add as `working_memory_layer TEXT`"). However:

1. The `nodes` table in §3.1 does **not** include `working_memory_layer` or any `layer` column.
2. The migration plan (§5.3) does not mention mapping `memories.layer` during backfill.
3. T19 (backfill: memories → nodes) has no spec for this field.

Since the question is "open", the current schema is **incomplete by its own admission**. But the bigger issue is that Q7 is **not actually open** — the recommendation is already made ("Recommend top-level"), yet the schema in §3.1 doesn't reflect it. This is a decision disguised as open (Check #30's territory).

`MemoryLayer` is used in:
- `association/candidate.rs:107` — candidate selection by layer
- `association/former.rs:136` — Hebbian formation filtered by layer
- `memory.rs` — consolidation, retrieval filtering, lifecycle transitions

**Impact:** Backfill would lose the layer discriminator. All memories would have no layer information post-migration, breaking the Memory Chain Model.

**Suggested fix:**
1. Add `layer TEXT` to `nodes` schema in §3.1 (nullable — only node_kind='memory' uses it).
2. Close Q7 as decided: top-level column `layer TEXT`.
3. Add layer mapping to T19 backfill spec: `memories.layer → nodes.layer`.

---

## FINDING-3 🔴 Critical — `memories.memory_type` (7-variant behavioral discriminator) not mapped

**Check:** #6 (Data flow completeness) + #29 (Ground truth verification)
**Location:** §3.1 (`nodes` schema), §4.1 (Memory ingest)

**Issue:** The `memories` table has `memory_type TEXT NOT NULL` with 7 active variants: `factual`, `episodic`, `relational`, `emotional`, `procedural`, `opinion`, `causal` (verified: `types.rs:78-93`). This is **not** `node_kind` — it's a sub-classification within `node_kind='memory'`.

`MemoryType` drives behavioral logic:
- `MemoryType::default_importance()` — per-type importance defaults (Emotional=0.9, Factual=0.3)
- `MemoryType::default_decay_rate()` — per-type decay rates (Emotional=0.01, Episodic=0.10)
- Used in `store_raw` for importance/decay initialization
- Used in `storage.rs` for type-filtered queries (`WHERE memory_type = ?`)

The `nodes` schema has no `memory_type` column. The design mentions `node_kind='memory'` but never addresses the sub-type. It's not in `attributes` either — no mapping is specified.

**Impact:** After migration, all memories would lose their type classification. Decay, importance defaults, and type-filtered retrieval would break. There is no `idx_memories_type` equivalent in the nodes indexes.

**Suggested fix:**
1. Either: add `memory_type TEXT` to `nodes` schema (like `layer`), or
2. Specify explicitly that `memory_type` maps to `nodes.attributes.memory_type` in JSON, with an index: `CREATE INDEX idx_nodes_memory_type ON nodes(json_extract(attributes, '$.memory_type')) WHERE node_kind='memory'`.
3. Update §4.1 pseudocode to include memory_type in the INSERT.
4. Update backfill T19 to map this field.

---

## FINDING-4 🟢 Minor — §4.2 cross-reference error: "§6 Q3" should be "§6 Q2"

**Check:** #2 (Every reference resolves)
**Location:** §4.2 (Entity resolution)

**Issue:** §4.2 says: "(Or kept inline in `nodes.attributes` JSON — decision in §6 Q3.)" But §6 Q3 is about UNIQUE constraints on edges. The alias storage decision is addressed in §6 **Q2** ("Entity aliases: inline JSON vs. dedicated alias nodes?").

**Suggested fix:** Change "§6 Q3" → "§6 Q2" in §4.2.

---

## FINDING-5 🔴 Critical — §4.3 Hebbian upsert references non-existent UNIQUE constraint

**Check:** #5 (State machine invariants — guard conditions) + #22 (Missing helpers)
**Location:** §4.3 (Hebbian co-activation) + §3.2 (`edges` schema)

**Issue:** §4.3 pseudocode uses:
```sql
ON CONFLICT (source_id, target_id, edge_kind, predicate)
DO UPDATE SET weight=weight+delta, recorded_at=now;
```

But the `edges` table in §3.2 has **no UNIQUE constraint** on these columns. The only uniqueness constraint is `id BLOB PRIMARY KEY`. The `ON CONFLICT` clause would silently become a plain INSERT (no conflict would ever trigger), creating duplicate associative edges instead of incrementing weight.

§4.3 itself acknowledges this: "(Need UNIQUE constraint on `(source_id, target_id, edge_kind, predicate)` for associative+containment kinds.)" — but this is stated as a parenthetical aside, and §6 Q3 marks it as "Open." Meanwhile, the pseudocode **depends** on this constraint existing.

This is a **schema-pseudocode inconsistency**: the schema and the cognitive function mapping disagree.

Additionally, the current `hebbian_links` PK is `(source_id, target_id)` — no `edge_kind`/`predicate` discriminator needed because the table IS the discriminator. In the unified schema, without the UNIQUE constraint, the Hebbian upsert pattern is broken.

**Impact:** Without resolving this before implementation, T14 (Hebbian dual-write) would either fail or silently create duplicate edges.

**Suggested fix:**
1. Close Q3 as a **mandatory** decision before Phase A. This is not optional — it blocks T06 (edges table creation) and T14 (Hebbian dual-write).
2. Recommended: add a **partial UNIQUE index** for associative edges:
   ```sql
   CREATE UNIQUE INDEX idx_edges_assoc_unique
     ON edges(source_id, target_id, edge_kind, predicate)
     WHERE edge_kind = 'associative';
   ```
3. Add a similar partial unique index for containment edges if upsert is needed there.
4. Update §3.2 to include these indexes.

---

## FINDING-6 🔴 Critical — FTS5 `content_rowid='rowid'` incompatible with BLOB PRIMARY KEY

**Check:** #29 (Ground truth verification) + Risk R3
**Location:** §3.3 (`nodes_fts`), §3.1 (`nodes`)

**Issue:** The `nodes_fts` definition uses:
```sql
CREATE VIRTUAL TABLE nodes_fts USING fts5(
    content, summary,
    content_table='nodes', content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);
```

SQLite FTS5 content-sync mode (`content_table`) requires `content_rowid` to reference an INTEGER column. When a table has `id BLOB PRIMARY KEY`, SQLite does **not** alias `rowid` to the PK — the table becomes a WITHOUT ROWID table internally (or the implicit rowid is unstable across VACUUM). FTS5 content-sync mode relies on stable integer rowids to map FTS results back to the source table.

With a BLOB PK, either:
1. The table has no stable implicit rowid (WITHOUT ROWID), making content-sync impossible, or
2. The implicit rowid exists but is not the PK, so deletions + VACUUMs can reassign rowids, corrupting FTS ↔ node mapping.

The current `memories` table uses `id TEXT PRIMARY KEY` with `memories_fts` — same problem class, but the current code may work by luck (no VACUUM) or by using external-content mode differently.

**Impact:** FTS queries would return wrong nodes or crash after VACUUM. This is a correctness bug in the terminal schema.

**Suggested fix:**
1. Add an `INTEGER` auto-increment column to `nodes` for FTS rowid stability: `rowid_stable INTEGER NOT NULL UNIQUE` (or use SQLite's implicit rowid by making `id BLOB` a regular UNIQUE column, not PRIMARY KEY, and letting the implicit INTEGER rowid be the true PK).
2. Alternative: use FTS5 external-content mode with manual trigger-based sync keyed on the UUID, avoiding rowid dependency entirely.
3. Verify how the existing `memories_fts` works in the codebase to match the pattern.

---

## FINDING-7 🔴 Critical — Node ID type mismatch: `nodes.id` is BLOB but all existing IDs are TEXT strings

**Check:** #29 (Ground truth verification) + #27 (API compatibility)
**Location:** §3.1 (`nodes`), §3.2 (`edges`), §5.3 (Phase C backfill)

**Issue:** The `nodes` schema defines `id BLOB PRIMARY KEY` (16-byte UUID). But:
- `memories.id` is `TEXT PRIMARY KEY` (string UUID like `"550e8400-..."`)
- `entities.id` is `TEXT PRIMARY KEY`
- All edge FKs in legacy tables (`memory_entities.memory_id`, `entity_relations.source_id`, etc.) are TEXT
- All existing Rust code uses `String` for memory IDs (verified: `MemoryRecord.id: String` in types.rs)

The backfill (Phase C) must convert TEXT UUIDs → 16-byte BLOB UUIDs. This is a lossy direction: BLOB UUIDs don't round-trip through the existing Rust `String`-based API without explicit conversion at every boundary.

During dual-write (Phase B), `store_raw` would need to generate BOTH a TEXT id (for legacy) and a BLOB id (for unified), maintain a mapping, and ensure all edge references use the correct format. This is not specified anywhere.

**Impact:** Phase B dual-write and Phase C backfill are under-specified. The ID format change propagates to every FK, every edge, every retrieval adapter, and every public API boundary.

**Suggested fix:**
1. Either: keep `nodes.id` as `TEXT PRIMARY KEY` (matching current convention) — simplest migration path, or
2. If BLOB UUID is desired for performance: add an explicit ID migration strategy to §5, specify the TEXT→BLOB conversion function, and document every Rust API boundary that needs adaptation.
3. Add this as a required decision in §6 (currently missing).

---

## FINDING-8 🟡 Important — §5.2 dual-write "in one transaction" impossible across separate DB files

**Check:** #21 (Ambiguous prose) + #23 (Dependency assumptions)
**Location:** §5.2 (Phase B — dual-write)

**Issue:** §5.2 says: "`store_raw` writes to `memories` AND `nodes` in one transaction." But §6 Q1 leaves open whether unified tables are in the same DB file. If they're separate (as v0.3 bench harness uses), SQLite cannot provide atomic cross-file transactions without ATTACH — and even with ATTACH, atomicity guarantees are weaker (not crash-safe across files).

Even if Q1 is resolved as "single file," the current codebase may use separate connections or separate `SqliteStorage` instances for legacy vs. graph tables. True single-transaction dual-write requires both table sets on the same connection.

**Impact:** If dual-write isn't truly atomic, a crash between legacy write and unified write creates divergence. The "row-count parity" acceptance criterion (§5.2) would catch this eventually, but not prevent it.

**Suggested fix:**
1. Close Q1 before Phase B design. If single-file: document that both legacy and unified tables share one `rusqlite::Connection`.
2. If separate files: replace "one transaction" with "best-effort dual-write with reconciliation" and add a reconciliation step to T17 (parity test).

---

## FINDING-9 🟡 Important — `synthesis_provenance` columns lost in migration mapping

**Check:** #6 (Data flow completeness) + #29 (Ground truth)
**Location:** §4.5 (Synthesis/insights), §5.3 (Phase C backfill T25)

**Issue:** The `synthesis_provenance` table (verified `storage.rs:440-452`) has columns: `id, insight_id, source_id, cluster_id, synthesis_timestamp, gate_decision, gate_scores, confidence, created_at`. §4.5 maps only `source_id → target_id` and `confidence → confidence`. Lost: `cluster_id`, `gate_decision`, `gate_scores`, `synthesis_timestamp`.

These record the decision process (which gate passed, scores) — provenance of provenance. Without them, the audit trail of synthesis decisions is lost.

**Suggested fix:** Map `gate_decision`, `gate_scores`, `cluster_id` into edge attributes (requires FINDING-1's `attributes` column on edges) or keep `synthesis_provenance` as an audit table (like `pipeline_runs`).

---

## FINDING-10 🟡 Important — `memory_entities.role` column not mapped to edges

**Check:** #6 (Data flow completeness)
**Location:** §4.1/§4.2, §5.3 (T23)

**Issue:** `memory_entities` has a `role TEXT NOT NULL DEFAULT 'mention'` column. The design maps these to `edges(kind='provenance', predicate='mentions')` but doesn't map the `role` value. If role varies (e.g., 'subject', 'object', 'mention'), this semantic is lost.

**Suggested fix:** Map `memory_entities.role` → `edges.predicate` (so role='mention' → predicate='mentions', role='subject' → predicate='subject_of', etc.) or to edge attributes.

---

## FINDING-11 🟡 Important — §7 T26 violates ≤300-line sub-agent rule

**Check:** #25 (Testability) + task sizing
**Location:** §7.4, T26

**Issue:** T26 is "triple extraction on historical memories (~24k Haiku calls, batched, resumable)". This is not a ≤300-line sub-agent task — it's an operational job requiring rate limiting, resumability, error handling, cost monitoring, and multi-hour wall-clock time. It's also the only task with external API cost (~$25).

**Suggested fix:** Split T26 into:
- T26a: Write the backfill driver (code, ≤300 lines) — resumable batch processor with checkpoint
- T26b: Dry-run on 100 memories, validate output, extrapolate cost (operational)
- T26c: Full production run (operational, not sub-agent — human-supervised)

---

## FINDING-12 🟡 Important — §4.11 "no counter-examples" claim is premature

**Check:** #33 (Simplification vs completeness) + task-specific request
**Location:** §4.11

**Issue:** §4.11 asserts "Every active cognitive function maps cleanly. The substrate is sufficient." But several near-future cognitive patterns don't map cleanly:

1. **Working memory session scope**: The current `session_wm.rs` (file exists in codebase) manages session-scoped working memory. This is a *volatile* layer that shouldn't persist to `nodes` at all — but the unified schema has no concept of "volatile vs persistent" nodes. Adding `volatile: bool` or handling this via a separate in-memory store needs addressing.

2. **Dream/consolidation reactivation**: Memory consolidation (sleep-like replay) would need to reactivate nodes and form new edges in batch. The schema supports this, but the design doesn't specify how batch-reactivation avoids creating duplicate associative edges (ties back to FINDING-5's missing UNIQUE constraint).

3. **Drive alignment / goal tracking**: If plans or goals become first-class nodes (`node_kind='plan'`), they need a completion/outcome status that doesn't fit the retirement model (`deleted_at`/`superseded_by`). A goal can be "completed" without being "deleted" or "superseded."

These aren't blocking, but the claim should be softened or these cases should be explicitly listed as "validated for future fit."

**Suggested fix:** Replace "No reverse counter-examples found" with "No counter-examples found for current functions. Near-future extensions (session WM volatility, batch consolidation, goal completion status) verified compatible with minor additions."

---

## FINDING-13 🟡 Important — `nodes.superseded_by` semantics mismatch with current code

**Check:** #29 (Ground truth verification)
**Location:** §3.1, §4.7

**Issue:** Current `memories.superseded_by` is `TEXT DEFAULT ''` (verified `storage.rs:280`). All existing queries filter with `(superseded_by IS NULL OR superseded_by = '')` (verified `storage.rs:1066,1089,1298,1310`). The unified `nodes.superseded_by` is `BLOB REFERENCES nodes(id)` — a typed FK, nullable, no empty-string case.

This means:
1. Backfill must convert empty-string superseded_by to NULL (not just copy).
2. All retrieval queries change from `(superseded_by IS NULL OR superseded_by = '')` to just `superseded_by IS NULL`.
3. The FK constraint means you can't set superseded_by until the superseding node exists — ordering constraint during backfill.

**Suggested fix:** Document the empty-string → NULL conversion in T19 backfill spec. Verify no code depends on distinguishing NULL vs empty-string superseded_by.

---

## FINDING-14 🟢 Minor — §6 Q7 is a decision disguised as "open"

**Check:** #30 (Technical debt — decisions disguised as open)
**Location:** §6 Q7

**Issue:** Q7 asks whether `memories.layer` should be a top-level column or JSON attribute, then says "Recommend top-level — add as `working_memory_layer TEXT`." The recommendation is clear, the rationale is given (queried often), and no counter-argument is presented. This is a decided question presented as open, which delays implementation.

**Suggested fix:** Close Q7. Add the column to §3.1 schema. (Subsumed by FINDING-2's fix.)

---

## FINDING-15 🟢 Minor — Dead definition: `predicate_kind` column has no usage in any cognitive function

**Check:** #3 (No dead definitions)
**Location:** §3.2 (`edges.predicate_kind`)

**Issue:** `edges.predicate_kind TEXT NOT NULL DEFAULT 'canonical'` with values `'canonical'|'proposed'`. This column is never referenced in any cognitive function mapping (§4.1–§4.10), any migration step (§5), or any open question (§6). No existing table has this concept.

If this is for future use (proposed predicates awaiting confirmation), it should be documented. Otherwise it's speculative flexibility.

**Suggested fix:** Either document the use case for `predicate_kind` in §4 or remove it from the schema to avoid YAGNI.

---

## FINDING-16 🟢 Minor — `entity_relations.metadata` not mapped in backfill

**Check:** #6 (Data flow completeness)
**Location:** §5.3 (T22 — backfill entity_relations → edges)

**Issue:** `entity_relations` has a `metadata TEXT` column (verified `storage.rs:413`). No mapping specified for T22 backfill. Also `entities.metadata` and `entities.entity_type` need explicit mapping — `entity_type` is a classification that should map to `nodes.attributes.entity_type`.

**Suggested fix:** Specify field-level mapping for T21 and T22 in §5.3, including metadata and entity_type.

---

<!-- FINDINGS -->

## ✅ Passed Checks

- **Check #0**: Document size — 5 components (§3.1–3.5), under 8 limit ✅
- **Check #1**: Types fully defined — SQL schemas complete; Rust types deferred to T10 (acceptable) ✅
- **Check #2**: References resolve — all §-references valid except FINDING-4 (minor) ✅ (with caveat)
- **Check #4**: Consistent naming — `node_kind`/`edge_kind` consistent throughout; snake_case SQL, CamelCase Rust ✅
- **Check #7**: Error handling — migration phases have explicit acceptance criteria and rollback ✅
- **Check #8**: String operations — no string slicing on user text in schema/pseudocode ✅
- **Check #9**: Integer overflow — `consolidation_count` unbounded but acceptable (monotonic counter) ✅
- **Check #10**: Option/None — CHECK constraints defined for bounded fields; NULL semantics documented ✅
- **Check #11**: Match exhaustiveness — `edge_kind` taxonomy is open (TEXT, not enum) so no exhaustiveness issue ✅
- **Check #12**: Ordering sensitivity — N/A (no match chains in design pseudocode) ✅
- **Check #13**: Separation of concerns — pure schema + ops mapping, no IO mixing ✅
- **Check #14**: Coupling — events carry observed data; edges carry source_run_id provenance correctly ✅
- **Check #15**: Configuration vs hardcoding — `MemoryConfig::unified_substrate` flag for gradual rollout ✅
- **Check #16**: API surface — minimal: nodes + edges + 2 extensions, no unnecessary public surface ✅
- **Check #17**: Goals explicit — §1.2 pain, §1.3 gains, non-goals implicit in §7 "out of scope" ✅
- **Check #18**: Trade-offs documented — wide-table + NULL strategy justified; single-vs-split DB discussed ✅
- **Check #19**: Cross-cutting — decay, retirement, provenance addressed; performance noted in R5 ✅
- **Check #20**: Appropriate abstraction — SQL + pseudocode at right level for schema design ✅
- **Check #24**: Migration path — 6-phase (A–F) with explicit acceptance criteria per phase ✅
- **Check #26**: Existing functionality — design explicitly consolidates existing tables, not duplicating ✅
- **Check #28**: Feature flag — `unified_substrate: bool` flag for gradual rollout in Phase D ✅
- **Check #31**: Shortcut detection — design addresses root cause (schema sprawl), not symptoms ✅
- **Check #34**: Breaking-change risk — Phase D parity campaign + 1-week production observation ✅
- **Check #35**: Purpose alignment — all components serve stated consolidation goal ✅

## Applied

### Round 1 (2026-05-12, all 16 review findings)

🔴 Critical (5):
- **FIN-1** — `edges.attributes TEXT NOT NULL DEFAULT '{}'` added (L212); preserves hebbian 6 sub-fields + synthesis 3 gate fields
- **FIN-2** — `nodes.layer TEXT` added (L119) for memories
- **FIN-3** — `nodes.memory_type TEXT` added (L120) + partial index `idx_nodes_memory_type` (L182)
- **FIN-5** — 2 partial UNIQUE indices created in §3.2 (associative + containment)
- **FIN-6** — §3.3 FTS5 switched to external-content + manual triggers, keyed on `nodes.id` (TEXT UUID); no rowid coupling
- **FIN-7** — `nodes.id` / `edges.id` / FK columns switched to TEXT (zero API boundary churn)

🟡 Important (5):
- **FIN-8** — §5.2 "Atomicity prerequisite" note added (L561): Q1 must close as single-file DB before Phase B
- **FIN-9** — `synthesis_provenance` columns mapped into `edges.attributes` JSON
- **FIN-10** — `memory_entities.role` mapped to `edges.attributes`
- **FIN-11** — §7 T26 split into T26a/T26b/T26c (driver / dry-run / supervised production)
- **FIN-12** — §4.11 weakened to soft claim; explicitly addresses session WM, batch reactivation, goal completion
- **FIN-13** — `superseded_by` empty→NULL handling + two-pass backfill ordering written into §5.2

🟢 Minor (5): all applied (cross-ref fixes, naming clarifications, redundancy removals)

**Caveat surfaced during apply (not in original review):** Pre-existing duplicate rows in `hebbian_links` / `cluster_assignments` will break partial UNIQUE index creation during Phase A. Must dedupe during T24 backfill — to be specified in T24 task.

### Round 2 (2026-05-12, self-review findings)

🔴 Critical (1):
- **FIN-17** — FIN-7 missed nullable reference columns. `source_run_id` and `episode_id` switched from BLOB → TEXT in both `nodes` (L159-160) and `edges` (L228-229).

🟡 Important (2):
- **FIN-18** — §4.10 stale cross-ref `§6 Q6` → `§6 Q4` (Q6 renumbered after Q7 removal); added BLOB→TEXT note
- **FIN-19** — Risks §R3 reframed: FTS rowid volatility ✅ mitigated by §3.3 external-content design choice (no rowid coupling)

🟢 Minor (1):
- **FIN-20** — §4.3 added inline `-- NOTE:` explaining ON CONFLICT works against partial UNIQUE index because inserted rows satisfy the WHERE predicate

### Status

Design doc at 963 lines, 10 H2 + 38 H3 sections (§6 expanded into 7 closed sub-decisions). Schema internally consistent, cross-refs correct, risks aligned with design choices.

### Round 3 (2026-05-12, §6 open questions closed via first-principles cognitive-substrate framing)

All 7 questions in §6 resolved. Reasoning grounded in engram thesis: *patterns of neural activation → graph; bookkeeping about patterns → audit table*.

- **§6.1 Q1** — Single DB file. SQLite ATTACH doesn't give true cross-DB atomicity → Phase B fan-out requires single file.
- **§6.2 Q2** — Surface forms are nodes (not inline JSON aliases). Entity resolution is a graph problem (find `same_as` connected component), not a column lookup. Mirrors cortical lexical-access ↔ semantic-memory separation.
- **§6.3 Q3** — Partial UNIQUE indexes (already in §3.2).
- **§6.4 Q4** — Episode-as-node only. `episode_id` columns DROPPED from `nodes` and `edges`. Episode is a hippocampal binding, not a label. Dual-write (column + edge) = forever technical debt. §3.1 schema, §4.1 ingest pseudocode, §4.10, T39 all updated.
- **§6.5 Q5** — Promotion candidates stay as audit table. Candidate is the promoter's working memory, not a cognitive entity. Separation of concerns: substrate stores cognitive state, audit stores algorithmic decisions about that state.
- **§6.6 Q6** — Drop `triples` table in Phase F (0 rows, dead schema).
- **§6.7 Q7** — Legacy reader during Phase B with **hard exit criteria**: 7 days zero invariant violations + shadow-read parity ≥99% Jaccard@K=20 on 95% of queries + LoCoMo J-score within ±1pp of pre-Phase-B baseline. Phase B is a verification window, not indefinite.

### Schema impact summary (Round 3)

- `nodes.episode_id TEXT` (L160) — REMOVED
- `edges.episode_id TEXT` (L229) — REMOVED
- `idx_nodes_episode` partial index — REMOVED
- `nodes.episode_id` parameter in §4.1 ingest pseudocode — REMOVED, replaced with explicit containment-edge INSERT
- §4.10 rewritten: dropped column, not retained; migration via Phase C backfill creates episode nodes + containment edges
- §4.2 rewritten: surface-form-as-node framing replaces "alias inline JSON vs node" hedge
- T39 (Phase F) expanded: explicitly drops `triples` table + `episode_id` columns + `entity_aliases` legacy table

---

## FINDING-21 🟡 Important — `nodes.layer` is v0.2 ontological residue; should be derived, not stored (defer to v04.1)

**Check:** #29 (Ground truth verification) + #33 (Simplification vs completeness) + first-principles
**Location:** §3.1 `nodes.layer TEXT` (L119); follow-up to FIN-2

**Issue:** FIN-2 correctly added `layer TEXT` to the `nodes` schema so the migration doesn't lose data. The fix is right for v04 scope. But it surfaces a deeper question: **is `layer ∈ {core, working, archive}` semantically coherent in a graph-native substrate?**

First-principles argument that `layer` is residue:

1. **In cortex, "working memory" is not a separate substrate.** It is *persistent activation* of the same neurons that hold long-term knowledge in cortex. There is no "working memory store." There is an activation state. `MemoryLayer` was a v0.2 storage-tier abstraction (different tables/retention policies) — the whole v04 thesis is to *dissolve* storage tiers into one graph.

2. **The three categories are regions of a continuous space, not categorical types.** The actual cognitive distinctions are:
   - working ≈ high activation_level AND recently accessed AND capacity-limited
   - core   ≈ high access_count AND moderate activation AND long retention
   - archive ≈ low activation AND long time-since-access

   These are *queries over (last_accessed_at, access_count, activation_level)*, not states a memory "is in."

3. **Storing `layer` as a column means we've discretized a continuous variable and frozen it.** Every read/write path must remember to update `layer` when activation crosses a threshold. This is exactly the kind of bookkeeping-disguised-as-state we eliminated in §6.4 (episode_id) and §6.5 (promotion_candidates as audit, not substrate).

4. **Layer is used by retrieval (`association/candidate.rs:107`, `association/former.rs:136`).** Switching to derived would require these call sites to query by `(last_accessed_at, access_count, activation_level)` thresholds instead of `layer = 'core'`. Tractable but non-trivial.

**Why this stays in v04 as a column (FIN-2's fix is correct):**

- Phase B (dual-write) cannot simultaneously rewrite retrieval semantics. Code currently reads `MemoryLayer`. Schema must round-trip it.
- Dropping `layer` requires introducing `access_count INTEGER` and explicit `activation_level REAL` on `nodes`. Neither exists cleanly in the current substrate.
- Migration risk: breaks Memory Chain Model behavior if layer-equivalent derived predicates don't exactly reproduce current routing.

**Recommended action (NOT in v04 scope):**

- **v04.1 sub-feature: deprecate-layer.** Add `access_count` + `activation_level` to nodes. Introduce `node_layer_view` (SQL view or scalar function) that derives layer from those + `last_accessed_at`. Migrate retrieval call sites to use the view. Drop `nodes.layer` column.
- File as separate feature/issue after v04 lands and parity-soak (§5 Phase D) confirms no regressions.

**Status:** Not blocking. Documenting the asymmetry between v04's "graph is the substrate" thesis and the persistence of `layer` as a frozen categorical. v04 ships with `layer`; v04.1 dissolves it.
