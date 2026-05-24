---
id: ISS-144
title: Classifier NullEntityLookup hardcoded — entity signal永远=0, abstract plan misrouting on factual questions
kind: issue
status: in_progress
priority: P0
severity: degradation
tags:
- classifier
- retrieval
- unified-substrate
- silent-no-op
- root-cause
created: 2026-05-23
related:
- ISS-111
- ISS-136
- ISS-141
- ISS-142
- ISS-143
---

# ISS-144 — Classifier NullEntityLookup hardcoded (silent no-op)

## TL;DR

`Memory::graph_query` (api.rs:495) hardcodes `HeuristicClassifier::with_null_lookup()` →
entity signal in the classifier is **always 0.0**, regardless of how many entities the
unified substrate has stored. Combined with `EntityConfig::default()` shipping with
`known_people: vec![]`, the entire entity-extraction + entity-lookup pipeline is a
3-layer silent no-op chain on free-form conversational corpora like LoCoMo.

**⚠️ Diagnosis refined after batch probe (n=152) — see "Refined diagnosis" section below.**
The original n=1 finding ("Abstract mis-routing is dominant") turned out to be wrong:
only 12% of conv-26 queries route to Abstract. The dominant pattern is 80% route to
**Factual**, but Factual plan's `EntityResolver` is broken by the same root cause
(no entities in graph, no lookup if there were), so anchors are empty and the plan
falls through to fuzzy fallback — which is why **65% of IDK-failed queries have the
gold fact completely missing from top-10**.

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

## Refined diagnosis (batch probe n=152, 2026-05-23)

After the first probe surfaced this ISS, I extended the introspection tool to batch
mode and ran it across all 152 conv-26 queries (no LLM calls, ~10s wall). The original
n=1 hypothesis ("Abstract mis-routing is the dominant failure mode") was **wrong**.

Actual plan distribution on conv-26:

| Plan | All (n=152) | IDK-failed (n=57) | Other (n=95) |
|---|---|---|---|
| Factual    | 80% | 70% | 85% |
| Abstract   | 12% | 14% | 11% |
| Hybrid     | 3%  | 9%  | 0%  |
| Episodic   | 1%  | 4%  | 0%  |
| Affective  | 4%  | 4%  | 4%  |

Key observations:

- **Abstract mis-routing is a minor pattern** (only 14% of IDK failures). It is real
  (q3 lives in this 14%) but not dominant.
- **Factual plan is where most failures happen** — 70% of IDK failures route to Factual.
- **The classifier is NOT the differentiator** — Plan distribution is nearly identical
  between IDK-failed and OTHER. The same `NullEntityLookup` is in play, but routes the
  same way. What differs is what happens **inside** the plan.
- **65% of IDK-failed queries have the gold fact completely absent from top-10**.
  Compared to 45% in the OTHER group. The plan is running, returning 10 candidates,
  and **the right answer just isn't among them**.
- **OTHER group's gold-in-top-K is only 55%** — meaning a meaningful fraction of "correct"
  LoCoMo answers come from the LLM correctly guessing or inferring from semantically
  related but factually different chunks. This is concerning in its own right.

### Revised root cause

The classifier `EntityLookup` is broken (L2) — but its real impact is **not** routing.
It's that the **`EntityResolver` used inside `FactualPlan::execute`** also has nothing
to resolve against, because **Layer 1 (extraction) never wrote any Person entities to
the graph in the first place**.

**Direct probe confirmation (2026-05-23)** — see `entity-resolution-probe.log`:

```
After ingesting 419 conv-26 episodes (every episode mentions Caroline or Melanie):

total entities in graph: 1
  the only entity is "go" (entity_type=technology, used 7x — a Golang false-positive
  on the phrase "go do some research")

Caroline mentions in graph: 0
Melanie mentions in graph: 0

GraphEntityResolver::resolve on 5 IDK-failed factual queries:
  q3  "What did Caroline research?"        → n_anchors=0
  q7  "What is Melanie's marital status?"  → n_anchors=0
  q11 "Where does Caroline want to travel?" → n_anchors=0
  q40 "How many children does Melanie have?" → n_anchors=0
  q55 "What does Caroline enjoy taking photos of?" → n_anchors=0
```

This is **decisive**: `GraphEntityResolver` is a real graph-backed implementation
(adapters/graph_entity_resolver.rs, not Null), so L2 (classifier `NullEntityLookup`)
is the only remaining `Null*` adapter — but it doesn't matter because there's nothing
in the graph to look up. **L1 is the binding constraint.**

So the priority ranking inverts again:

- **L1 (EntityExtractor known_people bootstrapping) = the only true root.** Without it,
  L2 and L3 have nothing to do.
- L2 (classifier `NullEntityLookup` → real GraphEntityLookup) = trivial cleanup *after*
  L1, because nodes will exist.
- L3 (ISS-111 KC collapse) = orthogonal, still real, lower impact since only 14% of
  queries route Abstract.

### Cheapest L1 fix (proposed)

LoCoMo episodes have a fixed shape: `"<Speaker>: ..."` (e.g. `"Caroline: Hey Melanie, ..."`).
The cheapest L1 fix is to **bootstrap `known_people` from the speaker prefixes** in a
preprocessing pass before the main extractor runs. This is:

- Zero LLM cost (regex match `^([A-Z][a-z]+):`)
- Zero new dependencies (no NER model)
- Covers all dialogue-style corpora (LoCoMo, AGI Eval dialog tracks, dialogue-style
  benchmarks, future user chats with engram)
- Generalises to a `Source-derived known-list bootstrap` design pattern: any well-shaped
  corpus annotates its own speakers, future GitHub issues bootstrap repo + issue IDs,
  emails bootstrap senders, etc.

Real production paths (potato's RustClaw / engram use cases) that don't have a speaker
prefix would still rely on user-supplied `known_people` config OR an NER fallback. But
this fix gets LoCoMo (and any structured-dialogue eval) from "0 entities" to "near-full
entity coverage" with a 5-line regex.

### Secondary independent finding from probe logs

```
[engramai::memory] Dedup: merging into existing memory c2cee531 (similarity: 0.9529)
```

Engram aggressively deduplicates conversational episodes by embedding similarity
(threshold ~0.95). Many conv-26 episodes are short repetitive utterances ("Wow, that's
cool!", "Thanks, Mel!"), which trigger this. Effect: 419 ingested episodes likely
result in significantly fewer storage rows. This may or may not be a problem —
deduping noise is good, deduping evidence is bad — but it's worth measuring after L1
is fixed, because it may further suppress evidence the Factual plan needs to walk.

File as separate ISS if needed; not blocking ISS-144.

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

## 2026-05-23 update — L1 fix landed, L1b discovered

**L1 (entity extraction) is FIXED** in commit `7eee30e` on branch
`iss144-l1-speaker-prefix-extraction` (merged to main). New `EntityPattern`
in `crates/engramai/src/entities.rs` with regex `(?m)^(\p{Lu}\p{L}+):`
extracts the speaker tag in dialogue-style episodes. Plus 4 unit tests
(multiline, single-turn, single-letter rejection, mid-sentence rejection).
1946 lib tests green.

Post-fix entity census on conv-26 (re-ran `iss144_entity_resolution_spike`):

```
total entities: 3
  "person"             2
  "technology"         1
top by usage:
  [ 210x] "person" "default" :: caroline
  [ 208x] "person" "default" :: melanie
  [   7x] "technology" "default" :: go
```

But `GraphEntityResolver::resolve` STILL returns `n_anchors=0` on all 5
probes. **L1 was necessary but not sufficient.** Root cause of the residual
gap (filed as **ISS-145**):

- `ingest_with_stats_at` (memory.rs:7244) writes to `entities` +
  `memory_entities` via `Storage::upsert_entity`. It does NOT call
  `ResolutionPipeline`, so no rows land in `graph_entities` /
  `graph_entity_aliases`.
- `GraphEntityResolver` reads exclusively from `graph_entity_aliases`.
- The two table families share a SQLite connection but no writer.

ISS-075's fix (commit `f95480b`) wired the resolution pipeline's
`stage_persist.rs::build_delta` to emit `AliasUpsert` correctly — but
the pipeline is never invoked by the ingest path that LoCoMo + every
default caller uses. ISS-075 is effectively dead code for benchmark
runs.

**Decision still pending on L1b** (ISS-145 documents 3 options). Until
L1b lands, Factual anchor resolution remains broken. L1 alone may still
move LoCoMo numbers via other readers of the `entities` table (dedup's
`find_entity_overlap`, future classifier `EntityLookup` once wired) —
isolated impact measurement is the next step.

**Status: in_progress** (L1 done, L1b open as ISS-145).
