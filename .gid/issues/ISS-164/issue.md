---
title: GraphEntityResolver wired but classifier never routes to Factual plan (0/152 on conv-26)
status: falsified
priority: P1
severity: architecture-gap
category: retrieval
created: 2026-05-26
relates:
- ISS-148
- ISS-149
- ISS-161
- ISS-162
- ISS-163
- ISS-165
- ISS-166
discovered_in: ISS-161 root-cause audit 2026-05-26
falsified_by: ISS-165
blocked_by: ''
---

## Summary

`GraphEntityResolver` (entity alias → graph traversal) and the
Factual retrieval plan that consumes it both exist in code with full
unit-test coverage. But on conv-26, **zero of 152 questions route to
the Factual plan**. The classifier emits `Intent::Factual +
DowngradeHint::Associative` for natural-language entity references
and the dispatcher silently runs the Associative plan instead,
which never consults the entity resolver.

This means engramai's advertised "entity-aware retrieval" is
**inactive at runtime** on the dominant LoCoMo question shape.

## Code-layer evidence

Verified 2026-05-26 against working tree (commit `5adf83e`):

1. **Resolver exists** —
   `crates/engramai/src/retrieval/adapters/graph_entity_resolver.rs:65-105`:
   ```rust
   pub struct GraphEntityResolver<'a> {
       pub graph: &'a dyn GraphRead,
   }
   impl<'a> EntityResolver for GraphEntityResolver<'a> { ... }
   ```
   Has alias-match search via `search_candidates(CandidateQuery {...})`,
   recency scoring, per-namespace iteration. Unit tests pass.

2. **Factual plan exists** —
   `crates/engramai/src/retrieval/plans/factual.rs:398` accepts
   `resolver: &dyn EntityResolver`, runs entity-resolution stage,
   then traverses edges from the resolved anchors.

3. **Dispatcher silently downgrades** —
   `crates/engramai/src/retrieval/dispatch.rs:92-93`:
   ```rust
   (Intent::Factual, DowngradeHint::Associative) => PlanKind::Associative,
   (Intent::Factual, DowngradeHint::None) => PlanKind::Factual,
   ```
   `Intent::Factual + hint=Associative` dispatches to the
   **Associative** plan — which does not consume `EntityResolver`.

4. **Classifier emits the downgrade hint by default** —
   `crates/engramai/src/retrieval/classifier/mod.rs:245-248`:
   ```rust
   // No strong signal → Factual with Associative downgrade hint.
   ClassifyResult {
       intent: Intent::Factual,
       downgrade_hint: DowngradeHint::Associative,
       ...
   }
   ```
   Strong-signal threshold `τ_high = 0.7`. Entity signal is the
   alias-match score; "Caroline" matches but "What did Caroline
   research" as a query usually scores below 0.7 because the
   entity is one token among many, not the dominant signal.

5. **Empirical confirmation** — from conv-26 V1 control sweep (Arm F,
   `/tmp/iss161-l3/iss161-l3-F.log`, 152 queries):
   ```
   $ grep "execute_plan ENTER" iss161-l3-F.log | awk -F'plan_kind=' '{print $2}' | awk '{print $1}' | sort | uniq -c
       18 abstract
        7 affective
      120 associative
        2 episodic
        5 hybrid
        0 factual
   ```

   `factual` count = **0**. `GraphEntityResolver` was never invoked
   during any AC-5a measurement.

## Why this hides

The downgrade-to-Associative behaviour is documented in
`classifier/mod.rs:87-88` and was an intentional design choice
("plan_used = Factual per §3.1 'exactly 5 intents' but actually run
Associative for natural-language queries with weak entity signal").
The motivation was reasonable — Associative is the safer default
when entity match is uncertain. But the unintended side-effect is
that **the entity resolver and factual plan only fire on queries
that already look like database lookups** ("show me memories about
Caroline"), not on natural conversation ("what did Caroline
research?").

The classifier's τ_high = 0.7 was tuned against synthetic test
queries with explicit entity markers, not against LoCoMo-style
natural-language gold questions.

## Comparable systems

**Mem0g** (graph variant) routes every query through entity
resolution. Entity match strength becomes a feature in the final
ranker, not a gate that decides whether to use the graph at all.

**Zep** uses LLM-based query understanding to identify entities
in the query, then runs entity-anchored traversal in parallel with
semantic search — both contribute candidates that are fused later.

**LangMem** uses keyword extraction (cheap, deterministic) to
identify candidate entity mentions and always runs entity-anchored
retrieval as one channel of multi-channel fusion.

The pattern is consistent: entity resolution runs **in parallel**
with semantic retrieval, never as an opt-in gate.

## Concrete failure example (conv-26 q3 + q40)

q3: "What did Caroline research?" (gold = "Adoption agencies")
- Classifier: "Caroline" is one token, query is question-shape.
  Entity signal ≈ 0.4 (below τ_high 0.7). Intent=Factual +
  hint=Associative. Dispatcher runs Associative.
- Associative plan does pure embedding + FTS fusion, no entity
  anchoring. Top-K candidates include many "Caroline did X" turns
  that aren't about research.

q40: "How many siblings does Caroline have?" (gold = "2")
- Same classifier outcome. Same downgrade. Top-K is whatever
  embedding finds closest to the literal question; "2" appears in
  hundreds of episodes in numerical contexts unrelated to siblings.
- An entity-anchored traversal would find `(Caroline)
  -[has_sibling]->(?)` edges if any were extracted, which would
  short-circuit the search to the correct memory directly.

## Acceptance criteria

**AC-1 (always-on entity channel)**: Add entity resolution as a
parallel channel in the Associative plan, not a gate. Even when
`Intent::Factual + hint=Associative`, run `EntityResolver::resolve`
on the query and contribute the resolved anchors' memories to the
candidate pool alongside semantic + lexical channels.

**AC-2 (no regression on non-entity queries)**: When
`EntityResolver::resolve` returns zero anchors (no entity in
query), the new channel contributes zero candidates and the
existing Associative behaviour is byte-for-byte preserved.

**AC-3 (config knob)**: Behaviour opt-in via
`FusionConfig.entity_channel_enabled: bool` (default `false`
until benched). When false, current routing preserved exactly.

**AC-4 (LoCoMo measurement)**: Re-run conv-26 K=10 temp=0 HyDE=off
with `entity_channel_enabled = true` (and ISS-162 + ISS-163
shipped). Target: single-fact sub-bucket ≥ 16/27 = 0.59 —
within 1 question of AC-5a 0.60.

**AC-5 (cost ceiling)**: Entity resolution per query must add
≤ 10ms p50 / ≤ 50ms p95 latency. Resolver is alias-match against
SQLite; this should be easy.

**AC-6 (no regression)**: Cross-validate on conv-44. Single-fact /
list / multi-hop / open-domain all within ±2pp on overall.

## Out of scope

- **Lowering τ_high threshold** — easier to ship but doesn't
  address the root issue (classifier's binary gate over a
  continuous signal). The correct fix is to use entity signal as
  a feature in fusion, not a routing gate.
- **Rewriting the classifier** — separate, much larger work. This
  ISS only adds a parallel channel; the existing classifier
  behaviour is untouched.
- **Entity extraction quality at ingest time** — assumes
  entities are extracted reasonably well. ISS-145 tracks the
  upstream extraction quality issue.

## Estimated effort

1 week. The resolver is already callable. The fusion module already
accepts multiple channels. New work: wire one more channel into
Associative plan with a Boolean gate + the LoCoMo bench + tuning
the channel weight.

## Expected lift

Per ISS-161 audit, ~3 of the 9 missing single-fact questions (q40,
q75, q76 — multi-episode composition questions, plus likely q11
once entities are extracted) need entity-anchored retrieval to
find the right memory deterministically. ISS-164 alone projects to
~15-16/27 = 0.56-0.59 single-fact. Combined with ISS-162 (context
window) and ISS-163 (semantic UPDATE), the AC-5a 0.60 target
becomes reachable for the first time.

## Combined ISS-162 + ISS-163 + ISS-164 path to AC-5a

| State | single-fact (n=27) | gap to AC-5a 0.60 |
|---|---|---|
| Current best (Arm B, L2) | 8/27 = 0.296 | -9 questions |
| + ISS-162 (context window) | ~11/27 = 0.41 | -6 questions |
| + ISS-163 (semantic UPDATE) | ~13/27 = 0.48 | -4 questions |
| + ISS-164 (entity channel) | ~16/27 = 0.59 | -1 question |
| + ISS-159 cross-encoder (already shipped) | ~17/27 = 0.63 | **+0** (PASS) |

Projection. Real measurements pending implementation of each lever.

---

## 2026-05-26 — Phase 1 (code layer) shipped

### Changes
- `crates/engramai/src/retrieval/fusion/combiner.rs` — added `FusionConfig.entity_channel_enabled: bool` field (`#[serde(default = "default_entity_channel_enabled")]` → `false`). Wired into `FusionConfig::locked()`. Default-helper test pinned.
- `crates/engramai/src/retrieval/api.rs` — added `GraphQuery.entity_channel_override: Option<bool>` field + `with_entity_channel(Option<bool>)` builder. Mirrors the `mmr_lambda_override` / `cross_encoder_override` pattern verbatim. `None` defaults via `FusionConfig::locked().entity_channel_enabled`.
- `crates/engramai/src/retrieval/plans/associative.rs` — added `AssociativePlanInputs.entity_channel_enabled: bool` and `AssociativePlanInputs.entity_resolver: Option<&'a dyn EntityResolver>`. Imported `EntityResolver` from `plans::factual`.
- `AssociativePlan::execute` — Step 2b (inside `Stage::EntityExtract` budget block) injects `EntityResolver::resolve(query.text)` anchors into `seed_entities` with `seed_score = match_strength as f64`. Tracks them separately in `injected_anchors`.
- `AssociativePlan::execute` — Step 3b' (inside `Stage::MemoryLookup` budget block, right after Step 3b) calls `memories_mentioning_entity` directly on each injected anchor, surfacing the anchor's mentioned memories at `edge_distance = 1` (mirrors Factual's path — the high-value channel for proper-noun queries where the gold-fact mentions the anchor directly).
- `crates/engramai/src/retrieval/orchestrator.rs` — both `PlanKind::Associative` (line ~1162) and `run_associative_fallback` (line ~1500) read `query.entity_channel_override.unwrap_or_else(|| FusionConfig::locked().entity_channel_enabled)` and pass `Some(collaborators.entity_resolver)`. Fallback mirrors primary path so downgrade→Associative behaves identically.

### Byte-identity contract
Default off → §4.3 pipeline executes byte-for-byte identically to pre-ISS-164. When `entity_channel_enabled = true` but resolver returns zero anchors, byte-identity is also preserved. Both pinned by tests.

### Tests added (5)
- `plans::associative::tests::iss164_channel_off_preserves_byte_identity` — channel off, resolver wired with anchors, candidate set must match the no-resolver baseline.
- `plans::associative::tests::iss164_channel_on_zero_anchors_preserves_byte_identity` — channel on, resolver returns empty Vec, must equal channel-off result.
- `plans::associative::tests::iss164_channel_on_anchors_expand_seed_entities` — channel on with one anchor pointing at an isolated memory; baseline off-path does NOT surface it; on-path does, at `edge_distance = 1`.
- `fusion::combiner::tests::locked_entity_channel_enabled_defaults_to_false` — locked config default pinned.
- `fusion::combiner::tests::default_entity_channel_enabled_helper_returns_false` — serde default helper pinned.
- `tests/v03_retrieval_acceptance_test.rs::graph_query_new_defaults_entity_channel_override_to_none` — GraphQuery constructor default.
- `tests/v03_retrieval_acceptance_test.rs::graph_query_with_entity_channel_stores_the_override` — builder semantics + composition with other `with_*` setters.

### Suite results
- `cargo test -p engramai --lib` → **1956 passed, 0 failed, 4 ignored** (was 1954 pre-ISS-164).
- `cargo test -p engramai --test v03_retrieval_acceptance_test` → **11 passed, 0 failed** (was 9 pre-ISS-164).
- No warnings introduced.

### Phase 2 (bench validation) — pending
- Wire `ENGRAM_BENCH_ENTITY_CHANNEL=on|off` env var in engram-bench → `GraphQuery::with_entity_channel(Some(bool))`.
- A/B sweep on conv-26, K=10, temp=0, HyDE=off (isolation envelope). Two arms only — no λ sweep.
- AC-5a projection: single-fact 8/27 → 11/27 (+3). Falsification rule: measured lift <50% of projection (i.e. <+1.5 single-fact) → STOP, re-plan before ISS-162.
- Decision dependence: pass → keep ISS-164 enabled and move to ISS-162. Fail → file root-cause issue and re-scope ISS-148 unblock sequence.

---

## Phase 2 — falsified 2026-05-26

A/B sweep on conv-26 (K=10, temp=0, HyDE=off, MMR=off):

| Metric              | A (off)  | B (on)   | Δ           |
|---------------------|----------|----------|-------------|
| overall             | 0.3947   | 0.3618   | **−3.29pp** |
| **single-fact**     | **3/12** | **3/12** | **0**       |
| list                | 3/20     | 5/20     | +2          |
| multi-hop           | 17/37    | 13/37    | −10.81pp    |
| temporal            | 33/70    | 30/70    | −4.29pp     |

Zero lift on the target metric (single-fact), with a multi-hop
regression. The hypothesis that surfacing entity-resolver anchors
into the Associative pipeline would close the AC-5a gap is
**falsified for conv-26 single-fact**.

Phase 1 code (commits 77ef3f3 + ebc9adf in engram, 908a83d in
engram-bench) is **NOT reverted** — `FusionConfig::locked()`
default `false` keeps the wiring inert in production, and the
existing instrumentation is needed for the root-cause probes
filed as **ISS-165**.

Status flipped to `falsified`; see ISS-165 for next steps.

---

## 2026-05-27 UPDATE — falsification verdict in question; blocked on ISS-166

Investigating ISS-165 AC-1 surfaced a P0 confounder: **engram-bench's
`fresh_in_memory_db` never calls `Memory::with_pipeline_pool(...)`**, so
the v0.3 graph subsystem (ResolutionPipeline → `apply_graph_delta`) is
the only production writer of `graph_entities`, and it never runs under
LoCoMo benchmarks.

Direct sqlite verification on the fresh in-memory DB after a full
419-episode conv-26 ingest (956s real Haiku extractor):
- `entities` = 3 rows
- `nodes` = 456 rows
- `graph_entities` = **0 rows**

`GraphEntityResolver::resolve()` reads `graph_entities` via
`GraphRead::list_namespaces` → returns `Vec::new()` for **every** query.

**Implication for this issue**: Phase 2 Arm A (`entity_channel=off`) and
Arm B (`entity_channel=on`) were both running on an empty
`graph_entities` table. Arm B's entity channel injected 0 anchors per
query. The A/B was effectively "channel-off vs channel-off + a tiny bit
of extra dead code", not the intended isolation. The observed Phase 2
deltas (overall −3.29pp, multi-hop −10.81pp, list +2/20) cannot be
attributed to the entity channel design itself; they are noise from
LLM-judge wobble and/or fusion path side-effects.

Falsification verdict on conv-26 single-fact is therefore **not safe**.
Status flipped from `falsified` → `in_question`. Blocked on **ISS-166**
(harness wiring fix). After ISS-166 lands, re-run the same A/B sweep on
the fixed substrate per Phase 2 protocol and re-evaluate.

Phase 1 code (commits 77ef3f3 + ebc9adf engram, 908a83d engram-bench) is
not reverted — `FusionConfig::locked().entity_channel_enabled = false`
remains the default and keeps it inert; ISS-166 + future re-test need
the instrumentation in place.

Artifacts:
- `/Users/potato/clawd/projects/engram/.gid/issues/ISS-166/issue.md`
- ISS-165 AC-1 probe logs (`/tmp/iss165-ac1-probe-v{3,4-unified}.log`)

---

## 2026-05-27 — status remains `in_question`; root cause moved to ISS-165

ISS-166 (harness wiring) and ISS-167 (parser tolerance) shipped
and validated end-to-end. The conv-26 substrate now contains 666
`graph_entities` after a full ingest, with all gold-fact
entities present (Sweden, sunsets, abstract painting, Becoming
Nicole, adoption agencies, …).

But the ISS-165 AC-1 re-validation probe on the fixed substrate
still returned **NO_ANCHORS 9/9** across the same 9 failing
single-fact queries. The root cause is now confirmed (see ISS-165
2026-05-27 update): `GraphEntityResolver` lacks a mention
extraction step — it passes the entire question string as a
single `mention_text` to `search_candidates`, which does exact
alias matching, which never matches a long natural-language
question.

### Implication for ISS-164 Phase 2 re-run

**Do NOT re-run the Phase 2 A/B sweep on the current
substrate.** Re-running would once again measure
"entity-channel-off vs entity-channel-on-but-receiving-0-anchors"
— the same A/B/A confound that produced the falsified Phase 2
delta, with a different surface explanation.

The Phase 2 sweep can be meaningfully re-evaluated only after
ISS-165 ships a real mention extraction step (token n-gram scan,
FTS5 alias index, or LLM NER). Until then, ISS-164 stays
`in_question`.

### Status path

- `in_question` (2026-05-26, after Phase 2 falsification)
- `in_question` + `blocked_by: ISS-166` (2026-05-26, harness
  confounder discovered)
- `in_question` + `blocked_by: ISS-165` (2026-05-27, current —
  harness fixed but resolver root cause now known and pending fix)

Phase 1 code (commits 77ef3f3 + ebc9adf engram, 908a83d
engram-bench) **remains unreverted** — `FusionConfig::locked()`
default `false` keeps it inert; we need the instrumentation in
place for the post-ISS-165-fix re-run.

## 2026-05-27 — Phase 2 RE-RUN VERDICT (post-ISS-165 fix, falsified)

### Run

- **Sweep STAMP**: `20260527T051146Z`
- **Substrate**: `ENGRAM_BENCH_PIPELINE_POOL=1` (engram-bench:bfb1115)
  → 666 graph_entities populated per arm
- **Resolver**: token n-gram mention extractor (engram:a5b0407) →
  postfix probe confirms 9/9 single-fact queries anchor on real
  entities (Caroline, Melanie, art, book, etc., 18 total anchors)
- **Envelope**: conv-26, K=10, temp=0, HyDE=off, MMR=off,
  cross-encoder=off — single-axis A/B on `ENGRAM_BENCH_ENTITY_CHANNEL`
- **Bench commit**: engram-bench:f28b41d
- **Engram commit**: engram:a5b0407

### Results

Aggregate (n=152):

| metric       | A (off)  | B (on)   | Δ       |
|--------------|----------|----------|---------|
| overall      | 0.3289   | 0.3289   | +0.00pp |
| single-hop   | 0.2188   | 0.2188   | +0.00pp |
| multi-hop    | 0.3243   | 0.3514   | +2.70pp |
| open-domain  | 0.3077   | 0.2308   | −7.69pp |
| temporal     | 0.3857   | 0.3857   | +0.00pp |

Single-fact n=9 sub-bucket (ISS-161 set: q3 q7 q11 q37 q40 q43 q71 q75 q76):

| arm | single-fact |
|-----|-------------|
| A (channel=off) | 0/9 |
| B (channel=on)  | 0/9 |
| Δ               | +0  |

Score flips overall (n=152): 5 A-only correct, 5 B-only correct.
Symmetric noise — no signal.

### Decision (per script header decision rule)

`B − A single-fact ∈ {0, +1}` → **STOP, file root-cause**.
ISS-164 entity_channel direction is **falsified** in the
single-fact bucket where it was supposed to help.

### What this means

The resolver fix (ISS-165) and the harness fix (ISS-166) are both
real and correct — anchors are populated, anchors are found. But
**finding the right anchors does not by itself convert
single-fact questions** in the current Factual / Associative plan
pipeline. The bottleneck is somewhere downstream of anchor
resolution:

- Possibly Factual plan's anchor-to-fact traversal
  (`memories_mentioning_entity` ranking, edge weighting)
- Possibly generation: the retrieved candidates may contain the
  right episode but the LLM doesn't extract the right answer
- Possibly category misrouting: single-fact questions are still
  going through Associative plan (every single execute_plan log
  line says `plan_kind=associative`), not Factual, so anchors
  feed an associative aggregation that washes out the single fact

The aggregate also shows a non-zero substrate-level effect of
turning the pipeline pool ON: vs the broken-pool baseline
20260526T213218Z, overall dropped from 0.395 → 0.329. Triple
extraction adds nodes/edges that participate in graph-traversal
queries; this is a separate confounder worth tracking but
orthogonal to entity_channel.

### Status path (continued)

- `in_question` + `blocked_by: ISS-165` (2026-05-27 morning)
- **`falsified`** (2026-05-27 02:13 — this entry, Phase 2 re-run
  delta = 0 on single-fact)

### Phase 1 code disposition

Phase 1 instrumentation stays in tree because (a) `locked()`
default is `false` (inert), (b) it's still useful as a
post-classifier-routing experiment after Factual plan is fixed.
**Do NOT flip the default to true.** Do NOT revert 77ef3f3 +
ebc9adf yet — pending root-cause investigation of why anchors
don't help single-fact.

### Next investigations

- **All single-fact queries route to `associative`, not
  `factual`** (per execute_plan log). File an issue: the
  classifier is mis-routing single-fact questions. This is
  probably the actual ISS-148 AC-5a bottleneck — entity_channel
  was a wrong-layer fix.
- Audit Factual plan's anchor consumption — if/when classifier is
  fixed, does Factual plan with entity_channel=on lift?
- Consider whether `memories_mentioning_entity` edges, now
  populated, are being ranked correctly (BM25 vs embedding vs
  pure-anchor)
