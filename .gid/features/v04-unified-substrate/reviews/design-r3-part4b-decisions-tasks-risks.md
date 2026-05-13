# Design Review r3-part4b — v04-unified-substrate (Decisions, Tasks, Risks)

> **Reviewer:** claude (sub-agent, part 4b)
> **Date:** 2026-05-12
> **Target:** `.gid/features/v04-unified-substrate/design.md` §7–§10 (lines 1685–2120)
> **Prior review:** `reviews/design-r2-part4-infra-meta.md` — resolved findings not re-raised
> **Method:** 27-check review-design skill, depth=standard (post commit-5 debt cleanup)

## Summary

| Severity   | Count |
|------------|-------|
| 🔴 Critical   | 1   |
| 🟡 Important  | 6   |
| 🟢 Minor      | 2   |
| **Total**  | **9**   |

**Recommendation**: Needs fixes before implementation. FINDING-1 (§4.12 vs §6.1 empathy op name mismatch) is critical — commit-5 fixed the enum but left §4.12's writer-path list pointing at different ops. FINDING-7 (§4.7 supersession has no tasks) is the biggest gap in §8 coverage.

**Estimated implementation confidence**: Medium-high — §7 resolved decisions are well-grounded; §8 task list is comprehensive but has dependency ordering gaps and one coverage hole; §9 risks need structural cleanup (R1-R7 are under-specified).

---

## FINDING-1 🔴 Critical — §4.12 writer ops and §6.1 WriteOp empathy variants are two incompatible sets; §8.10 T49 references the wrong one

**Check #6, #4 (data flow completeness, consistent naming)**

§4.12 explicitly lists 4 writer-queue ops by name:
- `WriteAlignmentEdge { memory_id, drive_id, score }`
- `WriteActionOutcome { ... }`
- `UpdateDriveReinforcement { drive_id, delta }`
- `LogExternalWrite { target, content_hash }`

Commit-5 replaced the generic `WriteEmpathySignal` in §6.1 with 4 **different** variants:
- `WriteEmpathyAccumulator { agent_id, accumulator_kind, delta }`
- `WriteEmpathyEvent { source_id, kind, magnitude, target_agent }`
- `WriteEmpathyResonance { event_a, event_b, strength }`
- `WriteEmpathyCorrection { old_event, new_event, reason }`

These are **two completely disjoint sets of 4 ops** — zero overlap. §4.12's ops model drive alignment, action outcomes, and audit logging. §6.1's ops model empathy accumulation, resonance, and correction. Neither set subsumes the other.

§8.10 T49 says: "Refactor `bus/` to drain into single writer queue (see §6.1 `WriteEmpathyEvent`)" — referencing one variant from §6.1 but none from §4.12.

**Impact**: An implementer of T49 will implement the §6.1 variants and miss the 4 §4.12 ops entirely (drive alignment edges, action outcomes, drive reinforcement, external write audit). Or they'll implement §4.12's ops and find no matching §6.1 variants.

**Suggested fix**: Reconcile the two sets. Likely resolution: §6.1's 4 empathy variants are the *corrected* design (commit-5 was intentional), so §4.12's "Writer paths through §6 queue" list must be updated to match §6.1's variant names. Additionally, 3 of §4.12's ops (`WriteAlignmentEdge`, `UpdateDriveReinforcement`, `LogExternalWrite`) model real functionality not captured by §6.1's variants — either add them as additional variants in §6.1, or show how they map to the existing 4 (e.g., `WriteAlignmentEdge` → `WriteEmpathyEvent` with `kind=AlignmentScore`?). T49 description should list all variants the refactor targets.

---

## FINDING-2 🟡 Important — §4.9 still offers two designs ("becomes nodes OR kept as audit table — decision in §7 Q5") despite §7.5 resolving this

**Check #2, #4 (references resolve, consistent naming)**

§4.9 body says:

> `promotion_candidates` becomes nodes of kind `'promotion_candidate'` linked via `edges` (kind=provenance, predicate=`promotion_source`) to source memories. **Or kept as audit table — decision in §7 Q5.**

But §7.5 has already resolved Q5: "promotion_candidates remains a dedicated table (current schema unchanged). It does NOT become a `node_kind`." The decision is closed with well-reasoned justification (promotion candidates are algorithm scratchpad, not cognitive entities).

§4.9 still presents the question as open with two alternatives. An implementer reading §4 linearly will be confused about which design to build.

**Suggested fix**: Rewrite §4.9 unified section to state the resolution directly:

```
**Unified**: `promotion_candidates` remains as-is (dedicated audit table per §7.5).
It is NOT unified into nodes — promotion candidates are algorithm state, not cognitive
entities. See §3.5 (audit tables retained) and §7.5 for reasoning.
```

---

## FINDING-3 🟡 Important — §7.6 says "no writer" for triples but `memory.rs:4721` has an active `store_triples()` call path

**Check #29, #34 (ground truth verification, breaking-change risk)**

§7.6 states: "drop `triples` table. **0 rows in production, no writer, no reader.** Dead schema."

**Ground truth** (verified via grep): `crates/engramai/src/memory.rs:4721` contains:
```rust
let result = extractor.extract_triples(content);
...
self.storage.store_triples(id, triples)?;
```

And `storage.rs` has a `store_triples()` method. There IS a writer — it's the triple extractor pipeline. The reason production has 0 rows is likely that the triple extractor is disabled/never triggered, not that no code path exists.

**Impact**: §7.6's "no writer" claim is factually wrong. If the triple extractor is ever enabled (or is running but producing empty results), dropping the table in Phase F (T39) without removing the writer code path will cause a runtime error. More importantly, T39 says DROP `triples` but no task removes the Rust code that writes to it — `store_triples()` and the triple extractor would become dead code with a broken SQL target.

**Suggested fix**: (a) §7.6 should say "0 rows in production, writer exists but is effectively dead (triple extractor not triggered in production)" — not "no writer". (b) Add a task (or extend T39) to remove `store_triples()` from `storage.rs` and the triple extraction call in `memory.rs:4721` as part of Phase F cleanup. This is a code-deletion companion to the schema DROP. (c) Verify the `triple_extractor.rs` module disposition — does §4.16 KC retirement cover it, or is it separate?

---

## FINDING-4 🟡 Important — §9 risks R1-R7 all lack structured (likelihood, impact, trigger/detection) fields

**Check #19, #20 (cross-cutting concerns, appropriate abstraction level)**

Each risk in §9 should have 4 fields: (a) likelihood, (b) impact, (c) mitigation, (d) trigger/detection. R8-R11 (added in commit 4) are more thorough, but R1-R7 (original) have only a brief mitigation and sometimes not even that:

| Risk | Likelihood | Impact | Mitigation | Trigger/Detection |
|------|-----------|--------|------------|-------------------|
| R1 | ❌ | ❌ | ✅ brief | ❌ |
| R2 | ❌ | ❌ | ✅ brief | ❌ |
| R3 | ❌ | ❌ | ✅ "mitigated by design" | ❌ |
| R4 | ❌ | ❌ | ✅ brief | ❌ |
| R5 | ❌ | ❌ | ✅ brief | ❌ |
| R6 | ❌ | ❌ | ✅ brief | ❌ |
| R7 | ❌ | ❌ | ✅ brief | ❌ |
| R8 | ❌ | ❌ | ✅ good | partial ✅ |
| R9 | ❌ | ✅ implicit | ✅ good | partial ✅ |
| R10 | ❌ | ✅ implicit | ✅ good | ❌ |
| R11 | ❌ | ❌ | ✅ good | ✅ (ISS-111 gate) |

No risk has explicit likelihood or impact ratings. R1-R7 are one-liners that could each be misinterpreted. Example: R1 "Schema rev mid-implementation" — how likely is this? What's the blast radius if it happens? When do we detect it? The mitigation ("§3 is locked before Phase A starts") is good but incomplete — what if a review finding forces a §3 change after Phase A?

**Suggested fix**: Add structured fields to each risk. At minimum: `Likelihood: low/medium/high`, `Impact: low/medium/high/critical`, and `Detection: [how/when we'd notice]`. For R1 specifically, add: "Detection: any r-N review finding that touches §3 schema after T05 starts. Impact: critical — requires Phase A rollback and schema re-migration."

---

## FINDING-5 🟡 Important — §8 tasks T45-T60 have no explicit dependency on T61 (writer queue), but all require it

**Check #12, #21 (ordering sensitivity, ambiguous prose)**

r2 FINDING-A4-12 flagged that T61-T68 (writer queue) is numbered after T12-T18 (Phase B dual-write) despite being a prerequisite. Commit-5 did NOT fix this — the T-numbering is unchanged.

The problem extends beyond Phase B: §8.9-§8.14 tasks (T45-T60) all emit writes through the §6 queue:
- T45-T48 (interoception): `WriteAnomalyEvent`, `UpdateDomainStats`
- T49-T50 (empathy): `WriteEmpathyEvent` etc.
- T51-T53 (WM): `WriteWmSnapshot`
- T54-T55 (metacog): `WriteMetaJudgment`, `WriteFeedbackEvent`
- T56-T59 (dimensions): writes through `WriteMemory`

**None of these can be implemented until T61 (WriteOp enum) and T62 (writer loop) exist.** But no `depends_on` field makes this explicit. An implementer picking up T45 first would have no writer queue to emit into.

**Suggested fix**: Add a header note to §8.9-§8.14: "All tasks in §8.9-§8.14 depend on T61-T62 (writer queue infrastructure). Implement §8.15 first." Or reorder: move §8.15 before §8.3 (since Phase B also depends on it per §6.8), and add explicit `depends_on: T61, T62` to T12, T45, T49, T51, T54, T56.

---

## FINDING-6 🟢 Minor — §10 "Next step" is not singular — lists 3 actions (T01, T03, T60 blocking note)

**Check #20 (appropriate abstraction level)**

§10's "Next step" paragraph contains:
1. "T01 → spawn review-design sub-agent"
2. "Then T03 (requirements.md)"
3. "Blocking: T60 blocks on ISS-111"

A next step should be singular and immediately actionable. T01 is the obvious next action, but T03 and the ISS-111 note are mixed in, making it unclear whether T03 is "next" or "after next".

**Suggested fix**: Structure as: "**Next step**: T01 — review-design this doc, apply findings. **After T01**: T03 — write requirements.md. **Blocked (not next)**: T60 awaits ISS-111 resolution."

---

## FINDING-7 🟡 Important — §8 coverage gap: §4.7 supersession has no dedicated task; unified supersession path is unimplemented

**Check #6, #3 (data flow completeness, no dead definitions)**

Cross-referencing §4.x features with §8 tasks:

| §4.x | §8 tasks | Coverage |
|-------|----------|----------|
| §4.1 Memory ingest | T12 | ✅ |
| §4.2 Entity resolution | T13 | ✅ |
| §4.3 Hebbian | T14 | ✅ |
| §4.4 KC | T15 | ✅ |
| §4.5 Synthesis | T16 | ✅ |
| §4.6 Decay/forget | T34-T36 (implicit in remove legacy) | ✅ (implicit) |
| §4.7 Supersession | **none** | ❌ |
| §4.8 Retrieval plans | T29 | ✅ |
| §4.9 Promotion | (audit table, no change needed per §7.5) | ✅ |
| §4.10 Episodes | T19, T22 backfill + T41 DROP columns | ✅ |
| §4.11-§4.16 | T45-T60 | ✅ |

§4.7 specifies a non-trivial unification: 4 different supersession mechanisms collapse into 2 (`nodes.superseded_by` + `edges.supersedes/invalidated_by`). This requires:
- Modifying the code that currently writes `memories.superseded_by` to write `nodes.superseded_by`
- Modifying code that writes `graph_entities.merged_into` to use the same path
- Modifying code that writes `memories.contradicts` to use edge supersession
- Backfilling existing supersession relationships

None of these have dedicated tasks. T12 (dual-write store_raw) might cover memory supersession incidentally, but entity merge supersession and contradiction tracking have no task.

**Suggested fix**: Add tasks for §4.7 implementation. Minimum: (a) a Phase B task for dual-writing supersession through the unified mechanism, (b) a Phase C task for backfilling existing `superseded_by`/`merged_into`/`contradicts` values into the unified columns, (c) a Phase E task for removing the legacy supersession write paths.

---

## FINDING-8 🟡 Important — §9 R9 still says "dedicated tokio task" despite commit-5 migrating writer to dedicated OS thread (§6.2)

**Check #4, #32 (consistent naming, conflicts with existing architecture)**

§10 commit-5 changelog item (h) explicitly states: "§6.2 writer loop migrated from `async fn` on tokio task to `fn` on dedicated OS thread."

But R9's mitigation (a) still says: "writer loop runs in **dedicated tokio task** with panic-catcher + auto-restart (T66)."

This is stale — the tokio-task language was the pre-commit-5 design. The dedicated OS thread changes the recovery model (thread panic via `JoinHandle` monitoring, not tokio task panic via `catch_unwind`). T66 was also rewritten in commit-5 to say "writer supervisor" with "auto-restart writer task on crash with fresh `Storage` handle" — but T66 still uses "task" language that could mean either.

**Suggested fix**: R9 mitigation (a): change "dedicated tokio task" to "dedicated OS thread (`std::thread::spawn`)". Also verify T66 says "thread" not "task".

---

## FINDING-9 🟢 Minor — §7.2 subsection still numbered "6.2.1" (r2 FINDING-A4-11, listed as deferred doc-debt in §10 but trivial to fix)

**Check #2 (references resolve)**

r2 FINDING-A4-11 flagged that §7.2 contains a subsection labeled "6.2.1" which should be "7.2.1". §10's "known doc-debt" list includes this as "A1-5" but it's a 3-character fix. Since this review is post commit-5, noting it's still present.

**Suggested fix**: Change "6.2.1" → "7.2.1" in §7.2.

---

<!-- FINDINGS -->

## ✅ Passed Checks

- **Check #0**: Document size — §7-§10 is 4 sections (decisions, tasks, risks, status). Within scope. ✅
- **Check #1**: Types fully defined — §7 decisions reference types from §3/§4 which are complete. No new types introduced in §7-§10. ✅
- **Check #2**: References resolve — §7.5 → §4.9 ✅, §7.6 → Phase F/T39 ✅, §7.7 → Phase B/§5.2 ✅, §8 → §5 phases all present ✅. Exception: §7.2 "6.2.1" misnumbering (FINDING-9, minor). Exception: §4.9 stale ref (FINDING-2).
- **Check #3**: No dead definitions — all §7 decisions referenced from §4/§5/§8. All §9 risks reference design sections. ✅ Exception: §4.7 supersession defined but no §8 task (FINDING-7).
- **Check #5**: §7.7 exit criteria — all 3 criteria are concrete and measurable (row-count parity, Jaccard ≥0.99 on 95%, J-score ±1pp). 7-day window is specified. Human go-decision gate is explicit. No deadlock risk. ✅
- **Check #7**: Error handling — §7.7 has explicit rollback path ("roll back the fan-out without consumer impact"). Phase B failure semantics are clear. ✅
- **Check #8**: No string slicing in §7-§10. ✅
- **Check #10**: No `.unwrap()` in §7-§10 (no pseudocode in these sections). ✅
- **Check #13**: Separation of concerns — §7.5 correctly distinguishes cognitive substrate from algorithm scratchpad (promotion_candidates as audit, not graph). ✅
- **Check #14**: Coupling — §7.7 exit criteria read from both substrates independently (legacy + unified), no coupling between verification and production path. ✅
- **Check #15**: Configuration — §7.7 thresholds (0.99 Jaccard, 95%, ±1pp) are numeric, could be configurable. Currently hardcoded in the design but these are acceptance criteria, not runtime config — appropriate. ✅
- **Check #17**: Goals explicit — §7 preamble states the decision framework (engram thesis: pattern → graph, bookkeeping → audit). Each Q has explicit reasoning. ✅
- **Check #18**: Trade-offs — §7.5 documents the rejected alternative (promotion_candidate as node_kind) with principled reasoning. §7.4 documents the rejected alternative (keep episode_id columns). ✅
- **Check #30**: Technical debt — §7.7 explicitly calls out dual-write as NOT a "temporary" state but a verification window with forced decision point. This is anti-debt design. ✅
- **Check #31**: Shortcut detection — §7.5 doesn't take the shortcut of "just make everything a node." The audit-vs-cognitive distinction is a root-cause analysis. ✅
- **Check #33**: Simplification — §7.7 doesn't simplify exit criteria to "looks right." Quantitative thresholds with time windows. ✅
- **Check #35**: Purpose alignment — all §7 decisions serve the stated migration goal. No speculative decisions. ✅
- **§7.5 vs §4.9 contradiction check** (focus area): §7.5 correctly decides promotion_candidates stays as audit table. This does NOT contradict §4.9 — §4.9 is just stale (still shows both options). No architectural conflict. ✅ (but §4.9 needs update — see FINDING-2)
- **§7.6 drop triples in Phase F** (focus area): T39 includes triples in the DROP list. §5.6 says "drop legacy tables" — triples is legacy. Consistent. ✅ (but "no writer" claim is wrong — see FINDING-3)
- **§7.7 exit criteria measurability** (focus area): All 3 criteria are measurable. "Who decides exit" → §7.7 says "explicit human go-decision required after reviewing §7.7 metrics." Clear. ✅
- **§8 task IDs** (focus area): All tasks have stable T-prefix IDs (T01-T68, T26a-c). Suitable for graph wiring. ✅
- **§8 task decomposition** (focus area): Most tasks are ≤1 day. T26a-c explicitly split a multi-day task. T19-T25 are each single backfill drivers. Reasonable granularity. ✅
- **§8 vs §5 phase coverage** (focus area): Every §5 phase (A-F) has a corresponding §8 section (§8.2-§8.7). ✅
- **§10 actionability** (focus area): Next step identifies T01 as the immediate action. Partially singular — see FINDING-6. ✅ (minor)

## Applied

(None — awaiting human approval before apply phase.)
