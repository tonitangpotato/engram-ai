# ISS-070 Design V1 — MultiHop Retrieval Plan (SUPERSEDED)

> ⚠️ **Status: SUPERSEDED 2026-04-29.**
>
> This design proposed a beam-search-over-typed-edges approach to
> multi-hop retrieval. After review, potato + rustclaw concluded the
> beam-search paradigm itself is wrong for engram's brain-inspired
> retrieval goal. This V1 doc is preserved as a historical artifact
> documenting what was rejected and why.
>
> **Replacement:** see
> `.gid/features/v03-retrieval/discussion-spreading-activation.md` for
> the full rationale. A V2 design (spreading activation engine) will be
> written after a standalone prototype validates the approach on
> LoCoMo conv-26 multi-hop questions.
>
> Original V1 content preserved below for reference.
>
> ---
>
> Status (original): **proposal** — written 2026-04-29 by rustclaw, before
> implementation. Not normative until potato approves. This is a
> per-issue working design (not a feature design); if it grows past
> ~400 lines or touches new GOALs/GUARDs, lift into
> `.gid/features/v03-retrieval/design.md` as a numbered section.

## 1. Problem

RUN-0006 LoCoMo conv-26: `multi-hop` category hit@5 = **0/3 (0%)**.
The classifier currently routes multi-hop questions to either
Factual (1-hop edge fetch around resolved anchors) or Hybrid
(which itself uses single-shot sub-plans). Neither plan walks
edges past depth 1, so any answer that requires composing two
or more `(subject, predicate, object)` hops is structurally
unreachable.

The bench substrate has the right edges — the same memories that
contain the multi-hop chain are in the index, just spread across
two or three triples nobody combines.

## 2. Non-goals

- General graph database. Cypher, GraphQL, anything declarative.
- Neighborhood pre-materialization, edge cache, embedding-of-paths.
  All run-time, on the existing `graph_edges` table.
- Touching Affective / Bitemporal / Episodic plan dispatch logic.
- LLM-in-the-loop traversal (Letta-style "agent picks the next
  hop"). Out of scope; pure deterministic search.
- Cross-namespace traversal. Stay within the query's namespace.

## 3. Algorithm — beam search over the typed edge graph

### 3.1 Inputs

```rust
pub struct MultiHopPlanInputs<'a> {
    pub query: &'a ResolvedQuery,         // already entity-resolved
    pub anchors: Vec<ResolvedAnchor>,     // from EntityResolver, same as Factual
    pub budget: &'a mut BudgetController,
    pub query_time: DateTime<Utc>,        // for as-of projection (§4.6)
    pub config: MultiHopConfig,
}

pub struct MultiHopConfig {
    pub beam_width: usize,        // default 8
    pub max_depth: usize,         // default 3
    pub decay: f32,               // default 0.7 — multiplicative score penalty per hop
    pub allowed_predicates: PredicateFilter,
    pub min_anchor_strength: f32, // default 0.5 — skip Fuzzy-only seeds
}

pub enum PredicateFilter {
    /// Walk every canonical + proposed predicate.
    All,
    /// Only the explicitly listed canonical predicates.
    Canonical(Vec<CanonicalPredicate>),
    /// Default: a curated multi-hop-relevant subset (see §3.4).
    Default,
}
```

Note on naming: I propose `allowed_predicates` rather than the
issue's `allowed_edges: Vec<EdgeKind>` because v0.3 has no
`EdgeKind` enum — edges are typed by `Predicate`. Sticking to the
existing taxonomy avoids introducing a parallel concept.

### 3.2 State

```rust
struct PathState {
    head_entity: Uuid,           // current frontier vertex
    path: Vec<Edge>,             // edges traversed in order
    score: f32,                  // accumulated score, [0, 1]
    visited: BTreeSet<Uuid>,     // entity ids visited on this path
}
```

`visited` is per-path, not global. Two beam paths can revisit the
same vertex via different routes; only cycles within a single path
are forbidden (a vertex appears at most once in `path`).

### 3.3 Loop

```
frontier = anchors.map(anchor → PathState{
    head_entity: anchor.entity_id,
    path: [],
    score: anchor.match_strength,
    visited: {anchor.entity_id},
})

for depth in 0..config.max_depth:
    if frontier.is_empty(): break
    if budget.should_cutoff(Stage::MultiHopExpansion): break

    next_frontier = []
    for state in frontier:
        edges = graph.edges_of(state.head_entity, query_time)
                     .filter(allowed_predicate)
                     .filter(|e| !state.visited.contains(other_end(e)))
        for edge in edges:
            other = other_end(edge, state.head_entity)
            if other.is_literal(): continue   // can't keep walking
            extended = PathState {
                head_entity: other,
                path: state.path + [edge],
                score: state.score
                       * config.decay
                       * edge_score(edge),
                visited: state.visited ∪ {other},
            }
            next_frontier.push(extended)
    frontier = top_k(next_frontier, by=score, k=config.beam_width)

paths = all states from frontier across all depths (depth ≥ 1)
return MultiHopResult { paths, anchors }
```

`edge_score(edge)` is `edge.confidence * recency_factor(edge)`
where `recency_factor` reuses the same exponential decay used in
Factual scoring (design §5.4). Activation is **not** part of the
score — that's a fusion-stage input, this plan is unscored at the
candidate level (consistent with Factual's "unscored output, fusion
ranks" contract).

### 3.4 Default `allowed_predicates`

The `Default` filter excludes predicates that are uninformative
for multi-hop chaining:

- **Excluded:** `RelatedTo` (too generic — explodes the frontier),
  `MentionedIn` (provenance, not semantics), `Contradicts`
  (dialectic, would route the chain through negations).
- **Included:** every other `CanonicalPredicate` + every
  `Proposed` predicate.

Rationale: `RelatedTo` is the canonical "I don't know what this
is" fallback. Walking through it usually means the chain has gone
off-topic. `MentionedIn` connects entities to source episodes —
useful for provenance, but not for "Alice → friend → Bob → city
→ Tokyo"-style chains. `Contradicts` flips polarity — chaining
through it gives wrong answers more often than right ones.

This default can be overridden per-call. Live agents that do want
to walk dialectical edges (debate analysis, contradiction
surfacing) set `PredicateFilter::All`.

### 3.5 Output → memory candidates

The plan returns paths, but the orchestrator ultimately needs
**memory ids**. Conversion:

```
candidate_memories = {}
for path in result.paths:
    for edge in path.edges:
        if let Some(mem_id) = edge.memory_id:
            candidate_memories.add(mem_id, score=path.score)
    // Also pull memories that mention the *terminal* entity —
    // these are the answer-bearing memories for "what is X" style
    // multi-hop questions.
    candidate_memories.add_all(
        graph.memories_mentioning_entity(path.head_entity),
        score=path.score * 0.8  // small penalty: terminal vertex,
                                // not the edge that proves the chain
    )
```

The 0.8 penalty for terminal-vertex memories is a guess; tune
against RUN-0007.

## 4. Classifier routing

### 4.1 New PlanKind

`crates/engramai/src/retrieval/dispatch.rs` — add to the
`PlanKind` enum:

```rust
pub enum PlanKind {
    Factual,
    Episodic,
    Affective,
    Abstract,
    Bitemporal,
    Associative,
    Hybrid,
    MultiHop,            // new
}
```

### 4.2 Multi-hop intent detection

Two signal sources, ORed:

**Bench metadata path (deterministic, used for LoCoMo):**
```
if query.bench_metadata.locomo_category == "multi-hop":
    return PlanKind::MultiHop
```

This requires the LoCoMo driver to attach the category to
`GraphQuery` via existing metadata. Confirm via RUN-0007 that the
category survives end-to-end.

**Heuristic path (live agents):**
```
let multi_hop_intent =
    anchors.len() >= 2
    || query_text matches one of:
       - r"the (one|person|place) (who|that|which) "
       - r"(before|after) (that|then|him|her|it) "
       - r"introduced .* to "
       - r"\b(via|through|because of)\b .* and "
    ;
```

These are bootstrapping heuristics, not a literature-grade
classifier. Two-anchor signal is the most reliable: if the user
mentions two distinct entities, they probably want them connected.
Pattern matches catch single-anchor cases ("the person Alice
mentioned at lunch").

If neither fires, **fall through to current routing** — never
prefer MultiHop on weak signal. The cost of running it on a
single-shot question is wasted budget; the cost of skipping it
on a true multi-hop is what RUN-0006 already shows (0% hit). Bias
toward false negatives on the heuristic.

## 5. File-by-file change plan

| File | Change | Approx LOC |
|---|---|---|
| `crates/engramai/src/retrieval/dispatch.rs` | Add `PlanKind::MultiHop` variant + classifier branch | +40 |
| `crates/engramai/src/retrieval/plans/multi_hop.rs` | **New file** — types, beam search, predicate filter, candidate conversion | +600 (incl docs) |
| `crates/engramai/src/retrieval/plans/mod.rs` | `pub mod multi_hop;` | +1 |
| `crates/engramai/src/retrieval/orchestrator.rs` | New dispatch arm `PlanKind::MultiHop => execute_multi_hop(...)`; telemetry outcome variants | +80 |
| `crates/engramai/src/retrieval/api.rs` | Optional `PlanKind::MultiHop` exposure on `GraphQuery::with_plan_override` (for tests) | +10 |
| `crates/engram-bench/src/drivers/locomo.rs` | Attach `locomo_category` to query metadata so the classifier can read it | +20 |
| `crates/engram-bench/src/scorers/locomo.rs` | Per-plan breakdown: include `MultiHop` row in `RUN-NNNN.md` reports | +30 |
| Tests: `crates/engramai/tests/multi_hop_*.rs` | Unit (algorithm) + integration (3 LoCoMo cat=multi-hop questions as fixtures) | +400 |

Total ≈ **1180 LOC** across 8 files. Largest single file is
`plans/multi_hop.rs` at ~600 LOC including doc comments
(consistent with sibling plans: factual=1144, abstract_l5=976).

Per AGENTS.md ISS-010 Rule 1 + Rule 3: this is **incremental
write only**. Skeleton → types → algorithm core → predicate
filter → candidate conversion → tests, each step a separate
write that compiles.

## 6. Test plan

### 6.1 Unit (no graph store needed, use in-memory mock)

- `beam_search_terminates_at_max_depth`
- `beam_search_respects_beam_width`
- `beam_search_avoids_cycles_within_path`
- `beam_search_two_paths_can_share_intermediate_vertex`
- `predicate_filter_default_excludes_related_to`
- `predicate_filter_canonical_subset_walks_only_listed`
- `score_decays_multiplicatively_per_hop`
- `terminal_literal_object_does_not_extend_frontier`
- `empty_anchors_returns_empty_result_no_panic`
- `budget_cutoff_returns_partial_paths`

### 6.2 Integration (real GraphStore, fixture data)

- `two_hop_friend_of_friend` — Alice `MarriedTo` Bob, Bob
  `WorksAt` Acme, query "where does Alice's husband work" returns
  the memory containing the `WorksAt` edge.
- `three_hop_chain_capped_at_depth_3` — verifies max_depth
  enforcement.
- `cycle_via_two_paths` — A→B→C→A; the visited-set blocks the
  return to A within a single path but allows depth-2 termination.

### 6.3 LoCoMo regression (RUN-0007)

- Re-run conv-26 with MultiHop plan enabled.
- **Pass:** multi-hop hit@5 ≥ 33% (1/3) — minimum acceptance.
- **Pass:** single-hop / temporal hit@5 ≥ RUN-0006 baseline
  (no regression on plans we did not touch).
- **Pass:** new RUN-0007.md report includes per-plan rows showing
  MultiHop appearing for cat=multi-hop queries (route confirmation).
- **Stretch:** multi-hop hit@5 ≥ 67% (2/3) — real target.

If RUN-0007 multi-hop is still 0%, the bug is in the *substrate*
(edges aren't there) not the *plan* — that's a separate issue
about ingestion and we file it then.

## 7. Risks

- **Frontier explosion.** `RelatedTo` excluded by default
  mitigates the worst case, but a hub vertex like a city or
  company can still have 100+ inbound edges. `beam_width=8` is
  the bound. Add a `max_edges_per_vertex` safety valve if RUN-0007
  shows budget cutoffs dominating.
- **Score calibration.** Multiplicative decay × edge confidence
  may produce scores incomparable with Factual's scoring scheme
  at fusion time. Fusion module currently treats sub-plan scores
  as already-normalized. If fusion mis-ranks MultiHop candidates
  against Factual ones, normalize MultiHop output to `[0, 1]`
  pre-fusion (rank → quantile map). Defer the actual fix to
  RUN-0007 evidence; don't pre-optimize.
- **Two-anchor heuristic over-fires.** A single sentence with two
  entities ("I met Alice and Bob at lunch") would route to
  MultiHop even though it's a recall question, not a relational
  one. If RUN-0007 shows non-multi-hop queries getting routed to
  MultiHop and missing, tighten the heuristic (require explicit
  relational verb between the two anchors).
- **Predicate-filter default is opinionated.** Excluding
  `RelatedTo` / `MentionedIn` / `Contradicts` is a judgment call
  that could mask real chains. Configurable per call, but the
  default decision should be revisited if a future issue shows
  systematic misses through these predicates.

## 8. Open questions for review

1. Is `MultiHopConfig::decay = 0.7` the right starting point, or
   should it be tunable per-query (e.g., shorter for "find me
   anything connected", longer for strict relational queries)?
2. Should the terminal-vertex memory penalty (`* 0.8`) be a
   config parameter or a constant? Constant is simpler; parameter
   means another knob to defend against in tests.
3. The bench-metadata routing (`locomo_category == "multi-hop"`)
   couples production code to bench-only metadata. Acceptable for
   a known-test-set evaluation, but should we gate it behind a
   `cfg(test)` or a feature flag so it never fires in live agents?
   I lean toward a feature flag named `bench-routing-hints`.
4. Predicate-filter default — does the
   exclude-{RelatedTo, MentionedIn, Contradicts} call match how
   you've seen these predicates used in practice, or is one of
   them actually load-bearing for some chain pattern I haven't
   considered?

---

If approved, implementation order:
1. dispatch.rs PlanKind variant + orchestrator stub arm (compile-only).
2. plans/multi_hop.rs skeleton + types (compile-only, returns empty).
3. Algorithm core (beam search loop, no scoring polish).
4. Predicate filter + candidate conversion.
5. Unit tests.
6. Classifier branch (bench metadata + heuristics).
7. LoCoMo driver metadata wiring + scorer per-plan breakdown.
8. Integration tests.
9. RUN-0007 on conv-26.
