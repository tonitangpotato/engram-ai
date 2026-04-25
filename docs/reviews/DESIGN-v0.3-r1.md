# DESIGN-v0.3 Review — r1 (Deep)

> **Reviewer**: RustClaw (potato's agent)
> **Date**: 2026-04-23
> **Target**: `/Users/potato/clawd/projects/engram/docs/DESIGN-v0.3.md` (708 lines, draft dated 2026-04-23)
> **Basis**: Actual `crates/engramai/src/` code graph + DESIGN prose cross-check
> **Review depth**: full (Phase 0–8, 36 checks) — but filtered to architectural issues that matter; skipping cosmetic nits.

## TL;DR

DESIGN-v0.3 is ambitious and mostly correct, but **collides with existing code in three places the document does not acknowledge**, and the Phase 1 timeline ("3 weeks") is not credible given the actual impact radius of the `MemoryRecord` and `storage.rs` changes. Before approval, three architectural decisions must be made explicit: (1) working-memory naming, (2) predicate schema strategy, (3) confidence field semantics. Three further issues require scope/timeline adjustment.

---

## 🔴 Class A — Architectural conflicts (must resolve before coding)

### A1. Two different things are both called "working memory"

**Evidence**:
- DESIGN §2 (line 50, 60, 77): L2 = `working_strength (r1)` trace, "occupancy cap ~100 items" (implied by lifecycle docs)
- Actual code `session_wm.rs` L1–L11: `SessionWorkingMemory` = "Miller's Law-constrained active memory buffer, capacity=7±2"
- DESIGN §3.2 keeps the field `working_strength: f64, // r1` but never mentions `SessionWorkingMemory`

**Problem**: The doc treats L2 as a single concept. The code already has two mechanisms that both claim "working memory": one is a **per-record strength scalar** (r1, ~100 items in the activation set), the other is a **per-session bounded buffer** (7±2 items in the current dialogue). Their roles are different but the names collide. A reader of v0.3 cannot tell which one §2 is talking about.

**Impact**: Any PR touching "working memory" has to pick one, and will silently diverge. Reviewers will not catch it because the naming hides the bug.

**Proposed resolution** (pick one):
- **Option A (rename session buffer)**: rename `SessionWorkingMemory` → `ActiveContext` or `ConversationBuffer`; L2 remains "working_strength". Cleanest but touches every call site.
- **Option B (rename L2)**: relabel L2 in DESIGN → `ShortTermActivation` or `RecencyTrace`; `SessionWorkingMemory` keeps its name. Doc-only change, but divorces "working_strength" field name from its layer label — ugly.
- **Option C (state the containment)**: keep both names, but DESIGN §2 must explicitly state `SessionWorkingMemory ⊂ L2`, where L2 is the population and `SessionWorkingMemory` is the attention slice. Requires one paragraph in §2.

**My recommendation**: **Option A + Option C**. Rename the session buffer (cleaner API surface) **and** document the containment. Cost: ~20 call sites to rename; benefit: future readers never hit this confusion.

---

### A2. Predicate schema: DESIGN says "emergent", code says "closed-set of 9"

**Evidence**:
- DESIGN §3.5 (line 209–221): *"Predicates are **free-form strings** at write time... A background job (`schema_inducer`) runs during consolidation: Clusters predicates by embedding + usage pattern, proposes canonical predicate names, rewrites edges..."*
- Actual code `triple.rs` L12–L22: `Predicate` is a **closed Rust enum** with 9 variants (`IsA`, `PartOf`, `Uses`, `DependsOn`, `CausedBy`, `LeadsTo`, `Implements`, `Contradicts`, `RelatedTo`).
- `triple.rs` L27–L35: `from_str_lossy` maps a fixed set of string aliases → enum; anything unrecognized falls back to `RelatedTo`.
- `triple_extractor.rs`: the LLM prompt is constrained to these 9 predicates (few-shot examples are all from the fixed set).

**Problem**: The design claims emergent schema as a **key differentiator** ("this is the emergent schema differentiator — engram's schema is learned, not designed"). The code implements the exact opposite: a closed enum. There is no `schema_inducer` module, no alias index, no canonicalization job. The gap between claim and code is not incremental — it's categorical.

**Two-way implication**:
1. If we keep DESIGN's story → `Predicate` must become `String` (or `Enum { Known(KnownPred), Custom(String) }`), `triple_extractor` prompt must allow free-form output, and `schema_inducer` is a new subsystem (not trivial: clustering + LLM naming + rewrite pass).
2. If we keep the code's story → DESIGN §3.5 must be rewritten to "seeded closed-set with `RelatedTo` fallback and post-hoc promotion", which is honest but sacrifices the "emergent schema" marketing claim.

**My recommendation**: **Hybrid — seeded-with-override, promoted by consolidation**. Concretely:
- `Predicate` becomes `enum Predicate { Seeded(SeededPred), Proposed(String) }`.
- Extractor prompt: "prefer one of [9 seeded], but if none fit, emit `OTHER: <your_name>`".
- `schema_inducer` only runs during consolidation: clusters `Proposed(_)` across episodes, promotes stable ones to `Seeded` (requires schema migration).
- DESIGN §3.5 rewrites to reflect this: it is still "emergent at the edges" but has a warm start. More honest, smaller surface, still a differentiator vs Graphiti.

**Effort correction**: DESIGN implies this is already handled by `schema_inducer`. It isn't. Add **~1 week** to Phase 2 or Phase 4 for this module.

---

### A3. `MemoryRecord.confidence`: DESIGN makes it stored — but confidence is supposed to be meta-cognitive

**Evidence**:
- DESIGN §3.2 line 131: `pub confidence: f64, // was computed; now stored`
- Actual code `confidence.rs`:
  - `confidence_score(record, all_records)` is a **pure function** of `content_reliability(record)` + `retrieval_salience(record, all_records)` (L134–L141).
  - `retrieval_salience` depends on sibling records (it's a relative measure).
  - `confidence_label`, `confidence_detail`, `calibrate_confidence`, `confidence_to_signal` are all downstream consumers.

**Problem**: Freezing confidence into a stored field kills its metacognitive character. `retrieval_salience` is **relational** — it changes depending on what other memories are present in the current retrieval. A stored value is a snapshot; it goes stale the moment a related memory is added or decays. You lose the "how confident am I *right now*, given what I know *right now*" semantics.

The DESIGN comment `// was computed; now stored` suggests this is a **deliberate** choice, not an oversight. But it's not justified anywhere — no rationale in §3.2, §6, or §10 (open questions).

**What breaks if we store it**:
- Every `MemoryRecord` mutation must re-invoke `confidence_score` and persist → write amplification.
- Consolidation changes `core_strength` → stored confidence is stale until next rewrite.
- Cross-record salience becomes incoherent: record A's stored confidence was computed against set S₁; recalled alongside set S₂, its value no longer represents "confidence within S₂".

**Three alternatives**:
1. **Don't store**: keep confidence computed. No schema change, no staleness. Cost: compute on every recall. Already fast (pure function over small batches).
2. **Cache with TTL + dirty bit**: store `last_computed_confidence: Option<f64>` + `confidence_computed_at: DateTime`. Recall re-computes if stale beyond threshold or dirty. Keeps semantics, avoids hot-path cost. Adds 2 fields instead of 1.
3. **Store reliability, compute salience**: split — `content_reliability` is intrinsic to the record (stable), store it; `retrieval_salience` stays dynamic. Record gets one stored field, confidence is reconstructed at recall.

**My recommendation**: **Alternative 3**. It aligns with the actual split in `confidence.rs` and gives us the write-time stability DESIGN seems to want, without losing the metacognitive relational property. Requires updating §3.2 field list and Phase 1 tasks.

---

## 🟠 Class B — Scope / timeline credibility

### B1. Phase 1 ("Graph foundation, 3 weeks") blast radius is badly underestimated

**Evidence** (from gid code graph impact queries, recalled from memory 04-24 02:23):
- `MemoryRecord` (src/memory.rs) → affects **14 nodes**: 7 recall variants + `correct_bulk` + cli main + 2 tests + the struct itself + Display/Debug impls.
- `storage.rs` (SQLite schema + serialization) → affects **422 nodes** (most of the crate reads or writes through it).

**Phase 1 deliverables (DESIGN §9, line 579–587)**: Entity/Edge tables, `MemoryRecord` field extensions (4 new fields), migration scaffolding, basic CRUD.

**The arithmetic DESIGN ignores**:
- Each of the 4 new `MemoryRecord` fields (`episode_id`, `entity_ids`, `edge_ids`, `confidence`) touches those 14 nodes on the read side.
- Each requires schema migration in `storage.rs`, which has 422 downstream dependents.
- That's roughly `4 × (14 + 422) = 1,744` touchpoints to verify (mostly trivial recompile, but non-zero test runs + manual checks for ~30 of them).
- Plus new `Entity` and `Edge` tables, their serialization, their indexes, their CRUD.

**Calibration**: ISS-024 (dimensional read-path, a much smaller change — one field through the read pipeline) took multiple weeks with reviews r1/r2/r3. Phase 1 is 4× that scope + new tables + new migration harness, scheduled for 3 weeks.

**Realistic estimate**: **5–7 weeks** for Phase 1 if done to the same quality bar as ISS-024. Alternatively, **scope-cut Phase 1** to just `Entity`+`Edge` tables and defer MemoryRecord extension to Phase 1.5.

**Recommendation**: DESIGN §9 should either (a) bump Phase 1 to 6 weeks, or (b) carve out a "Phase 1a — graph tables only, no MemoryRecord changes" of 2 weeks and a "Phase 1b — MemoryRecord extension" of 3–4 weeks. Current numbers are aspirational, not planning-grade.

---

### B2. Non-goal NG5 ("Not retrofitting v0.2's MemoryRecord") directly contradicts §3.2

**Evidence**:
- §1 NG5 line 38: *"Not retrofitting v0.2's `MemoryRecord` — it stays, it plays a specific role (see §3)"*
- §3.2 line 106: section title "**MemoryRecord — kept, extended**", adds 4 fields.

"Kept, extended" **is** retrofitting. The NG5 phrasing misleads readers into thinking MemoryRecord is untouched.

**Recommendation**: rephrase NG5: *"Not replacing MemoryRecord; v0.3 extends it with provenance links and confidence, but the episodic-trace role is preserved."* — honest about the 4 new fields.

---

### B3. Phase 0 "Alignment (1 week)" is the wrong shape

DESIGN §9 line 572–577 says Phase 0 = "alignment". Currently this is the *review* phase (which is fine), but it does not include the decisions raised in A1/A2/A3 above. The alignment week should produce:

1. A **naming decision doc** (working memory: A/B/C).
2. A **predicate strategy doc** (closed / emergent / hybrid).
3. A **confidence storage decision** (stored / computed / cached / split).
4. A **scope carve-up** of Phase 1 (single or 1a+1b).

Without these, Phase 1 starts from ambiguity and re-litigates architecture mid-implementation. Add an explicit deliverable list to Phase 0.

---

## 🟡 Class C — Specification gaps (tighten before coding)

### C1. §3.3 `Entity.activation: f64` and `Entity.valence: f64` — no update rule specified

Entities have ACT-R activation and affect valence, but DESIGN does not say:
- When does `Entity.activation` decay? (per-access? per-tick? same curve as MemoryRecord?)
- How is `Entity.valence` learned from mentions? (mean of mentioning memories' affect? weighted by activation?)
- If an entity is mentioned in a negative-affect memory that later decays, does entity valence move back?

Without these rules, implementers will invent. Add a §3.3.x subsection.

### C2. §4.3 "multi-signal fusion" for entity resolution — weights not specified

DESIGN claims fusion of (embedding + alias match + graph-context + temporal proximity) but gives no initial weights, no threshold for "same entity", no tie-breaker rule. Phase 2 is scheduled for 3 weeks; without weights, the first week is spent guessing.

Propose initial values **in the doc** (they can be tuned later — §8.3 even says so): e.g., `w_embed=0.5, w_alias=0.3, w_context=0.15, w_temporal=0.05`, threshold `≥0.72 = same entity`.

### C3. §3.4 Edge bi-temporality — `valid_from` vs `recorded_at`

Graphiti's bi-temporal model has two time axes. DESIGN mentions bi-temporal in §2 but §3.4 is not shown in my excerpt (line 166–208). Verify §3.4 defines both:
- `valid_time`: when the fact is true in the world
- `transaction_time`: when we recorded it

If only one is present, the "is Melanie still married to Marcus" guarantee (G1) cannot be delivered.

### C4. §4.5 "Interoceptive gating" — what actually gets gated?

DESIGN says interoception can suppress writes/edges. Under what signals? Stress-high → skip extraction? Load-high → defer consolidation? Needs concrete rules, not "gating happens".

### C5. §5.1 query classification — one LLM call per recall is not free

"One cheap LLM call OR heuristic" — which? If LLM, every `recall()` now has LLM latency (200–500ms). If heuristic, show the heuristic. A `recall()` that sometimes takes 500ms breaks user-visible latency expectations.

**Recommendation**: heuristic first (regex/embedding distance to query prototypes), LLM only when heuristic confidence is low. Spell this out.

---

## 🟢 Class D — Smaller nits (fix when convenient)

### D1. §0 claim "None of [Graphiti/mem0/A-MEM/Letta/LightRAG] have the cognitive layer"

True in aggregate but each has *something* — mem0 has importance scoring, Letta has agent-managed forgetting. Weaken to "none combine decay + affect + consolidation + interoception at engram's depth" to avoid cherry-picking accusations.

### D2. G3 target "2–3 LLM calls per episode vs Graphiti's 5–10" is aspirational

No measurement plan. Add to §11 success criteria: "Measured by LLM-call counter in `write_stats.rs`, averaged over benchmark corpus of N=500 episodes."

### D3. §7 Public API uses `recall()` and `recall_with()` — naming already in use by `memory.rs`

Verify call-site compatibility. If the new signatures change, NG1 backward compat goal is at risk.

### D4. §8.1 migration — no rollback plan

v0.2 → v0.3 schema migration is one-way per the text. Given v0.2.2 is published on crates.io and users *will* want to roll back after trying v0.3, add either (a) a rollback script or (b) an explicit "no rollback, snapshot before upgrade" warning.

### D5. `RelatedTo` is a semantic landmine

It's the fallback predicate. In practice it will accumulate 60%+ of all edges (every LLM extraction that doesn't match exactly). Either:
- Accept it and tune recall to deprioritize `RelatedTo` edges; or
- Require the extractor to refuse rather than fall back — edges are optional, not every sentence needs one.

### D6. Missing observability hook

No mention of how a user inspects "why did you think X?". With 5 layers + graph + cognitive signals, debuggability is essential. Add to §7: `explain_recall(query) -> RecallTrace` or similar.

---

## ✅ What DESIGN gets right (do not change)

- **§0 thesis** is sharp and defensible.
- **§2 layer diagram** is the right abstraction — matches the code's actual shape once naming is fixed.
- **NG1–NG4, NG6** are correctly scoped.
- **§10 explicit open questions** is exactly the discipline missing from most design docs.
- **§11 success criteria** exists (though D2 needs tightening).
- **Migration plan §8 existing** (even if D4 incomplete) is better than most.

---

## 🧭 Recommended next steps

1. **Resolve A1/A2/A3 before any code**. Three short decision docs (~1 page each) in `.gid/decisions/`.
2. **Re-plan Phase 1** with real blast-radius numbers (B1).
3. **Tighten C1/C3/C5** — these are silent correctness risks.
4. **Delete/rephrase NG5** (B2). Honesty matters.
5. **Re-review after revisions** — I'll run r2 once v0.3r1 is updated.

---

## Review metadata

- **Checks applied**: all 36 (full depth)
- **Findings**: A1–A3 (critical), B1–B3 (important), C1–C5 (specification), D1–D6 (minor)
- **Total issues**: 17
- **Agent self-assessment**: confidence ~0.7 — code-graph claims are sampled not exhaustive; "422 nodes" blast radius number comes from recalled memory, not a fresh `gid_query_impact` on this exact commit. Re-run before treating B1 numbers as final.

---

# Addendum: Discussion Findings (2026-04-23)

Follow-up findings from live architectural discussion with potato after r1 was written. These go beyond the design doc itself and challenge foundational assumptions I accepted too readily in the initial review.

## A4 (critical) — Interoceptive gating of write path is philosophically wrong

**Location**: DESIGN-v0.3 §4.5 "Budget-aware skip"

**What the design says**: When stress is high or operational_load exceeds a threshold, skip Stages 3/4/5 (extract + resolve + edge-resolve). Mark `pending_extraction=true` and backlog to consolidation.

**Why this is wrong**:

1. **Misreads interoception's function**. Interoception is the agent's *self-awareness signal* — it exists so the agent can notice "I'm in a dead end, change strategy" or "load is spiking, tighten scope". It's a mirror for behavior modulation. It is NOT a throttle for memory formation. Conflating them reverses the causality.

2. **Contradicts neuroscience**. Under stress, human memory encoding *strengthens* (amygdala-modulated consolidation), not weakens. Mapping stress → "less extraction" is backwards — the moments that matter most would be the least-indexed.

3. **Creates a reverse-causality anti-pattern**. A hard debugging session (agent stress = high) would fail to produce entity/edge extraction precisely when the conversation is most valuable. Backlogged extraction runs later with stale graph context, producing worse quality than if it had run inline.

4. **Conflates two orthogonal axes**. Resource pressure (token budget, queue depth, latency SLOs) ≠ affective state. A gating decision should be driven by resources, not emotions.

**Correct design**: Remove §4.5 entirely. Interoception's role in v0.3 is **zero on the write path**. Its legitimate placements are:
  - System prompt injection (already implemented)
  - Metadata marker on MemoryRecord ("what was my state when this was encoded") — useful for post-hoc analysis, does not gate anything
  - SOUL.md suggestion trigger on long-term trends (already implemented)

**Status**: Critical. Revise DESIGN-v0.3 §4.5 to delete gating logic, replace with "Interoception does not gate memory formation."


## A5 (critical) — Budget/backlog mechanism itself is unnecessary complexity

**Location**: Implicit throughout DESIGN-v0.3 §4 write path

**What I initially proposed**: "If Stage 3/4/5 exceed budget, mark `pending_extraction=true` and backlog to consolidation cycle."

**Why this is wrong** (surfaced by potato's challenge "why would that even happen?"):

The write path should either complete synchronously or not enter Stage 3 at all. Adding a budget-check + skip + backlog layer introduces three new moving parts for a problem that doesn't actually exist in normal operation:

- Regular agent-user dialogue: extraction is async and cheap (~2-3 LLM calls per episode at ~1.5KB in / 0.2KB out). Never saturates budget.
- Prompt size blow-up (Stage 3 with huge graph neighborhood): solve by *capping graph context size in the prompt design*, not by runtime budget gating.
- Bulk import (10k historical records): the importer's responsibility to batch/rate-limit. Not the write path's problem.
- LLM rate-limit/latency: infra-layer retry/queue, not engram's concern.

**Correct design**:
- Default: all episodes run full 6 stages synchronously.
- Decision to enter Stage 3 is based on **content** (importance, novelty), not **runtime resources**.
- Bulk import uses a dedicated batch API with its own pacing.

**Status**: Critical. Violates "no technical debt" — a complexity layer (budget check + backlog + pending-extraction state machine) for a scenario that shouldn't occur in normal operation. Delete the concept from DESIGN.

---

## E-series — Ground Truth Corrections (v0.2 feature scope)

During discussion, several claims in the original review and my architectural explanations turned out to misrepresent what v0.2 already implements. Filing these as corrections to avoid propagating errors.

### E1 — v0.2 already has two-layer intent classification (not pure heuristic)

**Claim in discussion**: "v0.2 recall is pure heuristic, no LLM on the read path."

**Ground truth** (verified via `query_classifier.rs:595-748`):
- `classify_intent_regex()` — Level 1 heuristic (regex + keyword)
- `QueryIntentClassifier::classify()` — Level 2, **calls Anthropic Haiku**
- Entry point `classify_query_with_l2()` runs L1 first, falls back to L2 LLM when L1 confidence is low

**Implication**: v0.3 does not need to "add" LLM-based intent classification — it exists. The design question is whether the two-layer structure should be kept, simplified, or hardened.

**Recommendation**: Keep the two-layer structure but strengthen L1 with embedding-prototype matching (each intent has 3-5 prototype queries, cosine similarity selects). This drops L2 fallback rate below 5%, keeping the LLM as a true last-resort tie-breaker rather than a routine call.

### E2 — v0.2 already has 6 intent classes, not 5

**Claim in discussion**: "v0.3 needs 5 intent types: Factual/Abstract/Episodic/Affective/Hybrid."

**Ground truth** (verified via `QueryIntent` enum): v0.2 has `Definition | HowTo | Event | Relational | Context | General` — **6 classes**, each with `TypeAffinity` weights mapped to 7 MemoryTypes (procedural/factual/episodic/etc.).

**Implication**: v0.3 is not introducing intent classification; it should **wire existing intents to new layers**:
- `Definition | Relational` → L4 graph traversal (currently goes through embedding search)
- `Event` → L1 Episode time index (currently goes through MemoryRecord timestamp)
- `HowTo` → L5 Topic pages (already partially works)
- `Context | General` → hybrid retrieval + rerank

The change is smaller than the DESIGN implies. Revise DESIGN §5 to describe the routing, not a "new" classification.

### E3 — Affect system has two distinct layers; my "state-dependent recall" conflated them

**Claim in discussion**: "If agent stress is high, recall should prefer past memories formed under high-stress context (state-dependent retrieval)."

**Ground truth** (verified via `interoceptive/types.rs` and `dimensions.rs`):

Two separate systems exist, with non-overlapping purposes:

- **Interoceptive signals** (`SignalSource`: anomaly, accumulator, confidence, alignment, operational_load, execution_stress, cognitive_flow) — **agent's self-state**. Feeds behavior modulation, system prompt injection, SOUL.md drive tracking.
- **11-dim affect** (`Dimensions`: valence, domain, importance, confidence, etc.) — **property of a memory record**. Captures the affective character of *what is being remembered*, not the agent's state at the moment of remembering.

User emotional state is currently captured **implicitly** through the memory's `valence` + `domain` dimensions at encoding time. There is no separate "user affective state aggregator".

**Implication**: My proposal was wrong. Agent interoceptive state should NOT influence recall ranking — recall exists to serve the user's query, and agent mood is irrelevant to "what answer is correct". What IS legitimate:

- At recall time, extract the **query's own affect** (dimensions from the query text itself) — if the user asks an emotionally-charged question, match against memories with similar affect.
- User affect match is a legitimate ranking signal.
- Agent interoception is not.

**Correct signal separation**:
- `stress` = agent state → behavior modulation only
- `empathy` (as a concept) = what the agent perceives about the user's state → could become an explicit signal later, but currently lives inside memory dimensions
- Memory `valence/domain` = content property → ranking signal

**Status**: Update DESIGN §5 ranking formula to use *query-derived affect* as the affective match signal, NOT interoceptive state.

### E4 — Knowledge Compiler already uses Infomap clustering

**Claim in discussion (ambiguous)**: "Will v0.3 compile use Infomap-style clustering?"

**Ground truth** (verified via `compiler/discovery.rs:241`): `run_infomap_and_build_candidates` already exists. Infomap community detection is the current clustering algorithm.

**Implication for v0.3**: The change is not algorithmic, it's **input scope**:
- Currently: Infomap runs on memory-memory semantic similarity graph
- v0.3: should run on **joint memory + entity graph** (memory→entity mention edges + entity→entity co-occurrence edges)
- Result: topics carry entity structure, not just text clusters

This is a smaller change than "add clustering to compile" — the engine is there, just feed it a richer graph.

### E5 — Consolidation and compile cadence should stay manual/periodic, not real-time

**Discussion point**: When should consolidation and compile run in v0.3?

**Ground truth**:
- `lifecycle::run_lifecycle()` — manual invocation, triggered by heartbeat in RustClaw
- `knowledge_compile` tool — manual or accumulation-triggered
- No existing real-time pipeline

**Recommendation for v0.3**: Do NOT add real-time triggering. Both operations are batch-natural:
- Consolidation aggregates across records (r1→r2 ODE increments, supersession detection, conflict detection) — batch is strictly better
- Compile runs Infomap + LLM naming — expensive, belongs on low-frequency schedule

**v0.3 additions go inside existing consolidation cycle**:
- Predicate schema induction (cluster Proposed(_) edges, promote to Seeded)
- Entity dedup sweep (re-run ambiguous resolutions with full graph context)
- Topic re-compilation on joint memory+entity graph

No new scheduler, no new trigger mechanism. Existing heartbeat cadence absorbs it.

---

## Summary of Addendum Impact

The discussion surfaced **2 critical findings** (A4, A5) that invalidate DESIGN-v0.3 §4.5 entirely, plus **5 corrections** (E1-E5) that scope down the actual delta between v0.2 and v0.3. 

The true v0.3 change is smaller than the document implies. Most "new" capabilities (intent classification, LLM usage on read path, Infomap clustering, consolidation scheduling) already exist in v0.2. The real additions are:

1. L1 Episode buffer (genuinely new storage layer)
2. L4 Semantic graph (Entity + Edge + bi-temporal — genuinely new)
3. Routing existing intents to new layers (integration, not new logic)
4. Joint memory+entity clustering in compile (input change, not algorithm change)
5. Schema induction in consolidation (new consolidation phase)

Interoceptive gating of the write path (§4.5) should be **deleted**, not refined.

*Addendum written 2026-04-23 during live discussion. Confidence: high — all E-series claims verified against current source code before filing.*

---

## Verification Method — GID-Backed Addendum (2026-04-23 late)

The original r1 review and earlier Addendum findings mixed raw-code grep (truth but narrow) with memory-recalled impact numbers (wide but stale). This section re-verifies the blast-radius claims using GID code graph queries on the live engram workspace (`/Users/potato/clawd/projects/engram/.gid/graph.db`, extracted 2026-04-23).

### Corrected blast radius numbers

| Node | Claimed in r1 / recalled | Actual (GID) | Correction |
|---|---|---|---|
| `MemoryRecord` struct | "14 nodes" (B1) | **11 nodes** (2 self-methods + 9 `Memory::recall*` methods) | Close. Update B1 to 11. |
| `Storage` struct | "422 nodes" (B1, from old memory) | **709 nodes** | Significantly larger than recalled. Phase 1 "3 weeks" claim is even less credible than B1 stated. |
| `QueryIntent` enum | not previously claimed | **39 nodes** (6 production + 33 tests) | Production impact is 6; tests dominate count. When citing impact, distinguish prod vs tests. |
| `classify_query_with_l2` | not previously claimed | **8 nodes** (7 prod call sites + 1 test) | Small, tight surface — easy to extend. |

### Method notes

- `gid_query_impact` uses graph traversal (`calls`, `defined_in`, `belongs_to` edges) so the count includes both production callers and test modules. For accurate "production blast radius" subtract test nodes manually — they inflate the number 3-5× for widely-tested types.
- Node ID format: `{kind}:{file}:{name}` where kind ∈ {func, method, class, struct, enum, module}. Use `gid_schema` for overview or check the graph for real IDs before querying.
- A `gid_extract` run was needed first because the engram monorepo's graph (as of 2026-04-23) had task nodes only, not code nodes. After extract: 2708 code nodes + 11872 edges merged in.

### Implications for r1 findings

- **B1 (scope underestimation) strengthened**: Storage touches 709 nodes, not 422. Any schema change (v0.3 new fields, new tables, new indexes) hits ~4× the surface than originally claimed. 3-week Phase 1 is not credible; realistic minimum is 6-8 weeks just for storage migration + test stabilization.
- **E1 (two-layer intent classifier) validated**: `classify_query_with_l2` has only 8 impacted nodes (7 prod). Low-risk to strengthen L1 without breaking anything — the heuristic upgrade I proposed is safe.
- **A1/A2 (working memory collision, predicate schema)**: Did not re-query — these are conceptual conflicts inside the design doc, not blast-radius claims. Code-level impact kicks in only when implementation starts.

### Lesson for next review

- For any finding that references impact counts or touch surface → run `gid_query_impact` before citing a number.
- Raw grep is fine for "does this code do X" claims. GID is required for "how many places would change if I modify X" claims.
- Numbers recalled from memory are stale — always re-verify on the current graph.

