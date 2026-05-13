# Design Review r3-part3 — Substrate Cognitive Ops (§4.11–§4.17 + §5)

> **Reviewer:** claude (sub-agent, design-review skill)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` lines 650–1158
> **Prior review:** `design-r2-part3-substrate-ops.md` (13 findings — most resolved in commits 1–5)
> **Method:** 27-check review-design skill (standard depth), post-commit-5 debt cleanup
> **Focus:** Internal consistency of revised §4.11–§4.17 + §5 migration plan

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 1 |
| 🟡 Important  | 6 |
| 🟢 Minor      | 4 |
| **Total**  | **11** |

**Recommendation**: Needs targeted fixes before implementation. The 1 critical finding (FINDING-2: WM crash-recovery data quality) is a small fix (one attribute). The 6 important findings are mostly accounting/specification gaps — the architecture is sound, but implementers need clearer contracts for: domain-stats write frequency (FINDING-1), Batch composition (FINDING-3), write-amplification baseline (FINDING-4), degradation behavior (FINDING-5), module retirement scope (FINDING-6), dimension backfill in migration plan (FINDING-9), and Phase F quality gates (FINDING-11).

**Estimated implementation confidence**: Medium-High — design intent is clear, architecture is well-justified. The commit-5 revisions resolved 10 of 16 r2 findings. Remaining gaps are specification-level, not architecture-level.

**Comparison to r2**: r2 had 4 critical / 6 important / 3 minor = 13. r3 has 1 critical / 6 important / 4 minor = 11. Critical count dropped from 4→1; the remaining critical is a new edge case (crash recovery), not a schema-mapping gap. Major structural issues (node_type/edge_type confusion, edge taxonomy violations, §8.13 contradiction) are all resolved.

---

## FINDING-1 🟡 Important — §4.11 domain node `UpdateDomainStats` contradicts "baseline ephemeral" volume math

**Check #6 (Data flow completeness) + Check #33 (Simplification vs completeness)**

r2 FINDING-A3-4 flagged the contradiction between "baseline signals are ephemeral" and "writer folds each signal into domain node rolling stats." Commit 1b added `UpdateDomainStats` to §6.1, closing the missing-WriteOp gap. But the **volume contradiction persists**:

§4.11 says: "baseline signal rate is ~1-10/sec across all subsystems (high), so **dropping them is the only sane choice**."

§6.1 `UpdateDomainStats` says: "baseline stream: folded into rolling stats, NOT persisted as event" — but folding into rolling stats IS a write. Each `UpdateDomainStats` op goes through the writer queue, opens a transaction, and UPDATEs the domain node's `attributes` JSON. At 1-10/sec, that's 3,600-36,000 UPDATEs/hour to domain nodes.

§4.15.6 write amplification budget accounts for memory ingest ops but does NOT include `UpdateDomainStats` in the throughput model. The §6.6 throughput ceiling of ~11,000 ops/sec has headroom, but the budget analysis is incomplete.

Two options:
1. **UpdateDomainStats is truly queue-routed**: then §4.15.6 must include it in the throughput budget. At 10/sec steady-state + 1,200/sec peak ingest = 1,210 ops/sec peak. Still within 9× headroom, but the accounting should be explicit.
2. **Domain stats fold in-memory, bypass queue**: the hub maintains rolling stats in `InteroceptionService` memory and only writes to substrate on anomaly events or periodic checkpoints. This matches "dropping them" but means `UpdateDomainStats` should NOT be a queue op — it should be an in-memory method.

**Suggested fix**: Clarify in §4.11 that `UpdateDomainStats` is either:
- (a) A queue op at 1-10/sec (update §4.15.6 budget to include it), OR
- (b) An in-memory fold with periodic `SnapshotDomainStats` queue op (add the snapshot op, remove or rename `UpdateDomainStats`)

---

## FINDING-2 🔴 Critical — §4.13 WM crash recovery is explicitly "lost" but §4.14 WriteWmSnapshot atomicity depends on in-memory WM state

**Check #7 (Error handling) + Check #5 (State machine — no deadlocks)**

§4.13 states: "On process restart, WM clears. That is biologically accurate — humans wake up without prior working-memory state either."

§4.14 states: `WriteWmSnapshot { feedback_event_id, slot_contents }` — "fires in the same transaction as `WriteFeedbackEvent` so the snapshot and the evaluation are atomically linked."

The atomicity guarantee is correct (both go into a `Batch` per §6.4). But the **data source** for `slot_contents` is the in-memory WM ring buffer. If metacognition triggers a feedback event right after a crash-restart (before any memories have been recalled into WM), the snapshot will be empty — not because the agent had nothing in WM, but because the crash lost it.

This creates a silent data-quality issue: feedback events post-restart have empty WM snapshots that look identical to "agent had genuinely empty WM." There's no way to distinguish "WM was empty" from "WM was lost."

**Suggested fix**: Add a `wm_valid: bool` flag (or `wm_state: 'cold_start' | 'warm'`) to `wm_snapshot` attributes. On process restart, WM enters `cold_start` state until at least one recall populates it. `WriteWmSnapshot` captures `wm_state` so downstream analysis can filter cold-start snapshots. This costs zero additional writes — it's just an attribute on the snapshot node.

---

## FINDING-3 🟡 Important — §4.14 `WriteFeedbackEvent` + `WriteWmSnapshot` Batch atomicity assumes caller constructs the Batch, but caller composition is unspecified

**Check #21 (Ambiguous prose) + Check #6 (Data flow completeness)**

§4.14 says these two ops "fire in the same transaction." §6.4 specifies `Batch { ops: Vec<WriteOp> }` for compound atomicity. But:

1. **Who constructs the Batch?** §4.14 doesn't say whether metacognition.rs builds a `Batch` containing both ops, or whether the writer task recognizes `WriteFeedbackEvent` and auto-appends a `WriteWmSnapshot`. The §6.1 `Batch` semantics say inner ops' `reply` channels are dropped — so the caller must use the outer Batch reply. This implies the **caller** constructs the Batch.

2. **What if metacog wants a feedback event WITHOUT a WM snapshot?** (e.g., evaluating a historical retrieval trace where current WM is irrelevant). Is the WM snapshot always mandatory? §4.13 says snapshot triggers include "every metacognition feedback event" — suggesting yes. But that means feedback events for historical evaluations carry a misleading WM snapshot of the current (unrelated) context.

**Suggested fix**: Add to §4.14: "Metacognition evaluator constructs `Batch { ops: [WriteFeedbackEvent {...}, WriteWmSnapshot {...}] }` and sends it to the writer queue. The WM snapshot captures *current* WM at evaluation time. For evaluations of historical events, the snapshot's `trigger_reason` is set to `'historical_eval'` to distinguish from real-time evaluations."

---

## FINDING-4 🟡 Important — §4.15.6 write amplification budget: "today's ~5 (P50)" baseline is unverified

**Check #29 (Ground truth verification) + Check #20 (Appropriate abstraction level)**

§4.15.6 presents a write amplification table showing the unified substrate produces ~11 inserts (P50) vs today's ~5 (P50), yielding a 2.2× ratio. The unified-side breakdown is detailed and traceable:

| Component | P50 | P95 |
|-----------|-----|-----|
| memory node | 1 | 1 |
| `describes_*` edges | 4 | 7 |
| `tagged` edges | 2 | 5 |
| etc. | ... | ... |

But **today's baseline of ~5** is stated without derivation. What are the 5 writes today?
- 1 × `INSERT INTO memories` 
- 1 × `INSERT INTO memory_embeddings`
- ~3 × entity-related? (but entity resolution is async, post-write)

If entity resolution writes are excluded from "today's" count (since they're async), they should also be excluded from the unified count (where `mentions` edges are also async via ResolutionPipeline). The comparison may be apples-to-oranges.

**Suggested fix**: Add a breakdown table for "today's" baseline matching the same component categories. E.g.:
```
Today P50: 1 memory + 1 embedding + 1 FTS trigger + ~2 async entity edges = ~5
```
This makes the 2.2× ratio auditable.

---

## FINDING-5 🟡 Important — §4.15.6 "what fails at 9×" is unanswered — no degradation model

**Check #7 (Error handling) + Check #33 (Simplification vs completeness)**

§4.15.6 states: "§6.6 throughput ceiling: ~11000 ops/sec. Headroom 9×. Well within budget." And lists mitigations "if growth exceeds projection." But:

1. **What happens at exactly 9×?** The writer queue fills (§6.3 bounded queue), backpressure surfaces as ingest latency. But what does the caller experience? Does `store_raw` block? Return an error? Drop the memory? §6.3 should specify this, but §4.15.6 doesn't cross-reference the backpressure behavior.

2. **The 9× headroom is for steady-state P95.** Burst patterns (e.g., bulk import of 1000 memories) could momentarily exceed the ceiling. The mitigations (lazy materialization, tag cap, background queue) are all described as "none required at launch; tunable knobs" — but there's no monitoring/alerting to know when to turn them.

**Suggested fix**: Add to §4.15.6: "When writer queue fills (capacity N per §6.3), `WriteMemory` callers receive `Err(WriterBackpressure)` and must retry with exponential backoff. Monitoring: track `writer_queue_depth` metric; alert at 80% capacity. Bulk import path should use a dedicated rate-limited channel."

---

## FINDING-6 🟡 Important — §4.16 "19 modules retire" — module count discrepancy (19 vs 21)

**Check #4 (Consistent naming) + Check #29 (Ground truth)**

The task description says "19 modules retire." §4.16 evidence shows `find ... | wc -l` → **21** modules. §4.16.1 says 2 modules have concepts worth re-using (`intake.rs`, `manual_edit.rs`). So the math is 21 - 2 = 19 retired.

But §4.16.1 says both `intake.rs` and `manual_edit.rs` **also have zero callers** and are "candidates for re-integration as substrate writers, not as active dependencies." So they're also being deleted in the retirement task — all 21 modules are removed, with 2 having their *concepts* (not code) carried forward.

§4.16.4 says: "T-XX: Remove `crates/engramai/src/compiler/` and update Cargo.toml + lib.rs exports." This removes the entire directory — all 21 modules.

The "19 retire, 2 keep" framing is misleading. All 21 are deleted; 2 have concepts noted for future reimplementation. This is a minor point but could confuse an implementer about whether `intake.rs` and `manual_edit.rs` should be preserved.

**Suggested fix**: Clarify in §4.16.1: "All 21 modules are deleted. Two modules (`intake.rs`, `manual_edit.rs`) contain *concepts* worth reimplementing as substrate writers in a future task — but their current code is not preserved."

---

## FINDING-7 🟢 Minor — §4.16 tests against retired modules are not addressed

**Check #34 (Breaking-change risk) + Check #24 (Migration path)**

§4.16 evidence shows "5/5 integration tests pass" for v0.2 compiler. When the 21 modules are deleted (T60 in §8.14), those 5 tests will fail to compile. §4.16 doesn't specify whether these tests should be:
- (a) Deleted alongside the modules (they test dead code)
- (b) Adapted to test v0.3 equivalents (if coverage gaps exist)

**Suggested fix**: Add to §4.16.4 or §8.14 T60: "Delete the 5 v0.2 compiler integration tests. Verify that v0.3 `knowledge_compile` tests cover the same scenarios; if not, port the test cases (not the test code) to v0.3."

---

## FINDING-8 🟢 Minor — §5 Phase D acceptance criterion "J-score unified ≥ legacy (current 42.1%)" is a low bar

**Check #25 (Testability) + Check #33 (Simplification)**

§5.4 Phase D acceptance says: "LoCoMo J-score on bench: unified ≥ legacy (current 42.1%)."

This says the unified substrate passes if it matches current quality — but §4.15 Tier 2 edges are designed to *improve* retrieval quality (cross-memory dimension traversal). If unified J-score equals legacy, the Tier 2 edges aren't helping yet, which may indicate a bug in the retrieval adapters.

**Suggested fix**: Keep the "≥ legacy" acceptance criterion (it's the minimum), but add a stretch target: "Expected: unified J-score > legacy by ≥2% (Tier 2 dimension edges enable new retrieval paths). If unified = legacy ± 1%, investigate whether dimension-edge traversal is active in retrieval plans."

---

## FINDING-9 🟡 Important — §5 Phase C backfill doesn't mention dimension backfill (§4.15 Tier 2/3 edges)

**Check #6 (Data flow completeness) + Check #24 (Migration path)**

§5.3 Phase C lists backfill for: memories → nodes, memory_embeddings → node_embeddings, entities → nodes, entity_relations → edges, memory_entities → edges, hebbian_links → edges, synthesis_provenance → edges.

It does NOT mention backfilling Tier 2 `describes_*` edges or Tier 3 `tagged` edges from existing `memories.dimensions` JSON blobs. §4.15.5 says "backfill (§5.3) iterates `memories.dimensions` blobs, materializes nodes-and-edges on first encounter" — but this backfill step doesn't appear in §5.3's numbered list or in §8.4's tasks (T19-T27).

Without dimension backfill, Phase D retrieval adapters that traverse `describes_*` edges would find nothing for historical memories — only new memories ingested during Phase B would have dimension edges. This would cause the J-score parity test to fail.

**Suggested fix**: Add to §5.3: "10. Backfill dimension edges: iterate `memories.dimensions` JSON blobs, materialize Tier 2 `describes_*` edges + dimension nodes and Tier 3 `tagged` edges + tag nodes (§4.15). Dedup dimension/tag nodes by value-hash." Add corresponding task to §8.4 (e.g., T27b).

---

## FINDING-10 🟢 Minor — §4.14 aggregate query uses bare `attributes.score` without JSON extraction syntax

**Check #22 (Missing helpers)**

(Carried from r2 FINDING-A3-16 — still present in revised text.)

§4.14 shows: `SELECT AVG(attributes.score) FROM nodes WHERE node_kind='feedback' AND dimension='recall_accuracy'`

`attributes` is `TEXT` (JSON) per §3.1. SQLite requires `json_extract(attributes, '$.score')`. Similarly `dimension` is inside `attributes`, not a top-level column. Corrected: `SELECT AVG(json_extract(attributes, '$.score')) FROM nodes WHERE node_kind='metacog_feedback' AND json_extract(attributes, '$.dimension') = 'recall_accuracy' AND created_at > ...`

Also note §4.14 body uses `node_kind='metacog_feedback'` but the aggregate query still says `node_kind='feedback'` — inconsistent.

**Suggested fix**: Fix the query to use `json_extract()` and `node_kind='metacog_feedback'`.

---

## FINDING-11 🟡 Important — §5 Phase F "≥2 weeks" has no measurable "done" criterion beyond time

**Check #25 (Testability) + Check #31 (Shortcut detection)**

§5.6 says: "After ≥2 weeks of unified-only writes, drop legacy tables." The acceptance criterion is: "schema diff matches §3 exactly. `ls -lh engram-memory.db` shows size reduction."

The 2-week timer is a time-gate, not a quality-gate. What if a bug surfaces in week 3? The real criterion should be: "no legacy reads in production logs for ≥2 weeks AND no quality regression flagged." The time-gate alone doesn't prove the system is healthy — it proves nobody noticed a problem.

§5.4 Phase D has quality gates (J-score, Recall@10). Phase F has none — it trusts that Phase D+E caught everything.

**Suggested fix**: Add to §5.6 acceptance: "Zero `unified_substrate=false` overrides in production. Zero retrieval quality regressions (J-score ≥ Phase D baseline). Zero `fallback_to_legacy` log entries. Only then does the 2-week clock start."

---

<!-- FINDINGS -->

## ✅ Passed Checks

- **Check #0**: Document size — §4.11–§4.17 is 7 subsections + §5 (6 phases). Total §4.x is 17 subsections, but §4.17 is a closure statement. Not recommending split. ✅
- **Check #1**: Types fully defined — all node_kinds now use single-level `node_kind='interoceptive_domain'` etc. matching §3.1. Edge predicates mapped to `edge_kind` taxonomy in §3.2 table. ✅ (r2 FINDING-A3-1, A3-2 resolved)
- **Check #2**: References resolve — §4.13 → §4.14 (WriteWmSnapshot), §4.14 → §6.1 (WriteFeedbackEvent), §4.15 → §3.2 (edge taxonomy). All cross-refs valid. ✅
- **Check #3**: No dead definitions — all node_kinds introduced in §4.11–§4.16 have writer paths in §6.1 and reader paths documented. ✅ (r2 FINDING-A3-3 resolved: WriteSomaticMarker, WriteRegulationPolicy added)
- **Check #4**: Consistent naming — `node_type`/`edge_type` confusion eliminated. `source_id`/`target_id` consistent with §3.2. `node_kind='metacog_feedback'` used consistently (except one query instance — see FINDING-10). ✅ mostly
- **Check #5**: State machine — §4.13 WM snapshot triggers well-defined (demand-driven). §4.11 anomaly threshold is z-score-based. No deadlocks in signal→marker→retrieval path. ✅ (see FINDING-2 for crash-recovery edge case)
- **Check #7**: Error handling — §6.9 covers writer failures. §4.11 anomaly false-alarm lifecycle still unspecified (r2 FINDING-A3-14 was 🟡, not addressed in commits — but this is a design-completeness gap, not a blocking issue for implementation). ✅ partial
- **Check #8**: String operations — no string slicing on user text in §4.11–§4.17 or §5. ✅
- **Check #9**: Integer overflow — `sample_count` (somatic markers), `coactivation_count` (Hebbian) are u32, bounded by practical event rates. ✅
- **Check #10**: Option/None — no `.unwrap()` patterns in pseudocode. ✅
- **Check #13**: Separation of concerns — §4.12 cleanly separates substrate patterns from I/O (file reads/writes). §4.11 two-tier separates ephemeral signals from persistent anomaly events. ✅
- **Check #14**: Coupling — events carry observed data, not derived state. WriteOps carry payloads, not pre-computed substrate state. ✅
- **Check #15**: Configuration — z-score threshold, MIN_SAMPLES, BATCH_MAX, tag cap (§4.15.6) documented as tunables. ✅
- **Check #16**: API surface — §4.15.4 preserves `dimension_access.rs` public API as thin shim. ✅
- **Check #17**: Goals/non-goals — §0 TL;DR + §1.3 state goals. Non-goals implicit (not multi-process, not sharded — see §6.7). ✅
- **Check #18**: Trade-offs documented — §4.13 Option A/B/C analysis excellent. §4.15.5 justifies edge cost. §4.15.6 write amplification budget is new and strong. ✅
- **Check #23**: Dependencies — no external deps in §4.11–§4.17 or §5. ✅
- **Check #30**: Technical debt — §4.16.3 explicitly documents v0.3 KC bug as tracked debt. No "clean up later" shortcuts. ✅
- **Check #31**: Shortcut detection — §4.11 two-tier addresses root cause (high signal volume). §4.13 Option C is root-cause design. ✅ (see FINDING-11 for Phase F time-gate)
- **Check #32**: Architecture conflicts — §8.13 T56-T59 now match §4.15 design (r2 FINDING-A3-11 resolved). §3.2 taxonomy table includes all new predicates from §4.11-§4.16. ✅
- **Check #34**: Breaking-change risk — §4.16 confirms zero external callers. §4.15.4 preserves accessor API. §5 phased migration with rollback. ✅
- **Check #35**: Purpose alignment — all components trace to §0 TL;DR goals. No speculative flexibility. ✅

### r2 Findings Resolution Status

| r2 Finding | Status | Notes |
|------------|--------|-------|
| A3-1 (node_type/edge_type) | ✅ Resolved | Flattened to single `node_kind`, predicates mapped |
| A3-2 (edge predicates unmapped) | ✅ Resolved | §3.2 taxonomy table complete |
| A3-3 (missing WriteSomaticMarker/WriteRegulationPolicy) | ✅ Resolved | Added to §6.1 |
| A3-4 (domain node write path) | ⚠️ Partially resolved | `UpdateDomainStats` added but volume contradiction persists (FINDING-1) |
| A3-5 (empathy WriteOps collapsed) | ✅ Resolved | 4 distinct ops in §6.1 |
| A3-6 (§4.13/§4.15 dimension_access contradiction) | ✅ Resolved | §4.13 now defers to §4.15 |
| A3-7 (metacog observation mechanism) | ⚠️ Not addressed | Minor — carried forward implicitly |
| A3-8 (describes_* as edge_kind) | ✅ Resolved | Now `edge_kind='structural', predicate='describes_*'` |
| A3-9 (wrong column names) | ✅ Resolved | `source_id`/`target_id` consistent |
| A3-10 (storage cost math) | ✅ Resolved | §4.15.6 budget replaces incorrect estimate |
| A3-11 (§8.13 node_dimensions table) | ✅ Resolved | T56-T59 rewritten for graph-native tiers |
| A3-12 (node_kind enumeration) | ⚠️ Partially | §3.2 edge taxonomy complete; §3.1 node_kind list still uses `...` |
| A3-13 (write amplification) | ✅ Resolved | §4.15.6 budget explicitly models compound ops |
| A3-14 (anomaly_event lifecycle) | ⚠️ Not addressed | Important — false-alarm resolution still unspecified |
| A3-15 (empathy_event in §8.10) | ⚠️ Not verified | Minor |
| A3-16 (aggregate query syntax) | ❌ Not fixed | Still present (FINDING-10) |

## Applied

(None — awaiting human approval before apply phase.)
