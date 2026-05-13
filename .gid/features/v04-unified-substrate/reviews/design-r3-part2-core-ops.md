# Design Review r3 Part 2 — Core Cognitive Ops (§4.1–4.10)

> **Reviewer:** sub-agent (coder)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` §4.1–4.10 (lines ~422–1004)
> **Prior review:** `design-r2-part2-ops-migration.md` — 14 findings, commit-5 debt cleanup applied
> **Method:** 27-check review-design skill (standard depth), post-commit-5 re-verification

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 0     |
| 🟡 Important  | 5     |
| 🟢 Minor      | 3     |
| **Total**  | **8** |

**Recommendation:** Ready to implement with fixes. No critical blockers. Five important findings (FINDING-1 through FINDING-5) affect Hebbian behavioral correctness, supersession safety, and episode creation completeness — all should be resolved before implementation of §4.3, §4.7, and §4.10 respectively.

---

## FINDING-1 🟡 Important — §4.3 Hebbian UPSERT changes weight semantics from max-win to additive

**Check:** #29 (Ground truth verification)
**Location:** §4.3 Hebbian co-activation, UPSERT `DO UPDATE SET weight = edges.weight + excluded.weight`

**Issue:** The design's UPSERT uses **additive** weight accumulation (`edges.weight + excluded.weight`). The current production code in `storage.rs:3696 record_association()` uses **max-win** semantics: `existing_strength.max(strength)` — the new strength only wins if it's higher; otherwise just the existing value is kept. The older `bump_coactivation()` at `storage.rs:1453` uses `(strength + 0.1).min(1.0)` (bounded additive with fixed +0.1 increment, capped at 1.0).

Neither current implementation matches the design's unbounded `weight + excluded.weight`. The design's UPSERT has no `.min(1.0)` cap on weight. Given `nodes.activation` has a `CHECK (activation BETWEEN 0.0 AND 1.0)` constraint, but `edges.weight` only has `CHECK (weight >= 0.0)` — so weight *can* grow unboundedly. Is this intentional? If so, it's a behavioral change from both current implementations that should be explicitly documented as a design decision.

Additionally, the current `record_association` checks **both directions** (`(source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1)`) before writing. The design's UPSERT only conflicts on `(source_id, target_id, edge_kind, predicate)` — unidirectional. If A→B and B→A are both inserted, they become **two separate edges**, not one bidirectional link as today. The `direction` attribute hints at awareness of this, but the UPSERT doesn't enforce canonical ordering (current `bump_coactivation` sorts: `if id1 < id2 { (id1, id2) }`).

**Suggested fix:**
1. Add weight cap: `weight = min(edges.weight + excluded.weight, 1.0)` or document why unbounded weight is desired
2. Either (a) canonicalize pair ordering before INSERT (application-side: `let (src, tgt) = if a < b { (a, b) } else { (b, a) }`) or (b) document that the unified substrate intentionally stores directional edges and how retrieval handles the A→B vs B→A lookup
3. Document the behavioral change from max-win to additive as a deliberate design decision

---

## FINDING-2 🟡 Important — §4.3 Hebbian UPSERT drops threshold-gating phase from `bump_coactivation`

**Check:** #29 (Ground truth verification)
**Location:** §4.3 Hebbian co-activation

**Issue:** The current `bump_coactivation()` at `storage.rs:1453` implements a **two-phase model**: links start at `strength=0.0` and accumulate `coactivation_count` until a `threshold` is reached, at which point strength jumps to 1.0 ("link formation"). The design's UPSERT has no threshold phase — it immediately writes `weight = :delta` on first insert and accumulates from there.

This is potentially intentional (simplifying to a continuous-strength model), but it's a behavioral change to a core cognitive primitive. If threshold-gating is being retired, §4.3 should state: "The legacy two-phase threshold model (coactivation_count must exceed N before link forms) is replaced by continuous weight accumulation." Without this, an implementer might think the threshold was accidentally omitted and re-add it.

**Suggested fix:** Add a sentence to §4.3: "Legacy threshold-gating (strength=0 until coactivation_count ≥ N) is retired. The unified model uses continuous weight accumulation starting at the initial delta."

---

## FINDING-3 🟡 Important — §4.3 UPSERT `signal_source` is never updated on conflict

**Check:** #6 (Data flow completeness)
**Location:** §4.3 `DO UPDATE SET` clause

**Issue:** The `json_patch` in the DO UPDATE only updates `coactivation_count`, `temporal_forward`, and `temporal_backward`. It does NOT update `signal_source` or `signal_detail`. On repeated co-activations of the same pair, the `signal_source` is frozen to whatever the **first** insert carried. The current `record_association` updates `signal_source` when `strength > existing_strength`.

This matters because §4.6 differential decay reads `edges.attributes.signal_source` to determine decay rate (corecall=0.95, multi=0.90, default=0.85). If a link starts as `entity` (fast decay) but later gets strong `corecall` reinforcement, it will continue decaying at the fast `entity` rate because `signal_source` was never updated.

**Suggested fix:** Either (a) update `signal_source` in the DO UPDATE to reflect the latest/dominant signal, or (b) add `signal_sources` as an array/histogram to track all contributing sources and let decay use the dominant one, or (c) document that frozen signal_source is intentional and explain the decay implications.

---

## FINDING-4 🟡 Important — §4.7 Supersession has no cycle protection specified

**Check:** #5 (State machine invariants — no deadlocks/infinite loops)
**Location:** §4.7 Supersession / correction

**Issue:** The current code at `storage.rs:1796 resolve_chain_head()` explicitly detects and handles supersession cycles: it walks the chain with a visited set and returns `None` on cycle. The current `supersede()` function prevents self-supersession but does NOT prevent indirect cycles (A superseded_by B, B superseded_by A).

§4.7 says "one mechanism per layer: `nodes.superseded_by`" but specifies no invariant preventing cycles (A→B→C→A). The §6.1 `Supersede` WriteOp carries `old_id` and `new_id` but no cycle check is mentioned. On the unified substrate, a cycle in `superseded_by` would cause `resolve_chain_head` to return `None`, effectively making ALL nodes in the cycle invisible to retrieval (since retrieval filters `WHERE superseded_by IS NULL`).

**Suggested fix:** Add to §4.7: "GUARD-4 (supersession acyclicity): the Supersede WriteOp MUST verify that `new_id` is not itself superseded by `old_id` (direct or transitive). Implementation: walk `new_id.superseded_by` chain (bounded at 100 hops) before writing. Cycle → reject with `SupersessionError::CycleDetected`."

---

## FINDING-5 🟡 Important — §4.10 Episodes have no WriteOp in §6.1

**Check:** #3 (Dead definitions) / #22 (Missing helpers)
**Location:** §4.10 Episodes ↔ §6.1 WriteOp enum

**Issue:** §4.10 specifies episodes as first-class `node_kind='episode'` nodes with `belongs_to_episode` containment edges. However, §6.1's WriteOp enum has NO `WriteEpisode` or `CreateEpisode` variant. Verified by grep: no match for `WriteEpisode` or `CreateEpisode` in design.md.

§4.1's `WriteMemory` includes an episode edge creation note ("if memory belongs to an episode"), but there's no op to **create** the episode node itself. During Phase C backfill, episode nodes are created from the distinct set of legacy `episode_id` values — but post-migration, how does a new episode get created? Currently `memory.rs:811` mints `episode_id = uuid::Uuid::new_v4()` per `store_raw` call. Under the unified substrate, this UUID needs a node INSERT before the containment edge can reference it.

Either `WriteMemory` implicitly creates the episode node (undocumented), or a separate op is needed.

**Suggested fix:** Add to §6.1:
```
WriteEpisode {
    episode_id: Option<NodeId>,  // None → mint new UUID
    content: String,             // episode label/description
    namespace: String,
    reply: oneshot::Sender<Result<NodeId>>,
},
```
Or document in §4.1 that `WriteMemory` emits a compound `Batch` (§6.4) that includes episode node creation when the episode doesn't already exist.

---

## FINDING-6 🟢 Minor — §4.5 Synthesis `attributes` uses non-JSON-standard field references

**Check:** #21 (Ambiguous prose)
**Location:** §4.5 Synthesis / insights, pseudocode

**Issue:** §4.5's pseudocode uses bare identifiers in the `json_object()` call: `'gate_decision', gate_decision, 'gate_scores', gate_scores, 'cluster_id', cluster_id`. These are SQL parameter placeholders but they're not prefixed with `:` like §4.3's UPSERT (which uses `:signal_source`, `:tf`, etc.). Inconsistent parameter notation between §4.3 and §4.5 — minor but could confuse an implementer about whether these are SQL parameters or pseudocode variables.

**Suggested fix:** Use consistent notation: either `:gate_decision` (SQL parameter style) or `gate_decision` (pseudocode variable style) everywhere.

---

## FINDING-7 🟢 Minor — §4.9 Promotion still has vestigial "Or kept as audit table" phrasing

**Check:** #2 (References resolve)
**Location:** §4.9 Promotion

**Issue:** §4.9 says: "Or kept as audit table — decision in §7 Q5." But §7.5 RESOLVED this as "stays as audit table." This was already flagged as FINDING-A2-14 in the r2 review. The phrasing remains unchanged post-commit-5, so this is a **surviving r2 finding that was not applied**.

**Suggested fix:** Replace with: "`promotion_candidates` stays as a dedicated audit table per §7.5 — not a `node_kind`."

---

## FINDING-8 🟢 Minor — §4.6 differential decay SQL path for edges.attributes.signal_source is unverified

**Check:** #29 (Ground truth verification)
**Location:** §4.6 Decay / forget, "Differential decay for associative edges"

**Issue:** §4.6 says differential decay "MUST read the discriminator from `edges.attributes.signal_source` (JSON)." The current implementation at `storage.rs:1600` reads `signal_source` as a **dedicated column** on `hebbian_links`. On the unified substrate, this becomes a `json_extract(attributes, '$.signal_source')` call inside the UPDATE's CASE expression. This SQL is valid (verified: `json_extract` is a core SQLite JSON function) but the performance implication is undocumented: the decay UPDATE scans ALL associative edges and calls `json_extract` per row. With 43,710 current hebbian_links, this is ~44K `json_extract` calls per decay tick.

Not a correctness issue, just a performance observation that should be noted if the edge count grows significantly.

**Suggested fix:** Add a note to §4.6: "Performance: differential decay on `json_extract(attributes, '$.signal_source')` is O(N) over all associative edges. At current scale (~44K) this is sub-second; at 1M+ edges, consider a generated column or partial index on `signal_source`."

---

<!-- FINDINGS -->

## ✅ Passed Checks

- **Check #0 (Doc size)**: §4.1–4.10 = 10 subsections within §4, reasonable for a subsection scope. ✅
- **Check #1 (Types fully defined)**: All types referenced (NodeDraft, NodeId, EdgeId, Direction, BumpAssociation, etc.) are defined in §6.1 WriteOp enum or implied by §3.1/§3.2 schema. No TBD fields. ✅
- **Check #2 (References resolve)**: §4.1→§7.4 (episode as node) exists, §4.2→§7.2 (surface forms as nodes) exists, §4.3→§3.2 (partial UNIQUE) exists, §4.6→ISS-103 exists, §4.7→§3.1 superseded_by column exists, §4.8→all plan names verified in codebase `retrieval/plans/`. ✅
- **Check #4 (Consistent naming)**: `edge_kind`, `predicate`, `node_kind` naming consistent across all 10 subsections. `co_activated` vs `co-activated` — verified consistent as `co_activated` (underscore) everywhere. ✅
- **Check #7 (Error handling)**: §4.3 UPSERT failure → transaction rollback per §6.2 batch semantics. §4.7 supersede failure → `SupersessionError` propagated via oneshot reply. ✅
- **Check #8 (String ops)**: No `&s[..n]` slicing in any §4 pseudocode. ✅
- **Check #9 (Integer overflow)**: `coactivation_count` incremented via `json_extract + 1` — SQLite integer, no overflow risk at realistic scales. ✅
- **Check #10 (Option/None)**: `occurred_at` correctly nullable in §4.1 INSERT (`REAL` with no NOT NULL). `episode_node_id` conditional insert — no unwrap. ✅
- **Check #13 (Separation of concerns)**: All §4 ops are pure data mapping (SQL/pseudocode). Side effects isolated in §6 writer queue. ✅
- **Check #14 (Coupling)**: Events carry observed data only. `BumpAssociation` carries `(from_id, to_id, delta)` — minimal payload. ✅
- **Check #15 (Config vs hardcoding)**: Decay rates (0.95/0.90/0.85) are parameters in `ApplyDecayTick`, not hardcoded in §4.6. ✅
- **Check #16 (API surface)**: WriteOp variants are the public surface. Each maps 1:1 to a §4 cognitive op — no unnecessary variants. ✅
- **Check #17 (Goals/non-goals)**: Stated in §1.1–1.3 and §4 preamble ("If any function doesn't fit, schema is wrong"). ✅
- **Check #18 (Trade-offs)**: §4.13 (WM) explicitly documents 3 options with rejection rationale. §4.9 defers to §7.5 (though phrasing is stale — see FINDING-7). ✅
- **Check #30 (Technical debt)**: No "TODO", "temporary", or "good enough for now" in §4.1–4.10. The one deferred item (§4.2 canonical-vs-clique) is explicitly marked as v0.4.1 scope. ✅
- **Check #31 (Shortcut detection)**: No symptom-level patches. All 10 ops specify root mechanism changes (4 retirement mechanisms → 2, Hebbian table → edge UPSERT, etc.). ✅
- **Check #32 (Conflicts with architecture)**: All ops use `WriteOp` → writer queue → single-transaction pattern from §6. No bypass of the queue. ✅
- **Check #33 (Simplification vs completeness)**: Edge cases preserved: ISS-103 invariant, differential decay, episode backfill, `same_as` clique graph. No requirement dropped without justification. ✅
- **Check #34 (Breaking change risk)**: §5 migration plan covers blast radius per phase. §4 ops are new write paths, not modifications to existing callers (callers change in Phase B/D). ✅
- **Check #35 (Purpose alignment)**: All 10 subsections map to existing cognitive functions verified in §2 production DB. No speculative components. ✅
- **§4.1 Memory ingest**: ISS-103 comment now present in §6.1 `ApplyDecayTick` ("reads nodes.created_at — never occurred_at (ISS-103)"). r2 FINDING-A2-1 resolved. ✅
- **§4.2 Entity resolution**: `same_as` edge per §7.2, `mentions`/`subject_of` predicate mapping complete. ✅
- **§4.3 Hebbian UPSERT syntax**: Canonical single INSERT...ON CONFLICT statement. r2 FINDING-A2-2 (malformed SQL) fully resolved. ✅
- **§4.3 Writer queue cross-ref**: Now includes "coalesced (§6.3) and submitted as a single `BumpAssociation` op". r2 FINDING-A2-3 resolved. ✅
- **§4.4 KC mapping**: `node_kind='topic'` + `containment/contains` edges. Clean. ✅
- **§4.5 Synthesis mapping**: `node_kind='insight'` + `provenance/derived_from` edges with gate metadata. Clean. ✅
- **§4.6 ISS-103 invariant**: Now has explicit bold callout with RUN-0017 context and J-score reference. r2 FINDING-A2-7 resolved. ✅
- **§4.8 Retrieval plans**: Plan-to-query mapping specified for 6 plan types. ✅
- **§4.10 Episode migration**: Phase C creates episode nodes from distinct legacy `episode_id` values, then edges. Backfill idempotency addressed in §5.3 via deterministic edge ID derivation. r2 FINDING-A2-9 resolved. ✅

## Applied

(None — awaiting human approval before apply phase.)
