# Design Review r2 Part 2 — Cognitive Ops + Migration (§4.1-4.10, §4.17, §5)

> **Reviewer:** sub-agent (coder)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` lines 370-558, 855-981
> **Method:** full-depth review, focused on cognitive op mappings + 6-phase migration plan

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 1     |
| 🟡 Important  | 8     |
| 🟢 Minor      | 5     |
| **Total**  | **14**|

**Recommendation:** Needs fixes before implementation. One critical (backfill idempotency) must be resolved; 7 important findings affect migration safety and spec clarity.

---

## FINDING-A2-1 🟢 Minor — §4.1 ISS-103 `created_at` vs `occurred_at` split not explicitly called out

**Location:** §4.1 Memory ingest, pseudocode block
**Issue:** The unified INSERT lists both `occurred_at` and `created_at` as columns, which is correct. However, §4.1 does NOT explicitly state the ISS-103 invariant: "`created_at` is ALWAYS wall-clock `Utc::now()` at ingest time; `occurred_at` is the optional caller-supplied event time." The pseudocode just lists them as columns with no comment about which drives decay. Cross-referencing §4.6 (Decay), which correctly says "reads `nodes.created_at`" — so the invariant is maintained in practice, but §4.1 should state it at the point of write so an implementer doesn't accidentally swap them.

Verified in source: `memory.rs:2477` — `let created_at = Utc::now()` is always wall-clock. The design's pseudocode is consistent but **under-specified** at the write site.

**Suggested fix:** Add a comment in §4.1's INSERT pseudocode:
```
-- ISS-103: created_at = wall-clock NOW (drives decay); occurred_at = caller event time (drives temporal grounding)
```

---

## FINDING-A2-2 🟡 Important — §4.3 Hebbian pseudocode has malformed SQL and inconsistent `predicate` value

**Location:** §4.3 Hebbian co-activation, pseudocode block
**Issue:** Two problems:

1. **Malformed SQL**: The pseudocode starts with a bare `INSERT INTO edges (source_id=A, target_id=B,` on one line, then a completely separate `INSERT INTO edges (...)` with `VALUES(...)` on the next. The first INSERT is incomplete — it has no closing paren, no VALUES clause, and is syntactically garbage. This looks like a draft remnant that was never cleaned up.

2. **Missing predicate value**: The VALUES clause uses `predicate` as a positional parameter but the ON CONFLICT clause targets `(source_id, target_id, edge_kind, predicate)`. However, the INSERT never specifies what `predicate` value to use. §3.2's edge_kind taxonomy says associative edges use `predicate='co_activated'`, but §4.3's pseudocode leaves `predicate` as a bare identifier rather than a concrete value like `'co_activated'`.

3. **`relation='hebbian'` claim in task description**: The task brief asks whether `relation='hebbian'` is consistent with §3.2. Looking at §3.2, associative edges use `predicate='co_activated'`, not `relation='hebbian'`. §4.3 doesn't mention `relation='hebbian'` at all — the task brief's reference is stale. The design is internally consistent (both §3.2 and §4.3 use `predicate` not `relation`), but the pseudocode needs the concrete value.

**Suggested fix:** Replace the §4.3 pseudocode with a single clean INSERT...ON CONFLICT statement using `predicate='co_activated'` explicitly.

---

## FINDING-A2-3 🟢 Minor — §4.3 doesn't cross-reference §6 writer queue for Hebbian atomicity

**Location:** §4.3 Hebbian co-activation
**Issue:** §4.3 specifies the SQL upsert but doesn't mention that Hebbian writes go through the §6 writer queue (`BumpAssociation` op). §6.1 defines `BumpAssociation { from_id, to_id, delta }` and §6.3 specifies Hebbian coalescing in the writer. §4.3 should cross-reference this — an implementer reading §4.3 alone would write direct SQL, bypassing the queue.

**Suggested fix:** Add a note: "Hebbian upserts are enqueued as `BumpAssociation` ops (§6.1, §6.3) and coalesced by the writer — not executed directly."

---

## FINDING-A2-4 🟢 Minor — §4.4 KC doesn't mention ISS-111 or EmbeddingInfomapClusterer

**Location:** §4.4 Knowledge compilation
**Issue:** §4.4 is a 5-line stub: "writes `knowledge_topics` + `cluster_assignments` → unified INSERT nodes/edges." It does NOT mention ISS-111 (clusterer degeneration on single-domain corpora), nor does it acknowledge whether the v0.3 `EmbeddingInfomapClusterer` is the assumed clusterer. §4.16.3 later covers ISS-111 in detail, but §4.4 itself—the section a KC implementer would read first—is silent. §4.4 notes "ISS-109 (clusterer collapse) becomes a tuning problem on the unified substrate" but ISS-109 is about `knowledge_topics` having 0 rows (never populated); ISS-111 is the actual clusterer bug. These are different issues.

**Suggested fix:** Add to §4.4: "Clusterer is v0.3 `EmbeddingInfomapClusterer` — see §4.16.3 for ISS-111 (degeneration on single-domain corpora), which is orthogonal to this substrate mapping."

---

## FINDING-A2-5 🟡 Important — §4.7 Supersession doesn't specify how retrieval skips superseded nodes

**Location:** §4.7 Supersession / correction
**Issue:** §4.7 defines the supersession mechanism (`nodes.superseded_by`, `edges.supersedes + invalidated_at/by`) but does NOT specify how retrieval filters out superseded nodes. Current code uses `WHERE superseded_by IS NULL OR superseded_by = ''` (verified: `storage.rs:1066`). The unified schema should use `WHERE superseded_by IS NULL` only (since §5.3 backfill converts `''` → `NULL`). But §4.7 doesn't state this, and §4.8 (retrieval plans) doesn't mention supersession filtering at all. An implementer rewriting retrieval against the unified substrate could miss this filter entirely.

**Suggested fix:** Add to §4.7: "Retrieval invariant: all plan queries MUST include `WHERE superseded_by IS NULL` (GUARD-3). Post-backfill, empty-string `''` is normalized to NULL — no OR clause needed."

---

## FINDING-A2-6 🟢 Minor — §4.8 claims "8 plans" but codebase has 7

**Location:** §4.8 Retrieval plans
**Issue:** §4.8 says "8 plans + 5 adapters". Verified: `retrieval/plans/` has 7 files (abstract_l5, affective, associative, bitemporal, episodic, factual, hybrid). `retrieval/adapters/` has 5 files + mod.rs. The plan count is off by one. Minor but could confuse an implementer doing a completeness audit.

**Suggested fix:** Correct to "7 plans + 5 adapters" or identify the 8th plan.

---

## FINDING-A2-7 🟡 Important — §4.6 Decay doesn't explicitly state it uses `created_at` (not `occurred_at`)

**Location:** §4.6 Decay / forget
**Issue:** §4.6 says "identical logic, reads `nodes.created_at`" — this is correct per ISS-103. But the section doesn't call out the ISS-103 invariant explicitly or explain WHY `created_at` and not `occurred_at`. Given that ISS-103 was a critical bug (historical content auto-deleted within hours), this invariant deserves a bold callout, not a passing mention. A future implementer unfamiliar with ISS-103 might "optimize" by using `occurred_at` for temporal relevance.

**Suggested fix:** Add: "**ISS-103 invariant**: decay MUST use `created_at` (wall-clock ingest time), NEVER `occurred_at` (event time). Using `occurred_at` causes historical-content ingests to appear years old and auto-delete within hours."

---

## FINDING-A2-8 🟡 Important — §5.2 Phase B dual-write atomicity is under-specified for Hebbian

**Location:** §5.2 Phase B, item 6
**Issue:** §5.2 says "Hebbian writes to `hebbian_links` AND `edges`." But §6.3 specifies Hebbian coalescing in the writer queue — `BumpAssociation` ops are coalesced into a single edge upsert. During Phase B dual-write, the writer must also write the un-coalesced individual bumps to `hebbian_links` (which has no coalescing). This semantic mismatch is not addressed: does the writer apply coalescing to both targets, or only to `edges`? If coalescing applies to both, `hebbian_links.strength` will diverge from legacy behavior (where each bump was an individual UPDATE). If only to `edges`, the writer needs two different code paths per bump.

**Suggested fix:** Specify: during Phase B, Hebbian coalescing applies ONLY to the unified `edges` target. Legacy `hebbian_links` receives individual un-coalesced bumps to preserve byte-exact parity with pre-v04 behavior.

---

## FINDING-A2-9 🔴 Critical — §5.3 Phase C backfill has no idempotency mechanism

**Location:** §5.3 Phase C, backfill driver
**Issue:** The backfill maps legacy rows → unified rows but specifies no deterministic ID derivation. If backfill crashes mid-run and restarts, re-inserting the same legacy row would fail on PK conflict (if `nodes.id = memories.id`, same ID) OR create duplicates (if new UUIDs generated). For `memories → nodes` this is OK (IDs preserved: `memories.id → nodes.id`), but for `entity_relations → edges` and `memory_entities → edges` and `hebbian_links → edges`, the design gives no guidance on how edge IDs are derived. If edge IDs are random UUIDs, re-running backfill creates duplicate edges. If they're deterministic (e.g., hash of source_id+target_id+edge_kind+predicate), the UNIQUE partial index handles associative/containment dedup but NOT structural or provenance edges (which have no UNIQUE constraint per §3.2).

§5.3 says the triple extraction backfill (T26) is "independently restartable" but doesn't say HOW. The main backfill driver items (T19-T25) don't mention restartability at all.

**Suggested fix:** Specify deterministic edge ID derivation for all backfill edges: `edge_id = sha256(source_table + legacy_pk)` or similar. For tables without unique legacy PKs (like `memory_entities` which may use composite keys), specify the composite. Add `INSERT OR IGNORE` / `ON CONFLICT DO NOTHING` to make re-runs idempotent.

---

## FINDING-A2-10 🟡 Important — §5.4 Phase D doesn't list read paths in switch order or rollback per path

**Location:** §5.4 Phase D — switch reads
**Issue:** The task brief asks: "'one plan at a time' — list of read paths in switch order? Each path has a rollback?" §5.4 specifies a single `MemoryConfig::unified_substrate: bool` flag that flips ALL retrieval adapters at once — not "one plan at a time." There is no per-plan rollback mechanism. If one plan regresses on the unified substrate but others improve, the flag is all-or-nothing. This contradicts the "every step is reversible" principle stated at the top of §5.

**Suggested fix:** Either (a) change to per-plan flags (`unified_substrate_episodic`, `unified_substrate_factual`, etc.) to enable incremental switch with per-plan rollback, or (b) acknowledge the all-or-nothing trade-off and justify it (e.g., "plans share adapters; partial switch creates inconsistent state").

---

## FINDING-A2-11 🟡 Important — §5.5 Phase E enforcement mechanism unspecified

**Location:** §5.5 Phase E — stop legacy writes
**Issue:** §5.5 says "Remove legacy write paths" and the acceptance criterion is "code search confirms zero INSERT/UPDATE/DELETE on legacy tables." But is this compile-time enforced or runtime? If compile-time, removing the code is sufficient. If there's any dynamic SQL (which there is — `storage.rs` builds SQL strings), a code search might miss a constructed query. No `#[deprecated]` attribute, no compile-time gate, no runtime assertion that legacy tables are read-only.

**Suggested fix:** Add: "Phase E acceptance includes: (a) remove all legacy write functions from `Storage` impl, (b) add a debug-assert in `Storage::open` that verifies legacy tables have no triggers writing to them, (c) CI grep for legacy table names in INSERT/UPDATE/DELETE context."

---

## FINDING-A2-12 🟡 Important — §5.6 Phase F gate condition inconsistent with §5 header

**Location:** §5.6 Phase F — drop legacy
**Issue:** §5 header says "one week of production traffic" before dropping. §5.6 says "≥2 weeks of unified-only writes." These are different: §5.5→§5.6 gate is 2 weeks, but the header implies 1 week. Which is it? Additionally, §5.6 doesn't specify what "production traffic" means quantitatively — just time elapsed. What if the system is idle for 2 weeks?

Also: §5.6 mentions dropping `triples` per §7.6, and T39 in §8 mentions dropping `nodes.episode_id` and `edges.episode_id` per §7.4. But §3.1 and §3.2 schemas DON'T HAVE `episode_id` columns (they were already removed from the schema definition per §7.4). T39's mention of "DROP denormalized columns (`nodes.episode_id`, `edges.episode_id`)" is contradictory — the columns don't exist in the terminal schema, so there's nothing to drop. This is likely referring to dropping them from legacy tables during migration, but that happens automatically when legacy tables are dropped.

**Suggested fix:** Reconcile the gate: "≥2 weeks" is the intended duration (stricter). Remove the T39 reference to dropping `episode_id` columns from `nodes`/`edges` since those columns were never added to the unified schema.

---

## FINDING-A2-13 🟡 Important — §4.17 coverage closure claims are unverifiable from §4.1-4.10 alone

**Location:** §4.17 Coverage closure
**Issue:** §4.17 claims "every active cognitive function in the codebase maps cleanly" but only references §4.11-§4.14 and §4.15-§4.16. It does NOT enumerate which GOALs are covered by §4.1-4.10 specifically. Spot-checking: §4.9 (Promotion) says promotion "stays as audit per §7.5" — this means promotion_candidates is NOT in the unified substrate, yet §4.17 doesn't flag this as a gap or confirm it's intentional. §4.10 (Episodes) maps cleanly. The "batch consolidation reactivation" and "goal/plan completion" extensions mentioned are future — not current cognitive functions. The closure argument would be stronger if §4.17 had a table: `{cognitive function} → {§4.x} → {GOAL covered}`.

**Suggested fix:** Add a coverage matrix or at minimum list each GOAL-ref and which §4.x subsection addresses it.

---

## FINDING-A2-14 🟢 Minor — §4.9 Promotion is ambiguous — audit table vs node_kind

**Location:** §4.9 Promotion
**Issue:** §4.9 says promotion_candidates "becomes nodes of kind `'promotion_candidate'` ... Or kept as audit table — decision in §7 Q5." But §7.5 RESOLVED this as "stays as audit table." §4.9 still presents it as an open question despite §7.5 being closed. The section should be updated to reflect the resolved decision.

**Suggested fix:** Replace the "Or kept as audit table" phrasing with: "Per §7.5, `promotion_candidates` stays as a dedicated audit table (not a `node_kind`)."

---

<!-- FINDINGS -->

## ✅ Passed Checks

- **§4.1 Memory ingest**: Unified INSERT covers all required fields from `MemoryRecord`. `occurred_at`/`created_at` both present. Episode containment edge correctly specified. ✅
- **§4.2 Entity resolution**: Matches §7.2 (surface forms as nodes, `same_as` edges). `memory_entities.role` → `edges.predicate` mapping is complete. ✅
- **§4.3 Hebbian weight updates**: ON CONFLICT upsert on partial UNIQUE index is consistent with §3.2 `idx_edges_assoc_unique`. `json_patch` for attribute accumulation is correct. ✅ (pseudocode quality issues flagged separately)
- **§4.5 Synthesis**: `node_kind='insight'` distinct from `node_kind='episode'`. Provenance edges (`derived_from`) carry gate metadata in `attributes`. ✅
- **§4.6 Decay**: Uses `created_at` (correct per ISS-103). Differential Hebbian decay reads `signal_source` from `edges.attributes` JSON. `pinned=1` skip logic preserved. ✅
- **§4.7 Supersession**: Consolidates 4 legacy mechanisms into 2 (node `superseded_by` + edge `supersedes`/`invalidated_at`). ✅
- **§4.10 Episodes**: Episodes as nodes per §7.4. Containment edges `belongs_to_episode`. Migration path specified (Phase C creates episode nodes from distinct legacy `episode_id` values). ✅
- **§5.1 Phase A**: Additive only — CREATE TABLE, no ALTER. Acceptance criterion is "existing test suite green." ✅
- **§5.3 Field mappings**: `memories` → `nodes` mapping is thorough (layer, memory_type, superseded_by normalization, two-pass FK ordering). `hebbian_links` signal fields preserved in `edges.attributes` for differential decay. ✅
- **§3.2 edge_kind consistency**: All §4.x sections use edge_kind values from the §3.2 taxonomy (structural, associative, containment, provenance, supersession). No rogue edge_kinds introduced. ✅

## Applied

(None — awaiting human approval before apply phase.)
