---
id: ISS-144
title: "Classifier NullEntityLookup hardcoded — entity signal永远=0, abstract plan misrouting on factual questions"
kind: issue
status: open
priority: P0
severity: degradation
tags: [classifier, retrieval, unified-substrate, silent-no-op, root-cause]
created: 2026-05-23
related: [ISS-111, ISS-136, ISS-141, ISS-142, ISS-143]
---

# ISS-144 — Classifier NullEntityLookup hardcoded (silent no-op)

## TL;DR

`Memory::graph_query` (api.rs:495) hardcodes `HeuristicClassifier::with_null_lookup()` →
entity signal in the classifier is **always 0.0**, regardless of how many entities the
unified substrate has stored. This silently mis-routes factual questions ("What did X
research?") to the Abstract plan, which then `DowngradedFromAbstract { reason: "l5_unavailable" }`
(because of ISS-111 KC cluster collapse) and falls back to bare vector RAG. Gold facts then
miss top-K.

**This is the dominant failure mode on LoCoMo conv-26 IDK failures.** Found while building
the introspection probe in `engram-bench/examples/iss144_introspect_one.rs` (the very
first query I tried surfaced this).

## Evidence

### Probe run — conv-26-q3

Query: `What did Caroline research?` (gold: `Adoption agencies`, category: single-hop)
Corpus: full conv-26 (419 episodes) into fresh in-memory engram (default unified_substrate).

```
[dispatch] intent=Abstract plan_kind=abstract method=Heuristic
  scores=SignalScores { entity: 0.0, temporal: 0.0, abstract_: 1.0, affective: 0.0, associative: 0.0 }
[execute_plan ENTER] plan_kind=abstract query_limit=10
[fallback ENTER] trigger=abstract reason=l5_unavailable
[fallback EXIT] candidates=10 outcome=downgraded_from_abstract
[execute_plan EXIT] outcome=DowngradedFromAbstract { reason: "l5_unavailable" }

plan_used: Abstract
outcome:   DowngradedFromAbstract { reason: "l5_unavailable" }
n_results: 10
gold-tokens present in any top-10 result: false
```

Top-1 result is "Caroline: ... I'm off to go do some research." (ep16). The actual gold
fact (ep25: "Researching adoption agencies — it's been a dream …") never enters top-10.

### Code-level confirmation

- `src/retrieval/api.rs:495` — `Memory::graph_query` constructs `HeuristicClassifier::with_null_lookup()`. No production code path passes a real `EntityLookup`.
- `src/retrieval/classifier/heuristic.rs:131-135` — `NullEntityLookup::lookup` always returns `EntityMatch::None`, so `score_entity()` always returns 0.0.
- `src/retrieval/classifier/heuristic.rs` — the only `impl EntityLookup` in the entire crate are `NullEntityLookup` (prod default) and `StubLookup` (a unit-test stub at line 426).
- `src/retrieval/dispatch.rs:286, 393` — `dispatch.rs` tests also wire `NullEntityLookup`.
- `src/graph/store.rs` — has `entities_in_episode(uuid)` / `entities_linked_to_memory(memory_id)` but **no `lookup_by_name(token)`** API, so the `EntityLookup` impl wouldn't even have what it needs today.

The comment at `heuristic.rs:113-117` says:
> The classifier-core (`task:retr-impl-classifier-core`) wires a real graph-backed implementation
> behind this trait once `v03-graph-layer` is available. Until then `NullEntityLookup` is used
> and `score_entity` trivially returns `0.0`.

v0.3 graph layer landed (T29 + T32 flipped `unified_substrate=true` by default). The TODO
was never closed.

## Why this is a unified-substrate audit finding

The classifier reads from a separate "entity lookup" abstraction. When the substrate was
consolidated (T20–T32), nodes/edges got populated with entities, but **no one wired the
classifier to read them**. The chain is broken silently — no log warning, no test failure,
the signal just stays 0 in production forever.

This is exactly the class of bug the introspection work was meant to surface. ✅

## ⚠️ Deeper layer — entity extraction itself is gated on known_people config

Investigated `src/entities.rs::EntityExtractor::new`. The extractor is **Aho-Corasick over a configured name list** (`EntityConfig::known_projects` / `known_people` / `known_technologies`) PLUS regex for structural patterns (ISS-IDs, file paths, URLs, `@mentions`). There is **no NER** — no statistical model, no PROPN heuristic, nothing that would extract "Caroline" as a Person entity unless `Caroline` is pre-loaded into `config.known_people`.

`EntityConfig::default()` returns `known_people: Vec::new()`. The built-in pre-loaded list is **only technology names** (Rust, Python, Tokio, etc.).

The LoCoMo harness uses `fresh_in_memory_db()` which builds Memory with defaults → **the conv-26 ingest path stores zero Person entities for Caroline/Melanie**. So even if Layer 2 (classifier `EntityLookup`) is fixed, Layer 1 already lost the entity at ingest time.

This is a **3-layer silent no-op chain**:

```
Layer 1: extraction       NullEntityList     → no "Caroline" Person node written to graph
Layer 2: classifier       NullEntityLookup   → entity signal=0 even if node existed
Layer 3: abstract plan    KC cluster-collapse → DowngradedFromAbstract on every Abstract route (ISS-111)
```

All three need fixing to recover factual-question accuracy on free-form corpora like LoCoMo.

### Fix-order recommendation

1. **Layer 1 first** — without entity nodes in the graph, fixing Layer 2 has nothing to read.
   Options: (a) plug an NER model (spaCy / Stanza / GLiNER), (b) extract proper-noun
   candidates heuristically (PROPN regex + post-filter), (c) bootstrap known_people from
   the corpus itself (collect tokens that appear as conversational turn speakers, repeated
   capitalised tokens, @mentions). Option (c) is cheapest, would auto-populate Caroline
   and Melanie from the LoCoMo turn-prefix format `"Caroline: ..."`.
2. **Layer 2** — wire `EntityLookup` to read from the now-populated `nodes` table.
3. **Layer 3** — ISS-111 KC clusterer fix (separate root-cause work).

## Compounding interaction with ISS-111

Even after fixing the classifier, factual questions with low-entity-signal corpora may
still mis-route to Abstract. **ISS-111 (KC cluster-collapse on dense single-domain corpora)
guarantees DowngradedFromAbstract → bare RAG fallback for every Abstract route**. So:

- ISS-144 fix alone → better routing (more questions land on Factual plan)
- ISS-111 fix alone → Abstract plan stops being permanently degraded
- Both together → real cognitive-grade retrieval on LoCoMo

This is the **real root** below all the LoCoMo MMR/HyDE/list-aware patches in
ISS-139/141/142/143.

## Hypothesised classifier behaviour after fix

With a real `EntityLookup` backed by `nodes` table:
- "What did Caroline research?" → "Caroline" hits entity table → `score_entity ≥ 0.7`
- entity signal becomes "strong" → §3.2 Stage-1 routes to **Factual**, not Abstract
- Factual plan runs entity resolution → walks `Caroline → researches → X` edges
- If those edges exist in the graph, gold fact is recovered surgically (not by fuzzy vector match)

This needs to be verified on the same probe after the fix lands.

## Acceptance criteria (rough — will refine after spike)

1. A real `EntityLookup` impl exists, backed by the `nodes` table (or whatever stores
   v0.3 entity rows). Lookup is by ASCII-lowercased name token, returns `EntityMatch::Exact`
   when a node's canonical name matches, `Alias`/`Fuzzy` per existing semantics.
2. `Memory::graph_query` wires the real lookup, not `NullEntityLookup`. (Probably constructs
   the classifier once, sharing the storage handle.)
3. `engram-bench/examples/iss144_introspect_one.rs` on `conv-26-q3` shows `entity ≥ 0.7`,
   `plan_used = Factual` (not Abstract), and the gold fact in top-10.
4. LoCoMo conv-26 IDK-failure count drops measurably (need to re-run baseline).
5. No regression on existing classifier unit tests (`heuristic.rs::tests`, `dispatch.rs::tests`).

## Out of scope

- Not changing the entity-extraction-from-text path (entities are presumably already being
  extracted during `ingest_with_stats_at` — verify in spike).
- Not changing the Factual plan internals (anchor resolution, edge walking) — those use
  the existing `EntityResolver` trait inside the plan, which is different from the
  `EntityLookup` trait used by the classifier.
- Not fixing ISS-111 here. ISS-144 + ISS-111 are independent root fixes that compound.

## Related

- ISS-111 — KC cluster-collapse forcing DowngradedFromAbstract for every Abstract route
- ISS-136 — LoCoMo regression hunt (this issue is one root cause)
- ISS-141 / ISS-142 / ISS-143 — surface-level retrieval patches; deprioritised in favour
  of root fixes
- `task:retr-impl-classifier-core` — original ungated task in design docs (presumed still
  open; not verified in this filing)
- `engram-bench/examples/iss144_introspect_one.rs` — introspection probe used to find this
