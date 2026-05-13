# Design Review r3 Part 1 — Foundation (§0-§3)

> **Reviewer:** sub-agent (coder)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` lines 1-421 (§0-§3)
> **Prior reviews:** r1-pre-expansion (16 findings, all applied), r2-part1-foundation (9 findings)
> **Method:** 27-check standard-depth review-design skill, post commit-5 debt cleanup

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 1     |
| 🟡 Important  | 3     |
| 🟢 Minor      | 3     |
| **Total**  | **7** |

**Recommendation:** Needs targeted fixes before implementation. FINDING-7 (structural edge UNIQUE contradiction) blocks Phase B dimensional signature work. The §2 staleness (FINDING-1, FINDING-2) should be updated for accuracy but doesn't block schema creation. All r1/r2 critical findings verified resolved — the schema is in much better shape post commit-5.

**Estimated implementation confidence:** High — schema is internally consistent except for the structural-edge uniqueness gap. §0-§3 are implementation-ready after FINDING-7 is resolved.

---

## FINDING-1 🟡 Important — §2 row counts are stale; synthesis_provenance is 6.7× higher than claimed

**Check:** #29 (Ground truth verification)
**Location:** §2, row count table

**Issue:** §2 claims "Verified current state (2026-05-12)" but the actual row counts (verified just now via `sqlite3 rustclaw/engram-memory.db`) diverge significantly:

| Table | §2 claims | Actual (now) | Delta |
|---|---|---|---|
| memories | 24,624 | 24,782 | +158 |
| memory_embeddings | 24,467 | 24,545 | +78 |
| entities | 2,310 | 2,330 | +20 |
| entity_relations | 6,531 | 6,587 | +56 |
| memory_entities | 9,237 | 9,233 | −4 |
| hebbian_links | 43,710 | 43,561 | −149 |
| synthesis_provenance | 72 | 483 | **+411 (6.7×)** |
| knowledge_topics | 0 | 0 | — |
| cluster_assignments | 0 | 0 | — |
| promotion_candidates | 0 | 0 | — |

The small deltas (memories, entities) are explainable by continued operation since the snapshot. But:
- **synthesis_provenance: 72 → 483** is a 6.7× increase. This suggests the synthesis pipeline became active since the snapshot. The design's characterization of synthesis_provenance as a minor table (72 rows) materially understates its growth rate. Migration planning (Phase C backfill T25) should account for ~500+ rows, not 72.
- **hebbian_links decreased by 149** — implies decay/pruning is active, which is consistent but not noted.

The r2 review (FINDING-A1-8) already flagged that §2 lacks reproducible verification commands. That finding was not applied. This r3 finding reinforces it with concrete evidence of drift.

**Suggested fix:**
1. Update §2 row counts to current values.
2. Add reproducible queries as §4.16 does (e.g. `$ sqlite3 rustclaw/engram-memory.db "SELECT COUNT(*) FROM memories"`).
3. Note the synthesis_provenance growth rate — it affects T25 backfill sizing.

---

## FINDING-2 🟡 Important — §2 lists 10 tables but production DB has 35 non-FTS tables; 20+ unaccounted

**Check:** #29 (Ground truth verification) + #33 (Simplification vs completeness)
**Location:** §2

**Issue:** §2 presents a 10-row table as the complete "current state." The actual production DB has **35 non-FTS tables** (verified via `sqlite_master`). The 20+ unlisted tables include:

**v0.3 graph tables** (0 rows each but schema exists):
`graph_entities`, `graph_edges`, `graph_links`, `graph_entity_aliases`, `graph_memory_entity_mentions`, `graph_predicates`, `graph_applied_deltas`, `graph_pipeline_runs`, `graph_resolution_traces`, `graph_extraction_failures`

**KC/cluster tables**:
`cluster_centroids`, `cluster_incremental_state`, `cluster_pending`, `cluster_state`, `kc_compilation_records`, `kc_compilation_sources`, `kc_topic_pages`

**Other**:
`behavior_log`, `emotional_trends`, `engram_acl`, `triples`

§3.5 names 7 audit tables to be "retained, not unified" (`pipeline_runs`, `resolution_traces`, `extraction_failures`, `access_log`, `engram_meta`, `backfill_queue`, `quarantine`). But the actual DB names are `graph_pipeline_runs`, `graph_resolution_traces`, `graph_extraction_failures` — the `graph_` prefix is missing from §3.5's list, so an implementer wouldn't know which tables to retain.

The v0.3 graph tables are particularly important: §2 mentions them in prose ("v0.3 DB... `graph_entities`, `graph_edges`") but doesn't include them in the summary table or specify their migration disposition. §5 Phase F should explicitly list which of these 10 `graph_*` tables get dropped.

**Impact:** Phase F "drop legacy" is under-specified. An implementer following §2 + §5.6 would miss 20+ tables — leaving dead schema behind or accidentally dropping audit tables.

**Suggested fix:**
1. Expand §2 table to include ALL non-FTS tables, grouped by disposition: (a) migrate to unified, (b) retain as audit, (c) drop in Phase F, (d) already empty/dead.
2. Fix §3.5 audit table names to match actual DB names (add `graph_` prefix where needed).
3. Ensure §5.6 Phase F lists every table to be dropped — no implicit "everything else."

---

## FINDING-3 🟢 Minor — §0 TL;DR "10 tables" count is inaccurate

**Check:** #2 (References resolve) + #4 (Consistent naming)
**Location:** §0, line ~14

**Issue:** §0 says "implementation grew organically into **10 tables** (4 node-shaped, 5 edge-shaped, 1 FTS)." The 1 FTS is `memories_fts`. But §2's table lists `memory_embeddings` as "ext" not "FTS" — so §0's "(4 node-shaped, 5 edge-shaped, 1 FTS)" doesn't match §2's "(4 node, 5 edge, 1 ext)." And the actual DB has 35 tables. This was noted in r2 FINDING-A1-7 but not applied.

The accounting also omits `triples` (0 rows, acknowledged in §7.6) and the 10 `graph_*` tables (v0.3 schema, 0 rows).

**Suggested fix:** Either expand the count to be accurate ("**10 active data tables** + 10 empty v0.3 tables + 7 operational/audit tables + FTS") or simplify to "dozens of tables" without a specific breakdown that doesn't add up.

---

## FINDING-4 🟡 Important — §3.1 `node_kind` comment enumerates 6 values but §4 uses at least 11

**Check:** #1 (Every type fully defined) + #4 (Consistent naming)
**Location:** §3.1 DDL comment (line ~129) vs §4.11-§4.15

**Issue:** The `node_kind` column comment says `'memory'|'entity'|'topic'|'insight'|'episode'|'plan'|...`. The trailing `|...` is a hand-wave. From §4, the full enumeration used in this design is:

1. `memory` — §4.1
2. `entity` — §4.2
3. `topic` — §4.4
4. `insight` — §4.5
5. `episode` — §4.10, §7.4
6. `plan` — §4.8 (mentioned in §3.1 comment)
7. `interoceptive_domain` — §4.11
8. `somatic_marker` — §4.11
9. `drive` — §4.11
10. `wm_snapshot` — §4.13
11. `tag` — §4.15 Tier 3
12. `dimension` — §4.15 Tier 2 (location, participant, etc.)
13. `feedback_event` — §4.14

The `edge_kind` taxonomy in §3.2 has a complete enumeration table with every predicate and its source section. There is no equivalent for `node_kind`. Two implementers would derive different `node_kind` value sets from reading §4.

**Suggested fix:** Add a `node_kind` taxonomy table to §3.1 (parallel to §3.2's edge taxonomy), enumerating all 13+ values with their source § and what they replace. This is especially important because §3.1 says "Adding new node-kinds is a schema-free operation (just a new value in `node_kind`)" — but implementers need to know the initial set.

---

## FINDING-5 🟢 Minor — §3.2 `supersession` edge_kind has no predicate row in taxonomy table

**Check:** #1 (Every type fully defined) + #3 (No dead definitions)
**Location:** §3.2, edge_kind taxonomy table

**Issue:** The taxonomy table lists `supersession` as an edge_kind but the row says "(managed via `supersedes` col)" with no predicate value. Rule 3 below the table says "Supersession is structural, not predicate-shaped: the `supersedes` and `invalidated_by` *columns* on `edges` express edge-level supersession; `edge_kind='supersession'` is reserved for cases where supersession itself is the relation being modeled (rare)."

If `edge_kind='supersession'` is "rare" and no predicate is defined, it's currently a dead definition — defined in the closed set of 6 edge_kinds but never used in any §4 cognitive function. The partial UNIQUE indexes don't cover it. An implementer might wonder: should I create edges with `edge_kind='supersession'`? What predicate do I use?

**Suggested fix:** Either (a) define at least one predicate for `supersession` edge_kind (e.g. `supersedes`, `corrects`, `retracts`) with a source §, or (b) remove it from the closed set and document that supersession is always expressed via the `supersedes`/`invalidated_by` columns on edges of other kinds. Option (b) reduces the closed set to 5 values, which is cleaner.

---

## FINDING-6 🟢 Minor — §3.5 audit table names don't match actual DB names

**Check:** #29 (Ground truth verification)
**Location:** §3.5

**Issue:** §3.5 lists retained audit tables as: `pipeline_runs`, `resolution_traces`, `extraction_failures`. The actual DB tables are: `graph_pipeline_runs`, `graph_resolution_traces`, `graph_extraction_failures`. An implementer would look for `pipeline_runs` and not find it.

Also, `behavior_log` and `emotional_trends` exist in the DB but aren't mentioned in §3.5 — their disposition (retain? drop?) is unspecified.

**Suggested fix:** Use actual DB names in §3.5. Add disposition for `behavior_log`, `emotional_trends`, `engram_acl`.

---

## FINDING-7 🔴 Critical — §4.15.3 claims UNIQUE constraint on structural edges that §3.2 explicitly excludes

**Check:** #5 (Guard conditions exhaustive) + #2 (Every reference resolves)
**Location:** §4.15.3 (line ~842) vs §3.2 partial UNIQUE indexes (lines 284-288) vs §7.3 (line ~1751)

**Issue:** §4.15.3 says:
> "the UNIQUE constraint on `(source_id, target_id, edge_kind, predicate)` from §3.2 prevents accidental duplicates"

for `tagged` edges with `edge_kind='structural'`. But §3.2 only defines partial UNIQUE indexes for `associative` and `containment` edge_kinds:

```sql
CREATE UNIQUE INDEX idx_edges_assoc_unique
    ON edges(...) WHERE edge_kind = 'associative';
CREATE UNIQUE INDEX idx_edges_containment_unique
    ON edges(...) WHERE edge_kind = 'containment';
```

§7.3 explicitly states: "Structural edges remain non-unique." There is **no UNIQUE constraint covering `edge_kind='structural'`**.

This means:
- `tagged` edges (structural) have no dedup protection — re-ingesting a memory with the same tags creates duplicate edges.
- `describes_*` edges (structural, Tier 2) have the same problem.
- `same_as` edges (structural) also — §7.3 line 1736 notes "same_as is structural (non-unique edge_kind)" as if this is intentional.
- §4.15.6 (line ~894) also claims "Edge UNIQUE constraint short-circuits duplicates" for Tier 2/3 re-ingest — same false claim.

This is a design self-contradiction. Either:
- (A) Structural edges should NOT be unique (§7.3 is correct) → then §4.15.3 and §4.15.6 are wrong about dedup guarantees, and the design needs an explicit idempotency strategy for tag/dimension edges (application-level INSERT OR IGNORE? separate partial UNIQUE for specific structural predicates?).
- (B) Some structural predicates SHOULD have uniqueness (tagged, describes_*) → then §3.2 needs additional partial UNIQUE indexes per predicate, and §7.3's blanket "structural = non-unique" needs a caveat.

**Impact:** Without resolution, Phase B dual-write for dimensional signatures (T56-T59) would create duplicate edges on re-ingest, corrupting tag counts and dimension queries.

**Suggested fix:** Option (B) is likely correct. Add partial UNIQUE indexes for structural predicates that are set-membership (idempotent):
```sql
CREATE UNIQUE INDEX idx_edges_structural_tagged
    ON edges(source_id, target_id, edge_kind, predicate)
    WHERE edge_kind = 'structural' AND predicate = 'tagged';
CREATE UNIQUE INDEX idx_edges_structural_describes
    ON edges(source_id, target_id, edge_kind, predicate)
    WHERE edge_kind = 'structural' AND predicate LIKE 'describes_%';
```
Update §7.3 to say "Structural edges are non-unique by default; specific predicates (`tagged`, `describes_*`) use partial UNIQUE indexes for set-membership semantics."

---

<!-- FINDINGS -->

## ✅ Passed Checks

- **Check #0**: Document size — §3 has 5 sub-sections (3.1-3.5), well under 8 limit ✅
- **Check #1**: Types fully defined — SQL schemas for all 4 tables are complete DDL with constraints. `node_kind` enumeration incomplete (FINDING-4) but schema itself is sound ✅ (with caveat)
- **Check #2**: References resolve — §3 cross-refs to §6/§7 verified; §3.3 references §6 writer correctly; §3.2 taxonomy table references §4 sections that exist ✅
- **Check #3**: No dead definitions — `supersession` edge_kind is borderline (FINDING-5, minor) but all other definitions are used ✅
- **Check #4**: Naming consistency — `source_id`/`target_id` now consistent (r2 A1-2 `from_id`/`to_id` fixed). `node_kind`/`edge_kind` naming consistent throughout §0-§3. `nodes.body` ghost (r2 A1-4) gone ✅
- **Check #6**: Data flow — r1 critical findings (FIN-1 edges.attributes, FIN-2 layer, FIN-3 memory_type) all applied. Fields fully mapped ✅
- **Check #7**: Error handling — §3.1 CHECK constraints cover all bounded REAL fields. FK constraints use ON DELETE RESTRICT (safe default) ✅
- **Check #8**: String operations — no string slicing in schema DDL ✅
- **Check #9**: Integer overflow — `fts_rowid` monotonic counter is unbounded INTEGER; SQLite INTEGER is 64-bit, overflow at 2^63 is not a practical concern ✅
- **Check #10**: Option/None — nullable columns documented (embedding, occurred_at, valid_from/to, etc.); NOT NULL where required ✅
- **Check #11**: Match exhaustiveness — `edge_kind` is TEXT (open), taxonomy table provides enumeration for implementers. Not an enum-match concern ✅
- **Check #12**: Ordering sensitivity — N/A for schema DDL ✅
- **Check #13**: Separation of concerns — §3 is pure schema (HOW), §1 is pure motivation (WHY). No leakage. §3.5 correctly separates audit from cognitive substrate ✅
- **Check #14**: Coupling — edges carry observed data; `target_literal` for dangling edges is correctly separated from `target_id` FK path ✅
- **Check #15**: Configuration — `node_kind` and `edge_kind` are open TEXT values, not hardcoded enums. Schema is language-agnostic ✅
- **Check #16**: API surface — minimal: 3 core tables + 1 extension + audit. No unnecessary public surface ✅
- **Check #17**: Goals/non-goals — §1.1-§1.3 clearly state goals; §7 resolves non-goals explicitly ✅
- **Check #18**: Trade-offs — wide-table + NULL strategy justified in §3.1 rationale; TEXT vs BLOB ID trade-off documented (r1 FIN-7 resolved as TEXT) ✅
- **Check #19**: Cross-cutting — decay (§4.6), retirement (superseded_by + deleted_at), provenance (source, source_run_id) all in schema ✅
- **Check #20**: Abstraction level — SQL DDL is concrete enough for implementation; prose explains rationale without over-specifying ✅
- **Check #30**: Technical debt — no "temporary solution" language. `fts_rowid_counter` is a permanent design, not a workaround ✅
- **Check #31**: Shortcut detection — FTS5 contentless + surrogate rowid solves the root cause (VACUUM instability) not the symptom. Sound ✅
- **Check #32**: Architecture conflicts — TEXT PKs match existing codebase convention. CHECK constraints match existing storage.rs pattern ✅
- **Check #33**: Simplification — edge cases (dangling edges via target_literal, bi-temporal windows, multi-model embeddings) are all handled, not simplified away ✅ (except §2 completeness, FINDING-2)
- **Check #34**: Breaking-change risk — Phase A is additive (CREATE TABLE only, no ALTER), verified. §5.1 is safe ✅
- **Check #35**: Purpose alignment — every column in nodes/edges traces to a §4 cognitive function. No speculative columns (predicate_kind now has documented usage in §4.9) ✅

### Previously-raised findings now verified resolved

| r1/r2 Finding | Status | Verification |
|---|---|---|
| r1 FIN-1 (edges.attributes) | ✅ Applied | `attributes TEXT NOT NULL DEFAULT '{}'` present in §3.2 DDL |
| r1 FIN-2 (nodes.layer) | ✅ Applied | `layer TEXT` present in §3.1 DDL |
| r1 FIN-3 (nodes.memory_type) | ✅ Applied | `memory_type TEXT` + `idx_nodes_memory_type` present |
| r1 FIN-5 (Hebbian UNIQUE) | ✅ Applied | Partial UNIQUE indexes in §3.2 |
| r1 FIN-6 (FTS5 rowid) | ✅ Applied | Contentless mode + `fts_rowid` surrogate in §3.1/§3.3 |
| r1 FIN-7 (ID type BLOB→TEXT) | ✅ Applied | All IDs are TEXT throughout |
| r2 A1-1 (node_type ghost) | ✅ Applied | No `node_type`/`edge_type` references found |
| r2 A1-2 (from_id/to_id) | ✅ Applied | All references use `source_id`/`target_id` |
| r2 A1-3 (FTS triggers) | ✅ Applied | Contentless delete syntax used correctly in §3.3 triggers |
| r2 A1-4 (nodes.body) | ✅ Applied | No `nodes.body` reference found |
| r2 A1-7 (10 tables count) | ⚠️ Not applied | Still says "10 tables" (FINDING-3) |
| r2 A1-8 (unverified counts) | ⚠️ Not applied | Counts now stale (FINDING-1) |
| r2 A1-9 (superseded_by index) | ✅ Applied | `idx_nodes_superseded` present in §3.1 |

## Applied

(None — awaiting human approval before apply phase.)
