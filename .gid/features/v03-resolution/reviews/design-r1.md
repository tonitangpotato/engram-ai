# Design Review r1 — v03-resolution

> **Reviewer:** RustClaw (main agent)
> **Date:** 2026-04-24
> **Target:** `.gid/features/v03-resolution/design.md` (~900 lines)
> **Requirements:** `.gid/features/v03-resolution/requirements.md` + `.gid/docs/requirements-v03.md` (master GUARDs)
> **Method:** 27-check review-design skill, depth=standard, incremental write protocol v1.1

## Summary

| Severity   | Count |
|------------|-------|
| Critical   | 0     |
| Important  | 6     |
| Minor      | 9     |
| **Total**  | **15**|

Overall quality: **high**. Design is carefully reasoned, the §4.2 preserve-on-silence contract (from prior r1 iteration) is root-caused, and the bi-temporal story is coherent. Main issues: §5bis scope traceability, naming drift (`ingest` vs `store_raw`), three-number threshold confusion for GUARD-12 budget, and several undefined trace subtypes. No blocking findings — findings are clarification/tightening work, not redesign.

---

## FINDING-1 🟡 Important ✅ Applied — §5bis "Knowledge Compiler" scope violation (satisfies retrieval GOALs without tracing)

**Section:** §5bis (entire section, ~100 lines)

**Problem:** §5bis defines `compile_knowledge()` as a scheduled compaction job that produces L5 abstractions. This is substantial scope (~100 lines covering API, algorithm, config, metrics, trace types). But:

- Resolution requirements (`.gid/features/v03-resolution/requirements.md`) are all in the **GOAL-2.X** namespace and cover entity/edge pipeline only. L5 synthesis is **not** listed.
- The L5-related GOALs (GOAL-3.6 "L5 on-demand synthesis", GOAL-3.7 "L5 cost isolation") live in **v03-retrieval's namespace** (GOAL-3.X), and v03-retrieval §4.4 explicitly defers L5 synthesis to "v03-resolution §5bis.3".
- §2 traceability table in resolution design does NOT list any GOAL-3.X — §5bis has no satisfying requirement in its own feature's namespace.

Result: §5bis is architecturally homeless. It's in resolution because it's a write-path job, but it satisfies retrieval requirements without being traced.

**Suggested fix:** Pick one:

- **(A)** Add GOAL-2.15 / GOAL-2.16 to `.gid/features/v03-resolution/requirements.md` explicitly claiming ownership of the Knowledge Compiler produce-side. Update §2 traceability. This is the cleanest root fix.
- **(B)** Move §5bis into a new feature doc `v03-knowledge-compiler/design.md` with its own GOALs. Resolution design retains only a brief §5bis stub referencing the new feature.
- **(C)** Keep §5bis here but add an explicit "Satisfies" row in §2 citing cross-feature GOAL-3.6, GOAL-3.7 with a note that Knowledge Compiler produce-side is owned here by architectural proximity (it shares the write-path worker pool).

Option (A) is simplest. (B) is cleanest if §5bis grows further.

**Rationale:** Every design section must trace to a requirement. Cross-feature traceability is allowed but must be explicit — otherwise §5bis is scope creep hidden by feature-doc separation.

**Applied**: Chose option (C). Added cross-feature GOAL-3.6 / GOAL-3.7 rows to §2 traceability table with architectural-proximity note; §5bis.1 header now includes an explicit "Satisfies (cross-feature)" block pointing back to §2.

---

## FINDING-2 🟡 Important ✅ Applied — `ingest()` naming drift: method referenced but never defined

**Section:** §6.1, §6.4, v03-benchmarks §3.3

**Problem:** The canonical public write API is `store_raw(raw, meta) -> MemoryId` (§6.1). But §6.4 says:

> "Per-`ingest()`-call counters feed the summary. [...] `ingest_with_stats(raw, meta) -> (MemoryId, ResolutionTraceSummary)`."

`ingest()` (bare) is referenced but never defined. Only `store_raw` (the v0.2 API) and `ingest_with_stats` (benchmark/test variant) exist. An external reader can't tell whether `ingest()` is a third API, an old name, or a typo.

The v03-benchmarks doc's §3.3 also references `ingest()` for the steady-state throughput metric, amplifying the ambiguity.

**Suggested fix:** Do a global search/replace in this design:

- "`ingest()`" → "`store_raw()`" (when referring to the public write entry point)
- "`ingest_with_stats()`" stays as-is (it IS distinct — returns trace summary)
- Update §6.4 counter docstring: "Per-`store_raw()`-call resolution trace summary, surfaced via `ingest_with_stats()` in tests/benchmarks."

Coordinate with v03-benchmarks: if benchmarks time `store_raw` but call it "`ingest`", either rename the metric or switch to `ingest_with_stats`.

**Rationale:** Naming drift in public APIs causes implementation bugs and doc confusion. `store_raw` is the v0.2-compat name (GUARD-11 requires preservation); it should be the only name used in this design.

**Applied**: §6.4 motivation, `ResolutionStats` docstring, and access-path prose rewritten to reference `store_raw()` / `ingest_with_stats()` consistently; bare `ingest()` references removed.

---

## FINDING-3 🟡 Important ✅ Applied — §2 traceability missing GUARD-9 and GUARD-11

**Section:** §2

**Problem:** Resolution requirements declare GUARD-1, GUARD-2, GUARD-3, GUARD-6, GUARD-8, GUARD-9, GUARD-11, GUARD-12 as applicable. §2 only lists GUARD-1, GUARD-2, GUARD-3, GUARD-6, GUARD-8, GUARD-12. Missing:

- **GUARD-9** (no new deps) — Heavily implicit: §6.1 keeps v0.2 API contract, §3.4 uses `decision_fusion` helpers that presumably exist. But no explicit trace line shows the design respects "no new external dependencies beyond v0.2".
- **GUARD-11** (v0.2 API compat) — This one is THE central constraint of §3.1 and §6.1 (the `store_raw` split). Omitting it from §2 is a documentation gap.

**Suggested fix:** Add rows to §2:

```
| GUARD-9 (hard) — no new deps                    | §3 pipeline uses only crates already in v0.2 workspace (list specific crates); §5bis compile job uses same Anthropic provider as classifier/extractor |
| GUARD-11 (hard) — v0.2 API compat                | §3.1 `store_raw` signature unchanged; §6.1 same return type; §9.4 regression tests assert existing `tests/` pass unmodified |
```

**Rationale:** §2 is the index readers use to verify GUARD coverage. Missing rows = missing test hooks. GUARD-11 especially — it's structurally defining for this design; absence from the table is a self-inflicted review gap.

**Applied**: §2 GUARD alignment block now includes GUARD-9 (enumerates reused crates + LLM providers, no new deps) and GUARD-11 (cites §3.1 `store_raw` signature preservation, §6.1 return type, §9.4 regression tests).

---

## FINDING-4 🟡 Important ✅ Applied — GOAL-2.14 warn threshold (4) vs GUARD-12 target (2-3) vs §7 "≤4" — three numbers for one budget

**Section:** GOAL-2.14, master GUARD-12, §7

**Problem:** Three different thresholds for per-episode LLM calls appear:

- **master GUARD-12**: target 2-3 avg per episode (soft).
- **GOAL-2.14**: warn if rolling avg >4 (2.14 is the resolution feature's own goal).
- **§7 observability**: `resolution_llm_calls_rolling_avg` "warn when > 4".

Operators reading these get three distinct numbers with no stated relationship. Is 2-3 the ship-gate target, 4 the degradation warn threshold, and something else the fire-alarm threshold? Or is GOAL-2.14 a relaxation of GUARD-12? The design should reconcile explicitly.

**Suggested fix:** Add a one-line reconciliation in §2 (or §7):

> "Budget tiers for per-episode LLM calls (GUARD-12 / GOAL-2.14):
> - **Target** (ship gate): avg ≤ 3 per episode (happy path: classifier 1 + extractor 1 + rare LLM tiebreaker ≤ 1 avg)
> - **Warn** (soft alarm): rolling avg > 4 — triggers `resolution_llm_calls_over_budget_total` counter and operator-visible warning
> - **Fail** (not set): there is no hard cap on per-episode calls; budget violations are advisory only, consistent with GUARD-12 being soft severity."

**Rationale:** Budget observability is only useful if tiers are clear. Three numbers with unclear relation → operator alert fatigue or ignored warnings.

**Applied**: §2 GUARD-12 row now contains an explicit 3-tier reconciliation: Target (≤ 3 ship gate), Warn (> 4 rolling avg → `resolution_llm_calls_over_budget_total`), Fail (none — advisory only).

---

## FINDING-5 🟡 Important ✅ Applied — `on_tiebreak_failure` default "Abort" contradicts "CreateNew is safe-by-default" framing

**Section:** §8.1

**Problem:** §8.1 says:

> "If `config.on_tiebreak_failure = Conservative`, defaults to CreateNew (safe-by-default). [...] If `Abort`, the run aborts, memory enters Failed state. Default is Abort to honor GUARD-2."

Two problems:

1. "CreateNew is safe-by-default" and "Abort is the default to honor GUARD-2" are contradictory framings. If CreateNew is safe, it should be the default.
2. GUARD-2 says "never silent degrade" — both Abort (with typed failure) and Conservative (with trace entry) satisfy GUARD-2. The distinction is between "fail fast, lose work" (Abort) and "preserve work, accept duplicate risk" (Conservative).

Operators picking a default want the one that preserves the most extraction work. Conservative does that. Abort throws away a successful classifier + extractor run because one edge tiebreak went bad.

**Suggested fix:** Change default to `Conservative` (CreateNew) and rewrite:

> "`on_tiebreak_failure` controls behavior when the LLM tiebreaker fails (timeout, error) for an entity resolution:
> - **`Conservative` (default)** — Creates a new entity (method=`Automatic`, confidence=low, trace entry flags `tiebreak_failed=true`). Preserves extraction work; risks a duplicate entity that the agent can merge later (§6.3 `agent_curate_entity`). Satisfies GUARD-2 via the trace entry.
> - **`Abort`** — Memory enters `Failed(tiebreak_unavailable)` state; re-run via `reextract --failed`. Loses extraction work but prevents any duplicate entities. Use when duplicate-free invariants matter more than throughput."

If the author genuinely wants Abort as default for safety reasons, keep it but drop the "CreateNew is safe-by-default" line — the two statements can't both be true.

**Rationale:** Default behavior matters for operational cost. The current framing is self-contradictory; pick one story and defend it.

**Applied**: §8.1 `on_tiebreak_failure` rewritten — default flipped to `Conservative`; both modes framed as GUARD-2-compliant (trace entry vs typed failure); `Abort` kept as opt-in for strict duplicate-free invariants.

---

## FINDING-6 🟡 Important ✅ Applied — Hybrid silent-drop pattern: signal redistribution formula unspecified

**Section:** §3.4.2

**Problem:** §3.4.2 says:

> "If any signal is unavailable, its weight is redistributed proportionally across the remaining signals."

But the formula is never given. Given weights `w = [w1..w8]` summing to 1.0, if signals 2 and 5 are unavailable, is the redistribution:

- **(A) Proportional to remaining weights:** each remaining weight `wi ← wi / sum(remaining)` — maintains relative importance.
- **(B) Uniform:** `wi ← wi + (w2+w5)/6` for each of the 6 remaining — loses relative importance.
- **(C) Drop-with-renorm:** same as (A), but equivalent to not redistributing at all since scores are compared in relative ranking anyway.

These give different decisions. For determinism (GOAL-2.11 / GUARD-4 fusion determinism), the formula must be exact.

**Suggested fix:** Specify in §3.4.2:

> "**Redistribution formula**: for a present signal i with original weight wi, if signals in set M are missing, the redistributed weight is:
> `wi' = wi + wi * sum(w_j for j in M) / sum(w_k for k not in M)`
> Equivalently, `wi' = wi / (1 - sum(missing))`. This preserves relative importance among present signals.
>
> Example: if s2 (w=0.15) and s5 (w=0.20) are missing, sum_missing = 0.35, each present wi scales by 1/(1-0.35) = 1.538x."

**Rationale:** Fusion determinism (GOAL-2.11) fails if the formula is implementation-defined. Pinning it here makes the §9.3 property test tractable.

**Applied**: §3.4.2 now specifies `w_i' = w_i / (1 − sum_missing)` with worked example (s2, s5 missing → 1.538× scale) and degenerate case (all missing → `CreateNew` + `StageFailure { kind: NoFusionSignals }`).

---

## FINDING-7 🟢 Minor ✅ Applied — Episode mis-route: Episodic plan's analogue not defined for Resolution

**Section:** §3 general

**Problem:** Retrieval §4.2 has a well-defined fallback when classifier mis-routes (see retrieval FINDING-9). Resolution has an analogous concern: what if extractor produces 0 entities/0 edges for a memory? §3 never specifies. Does:

- Memory stay in `Pending`? Move to `Done(empty)`? Move to `Failed`?
- Is there a re-extract-on-empty policy, or does operator need to re-run manually?

§3.3.2 extractor step says "0 mentions, 0 edges — memory finalizes as 'no semantic content'". That's a defined state, good. But it's not surfaced in any status enum (§7 doesn't list `Done(empty)` as a distinct status from `Done`).

**Suggested fix:** Clarify `ExtractionStatus` enum variants in §7:

```rust
pub enum ExtractionStatus {
    Pending { queued_at: SystemTime, reason: Option<String> },
    Running { started_at: SystemTime, worker_id: String },
    Done { finished_at: SystemTime, had_semantic_content: bool }, // explicit flag
    Failed { error: ExtractionError, failed_at: SystemTime, retry_count: u32 },
}
```

The `had_semantic_content` flag lets operators filter "memories with zero extractions" without re-scanning the graph.

**Rationale:** Minor but helps operability. Avoids silent "empty" done states that look identical to real dones.

**Applied**: §6.3 `ExtractionStatus::Completed` now carries a `had_semantic_content: bool` field with docstring referencing §3.3.2 "no semantic content" state.

---

## FINDING-8 🟢 Minor ✅ Applied — Several trace subtypes referenced but not defined

**Section:** §7.1

**Problem:** `ResolutionTraceSummary` fields reference types never defined in this doc: `EntityDecisionRecord`, `EdgeDecisionRecord`, `StageFailure`, `AffectSource`, `SignalsSummary`, `DecisionMix`. Implementation can invent shapes; reviewers can't verify trace completeness.

**Suggested fix:** Add §7.2 "Trace subtype definitions" with minimal sketches:

```rust
pub enum AffectSource { EpisodeSnapshot, None }

pub struct EntityDecisionRecord {
    pub mention: String,
    pub decision: EntityDecision,  // Merge | CreateNew | LinkExisting
    pub method: ResolutionMethod,  // Automatic | LlmTieBreaker | AgentCurated | Migrated
    pub confidence: f32,
    pub candidate_count: usize,
    pub signals: SignalsSummary,
}

pub struct SignalsSummary { pub present: Vec<SignalKind>, pub missing: Vec<SignalKind> }

pub struct StageFailure {
    pub stage: Stage,  // Classify | Extract | ResolveEntities | ResolveEdges | Persist
    pub error_kind: String,
    pub retryable: bool,
}

pub struct DecisionMix {
    pub automatic: u32,
    pub llm_tiebreaker: u32,
    pub agent_curated: u32,
    pub migrated: u32,
}
// ... etc
```

Not Rust-complete, just enough that implementers know field shapes and reviewers know what to assert.

**Rationale:** Observability (GOAL-2.7 / GOAL-2.12) is only as good as trace shapes. Undefined = ambiguous instrumentation.

**Applied**: Added §7.2 "Trace Subtype Definitions" with Rust sketches for `AffectSource`, `SignalKind`, `SignalsSummary`, `EntityDecision`, `EntityDecisionRecord`, `EdgeAction`, `EdgeDecisionRecord`, `Stage`, `StageFailure`, `DecisionMix`.

---

## FINDING-9 🟢 Minor ✅ Applied — `AffectSource::None` variant conflicts with GUARD-8

**Section:** §7.1

**Problem:** §7.1 lists `AffectSource: EpisodeSnapshot | None`. But GUARD-8 (affect_snapshot immutable) + the design's own claim that resolution reads affect strictly from the episode snapshot means `None` should be impossible in practice — every extracted entity/edge SHOULD have an affect source (the episode it came from).

If `None` is used for "memory had no affect data", then the variant should say so explicitly: `EpisodeSnapshot | NoAffectAvailable(reason: String)`.

**Suggested fix:** Replace with:

```rust
pub enum AffectSource {
    EpisodeSnapshot { memory_id: MemoryId },        // normal case
    NoAffectAvailable { reason: String },            // e.g., legacy memory pre-v0.3
}
```

The reason string tells operators why a particular extracted edge has no affect trace.

**Rationale:** Two-variant enum with unexplained `None` variant is a code smell — the variants should self-document.

**Applied**: §7.1 `affect_snapshot_source` comment updated to `EpisodeSnapshot | NoAffectAvailable`; §7.2 `AffectSource` enum uses self-documenting variants `EpisodeSnapshot { memory_id }` and `NoAffectAvailable { reason: String }`.

---

## FINDING-10 🟢 Minor ✅ Applied — Worker pool N=1 default hides concurrency design

**Section:** §5.1

**Problem:** §5.1 defaults `workers: N=1` to "simplify reasoning". With N=1, GOAL-2.4 (concurrent correctness) is trivially satisfied but untested. When an operator scales to N=2+ for throughput, the "session-affinity dispatch" described in prose needs to actually exist and be tested.

Specifically undefined:
- Hashing function for session_id → worker assignment
- Behavior when memory has no session_id (edge case for standalone memories)
- What happens on worker crash mid-job (queue re-assignment? lost job?)

**Suggested fix:** Add §5.1.1 "Concurrency details":

> "Worker dispatch: `worker_id = hash(session_id) % N`. Memories with no session_id use `worker_id = hash(memory_id) % N` (treating memory as a singleton session).
> 
> On worker crash: queued jobs assigned to the crashed worker are reassigned on restart; in-flight jobs are marked `Failed(worker_crashed)` and must be re-extracted manually. (Automatic retry is out of scope per requirements.)
>
> Testing: GOAL-2.4 property test runs with N=1, N=2, and N=4 to cover single-worker, inter-worker, and fan-out paths."

**Rationale:** Concurrency GUARD by implication is weak coverage. Making the dispatch explicit + testing multiple N values proves GOAL-2.4 rather than trivially satisfying it.

**Applied**: Added §5.1.1 "Concurrency Details" — `worker_id = hash(session_id) % N` (FxHash), no-session fallback, worker-crash semantics (queued → re-enqueue; in-flight → `Failed(worker_crashed)` + operator replay), and N ∈ {1, 2, 4} property-test matrix.

---

## FINDING-11 🟢 Minor ✅ Applied — `K_seed` / `candidate_top_k` hard cap enforcement unspecified

**Section:** §3.4.1

**Problem:** §3.4.1: "`config.candidate_top_k` default 10, hard cap 50". Where is the cap enforced? Config validation at load? Runtime clamp? Panic? If someone sets 100 in config, does the system error out or silently clamp to 50?

**Suggested fix:** Specify in §3.4.1:

> "`candidate_top_k` is clamped at runtime: `top_k = min(config.candidate_top_k, 50)`. A warning is logged if the user-configured value exceeded the cap. Config validation does NOT fail on over-cap values (forward compatibility — future versions may raise the cap)."

Or the stricter version:

> "Config validation rejects `candidate_top_k > 50` at load time with `InvalidConfig` error. The cap may be raised in future versions."

Either is fine — just pick one.

**Rationale:** "Hard cap" without enforcement story is documentation debt.

**Applied**: §3.4.1 adds "Cap enforcement" paragraph: runtime clamp `min(config.candidate_top_k, 50)` with once-per-process warning log; config load does NOT fail on over-cap (forward-compat).

---

## FINDING-12 🟢 Minor ✅ Applied — Queue-full behavior: `store_raw` success creates orphaned Pending memories

**Section:** §5.2

**Problem:** §5.2: when extraction queue is full, enqueue fails but `store_raw` still succeeds (L1/L2 written). Memory is then in `Pending(queue_full)` state. Operator recovery is via `reextract --pending`.

The concern: a storm of high-ingest + slow-extract periods creates a large backlog of Pending memories. There's no visible upper bound on Pending depth — nothing prevents the count from growing into millions. Operator finds out when `reextract --pending` takes hours.

This is acceptable for v0.3 MVP but should be surfaced.

**Suggested fix:** Add to §7 observability:

```
- resolution_pending_memories_total{reason}       gauge  — current count of Pending memories by reason (queue_full | awaiting_worker | ...)
- resolution_pending_memory_oldest_age_seconds    gauge  — age of the oldest Pending memory (alert threshold)
```

And add §5.2.1 note:

> "Backlog observability: `resolution_pending_memories_total{reason=\"queue_full\"}` should be monitored. Sustained growth indicates the worker pool is under-provisioned for ingest rate — scale N, raise `queue_cap`, or accept degradation. There is no automatic backpressure on `store_raw` — ingest is never blocked, per GUARD-1 (episodic completeness)."

**Rationale:** GUARD-1 correctly prioritizes episodic write availability over graph-extraction throughput, but operators need visibility into the tradeoff. Silent backlog growth is an operational footgun.

**Applied**: §7.2 metrics table adds `resolution_pending_memories_total{reason}`, `resolution_pending_memory_oldest_age_seconds`, `resolution_llm_calls_over_budget_total`. Added §5.2.1 "Backlog Observability (operator footgun note)" with required monitors and remediation ladder.

---

## FINDING-13 🟢 Minor ✅ Applied — `map_entity_type` table punts to code — risk of v0.2→v0.3 migration drift

**Section:** §3.2, §10

**Problem:** §3.2: "The table lives alongside the code, not in this doc." The mapping has only two examples (`Technology → Artifact`, `IsA → Canonical(IsA)`). If v0.2 has 10+ EntityType variants (Person, Organization, Location, Event, Technology, ...), losing 7 of those from the design doc means:

- Migration correctness is impossible to review without reading code.
- Future v0.3 updates that add new EntityType variants may break the table silently.
- Engineering Integrity Check #32 (Conflicts with existing architecture): the mapping may collide with v0.2 semantics in unclear ways (e.g., what does v0.2 `Event` map to?).

**Suggested fix:** Add full mapping table to §3.2:

```
| v0.2 EntityType | v0.3 EntityType          | Notes           |
|----------------|--------------------------|-----------------|
| Person         | Person                   | lossless        |
| Organization   | Organization             | lossless        |
| Location       | Location                 | lossless        |
| Technology     | Artifact                 | subtype loss    |
| Event          | Event                    | lossless        |
| ...            | ...                      | ...             |
```

If the v0.2 variants are unstable, at minimum list the current ones with a "see `crates/engramai/src/entity.rs::EntityType`" link and a commitment to update on any variant change.

**Rationale:** Migration correctness is a GUARD-11 concern (v0.2 compat). Hiding the mapping in code hides the compat surface.

**Applied**: §3.2 now contains full v0.2 `EntityType` → v0.3 `EntityKind` mapping table (8 rows: Person/Organization/Project/Technology/Concept/File/Url/Other) with lossless vs subtype-loss notes; prose commits to co-updates on v0.2 variant changes.

---

## FINDING-14 🟢 Minor ✅ Applied — §4.2 row 3 "Update-in-place-of-successor" is confusing terminology

**Section:** §4.2, row 3 (present/curated, different confidence > ε)

**Problem:** The action name "Update-in-place-of-successor" suggests mutating a prior edge's successor pointer, which would violate GUARD-3 (append-only). The actual behavior (per prose): create a new edge with `supersedes = prior_edge.id`. This is normal supersede semantics.

Why the awkward name? Maybe to distinguish from row 2's "create new edge with same triple, different source" (= `Link` action). But "Update-in-place-of-successor" is semantically misleading.

**Suggested fix:** Rename row 3 action to `SupersedeCurated` or just `Supersede(reason = \"confidence_changed\")`. Add to §4.2 legend:

> "Supersede creates a new edge with `supersedes = prior_edge.id` and `prior_edge.superseded_by = new_edge.id`; both edges remain in storage (append-only per GUARD-3). Older edges are filterable by bi-temporal query but not silently dropped."

**Rationale:** Terminology matters — "update-in-place" triggers GUARD-3 violation alarm even though the actual behavior is correct.

**Applied**: §4.2 row 3 action renamed from `Update-in-place-of-successor` to `Supersede (reason = confidence_changed)`. Added "Legend — supersede semantics" paragraph explicitly describing the append-only pair (`supersedes` + `superseded_by`) per GUARD-3.

---

## FINDING-15 🟢 Minor ✅ Applied — §4.2 "Preserve on silence" tradeoff not explicitly surfaced as design tradeoff

**Section:** §4.2 legend, §4.5

**Problem:** The core contract — "extractor silence is not a delete signal" — is correct and well-reasoned (captured in r1 design review loop). But it has a real tradeoff that operators should know about:

- **Pro:** Extractor volatility doesn't churn the graph; re-extracts are idempotent for preserve cases.
- **Con:** A fact that becomes untrue (e.g., "Alice works at Acme" → Alice leaves Acme) can only be recorded via an *explicit contradicting edge* or agent curation. Extraction cannot express "this is no longer true".

This is a fundamental choice but buried in prose. An operator expecting "re-extract refreshes the knowledge graph" would be surprised.

**Suggested fix:** Add §4.2.1 "Tradeoff: silence is not delete":

> "**Design tradeoff:** Extractor silence is treated as absence of evidence, not evidence of absence. Consequences:
> 
> - ✅ Re-extracts are stable and safe — volatility doesn't churn the graph.
> - ❌ Facts cannot be retracted by extraction alone. To record 'fact X is no longer true' at time T, the system requires either (a) an explicit contradicting edge from a new episode, or (b) agent curation via `invalidate_edge()`.
> 
> This tradeoff was chosen because extractor output is not statistically reliable enough to distinguish 'the fact became untrue' from 'the extractor missed it this time'. The bi-temporal model (GUARD-3) preserves the historical view regardless."

**Rationale:** Tradeoffs are a Phase 5 design-doc quality check (§18 in review-design skill). Surface the tradeoff explicitly so future operators / designers don't re-litigate it.

**Applied**: Added §4.2.1 "Tradeoff: Silence Is Not Delete" — Pro/Con bullets, explicit statement that retraction requires contradicting edge or agent curation, and operator guidance ("re-extract refines, it does not refresh").

---

<!-- FINDINGS -->

## Applied

### FINDING-1 ✅
- §2 added cross-feature GOAL-3.6/GOAL-3.7 rows; §5bis header now has explicit "Satisfies (cross-feature)" note pointing back to §2 traceability. Chose option (C) — traceability rows with architectural-proximity justification.

### FINDING-2 ✅
- §6.4 motivation, docstring, and access-path prose rewritten: `ingest()` → `store_raw()` / `ingest_with_stats()` (the distinct method name is preserved). No other `ingest()` references remained outside `ingest_with_stats` / generic "ingest" prose.

### FINDING-3 ✅
- §2 GUARD alignment block now includes GUARD-9 (no new deps — enumerates reused crates + LLM providers) and GUARD-11 (v0.2 API compat — cites §3.1, §6.1, §9.4).

### FINDING-4 ✅
- §2 GUARD-12 row now contains an explicit 3-tier reconciliation block: Target (≤ 3 ship gate), Warn (> 4 rolling avg → `resolution_llm_calls_over_budget_total`), Fail (none — advisory only).

### FINDING-5 ✅
- §8.1 `on_tiebreak_failure` rewritten: default flipped to `Conservative`, removed the self-contradictory "safe-by-default vs Abort-honors-GUARD-2" framing, both modes now framed as GUARD-2-compliant (trace entry vs typed failure). Abort is kept as opt-in for strict duplicate-free invariants.

### FINDING-6 ✅
- §3.4.2 now specifies the exact redistribution formula `w_i' = w_i / (1 − sum_missing)` with worked example (s2, s5 missing → 1.538x scale factor), plus degenerate case (all missing → `CreateNew` + `StageFailure { kind: NoFusionSignals }`).

### FINDING-7 ✅
- §6.3 `ExtractionStatus::Completed` now carries a `had_semantic_content: bool` field with docstring referencing §3.3.2 "no semantic content" state.

### FINDING-8 ✅
- Added §7.2 "Trace Subtype Definitions" with Rust sketches for `AffectSource`, `SignalKind`, `SignalsSummary`, `EntityDecision`, `EntityDecisionRecord`, `EdgeAction`, `EdgeDecisionRecord`, `Stage`, `StageFailure`, `DecisionMix`. Existing §7.2 Metrics section pushed (no conflict — new subsection inserted between `ResolutionTrace` and Metrics tables). Note: the Metrics subsection is now logically §7.3 in content order, but header numbering preserved for minimal diff.

### FINDING-9 ✅
- §7.1 `affect_snapshot_source` comment updated to `EpisodeSnapshot | NoAffectAvailable`; §7.2 `AffectSource` enum now carries `EpisodeSnapshot { memory_id }` and `NoAffectAvailable { reason: String }` self-documenting variants (replacing bare `None`).

### FINDING-10 ✅
- Added §5.1.1 "Concurrency Details" — specifies `worker_id = hash(session_id) % N` (FxHash), no-session fallback to `hash(memory_id) % N`, worker-crash semantics (queued → re-enqueue on startup; in-flight → `Failed(worker_crashed)` + operator replay), and N ∈ {1, 2, 4} property-test matrix for GOAL-2.4.

### FINDING-11 ✅
- §3.4.1 adds "Cap enforcement" paragraph: runtime clamp via `min(config.candidate_top_k, 50)` with once-per-process warning log; config load does NOT fail on over-cap (forward-compat).

### FINDING-12 ✅
- §7.2 metrics table adds `resolution_pending_memories_total{reason}`, `resolution_pending_memory_oldest_age_seconds`, `resolution_llm_calls_over_budget_total`. Added §5.2.1 "Backlog Observability (operator footgun note)" spelling out the unbounded-backlog tradeoff, required monitors, and remediation ladder (scale workers → raise queue_cap → scheduled reextract).

### FINDING-13 ✅
- §3.2 now contains full v0.2 `EntityType` → v0.3 `EntityKind` mapping table (8 rows covering Person/Organization/Project/Technology/Concept/File/Url/Other), with notes column flagging lossless vs subtype-loss mappings. Updated prose to commit to co-updates on v0.2 variant changes (GUARD-11 compat surface).

### FINDING-14 ✅
- §4.2 row 3 action renamed from `Update-in-place-of-successor` to `Supersede (reason = confidence_changed)`. Added a "Legend — supersede semantics" paragraph after the decision table explicitly describing the append-only pair (`supersedes` + `superseded_by`) per GUARD-3.

### FINDING-15 ✅
- Added §4.2.1 "Tradeoff: Silence Is Not Delete" — Pro/Con bullets, explicit statement that retraction requires contradicting edge or agent curation, and operator guidance ("re-extract refines, it does not refresh").

### Summary
- Applied: 15/15
- Skipped: 0/15
