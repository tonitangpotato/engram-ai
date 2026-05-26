---
title: "GraphEntityResolver wired but classifier never routes to Factual plan (0/152 on conv-26)"
status: open
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
discovered_in: ISS-161 root-cause audit 2026-05-26
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
