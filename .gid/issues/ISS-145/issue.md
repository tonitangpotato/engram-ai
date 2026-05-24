---
id: ISS-145
title: ISS-144 L1b — Memory ingest path doesn't populate graph_entity_aliases, leaving GraphEntityResolver blind
status: open
priority: P1
labels:
- retrieval
- resolution
- factual-plan
- locomo
- root-cause
relates_to:
- ISS-144
- ISS-075
- ISS-076
- .gid/issues/ISS-144/issue.md
---

# ISS-145: L1b — ingest never writes aliases, so Factual anchor resolution is blind

## TL;DR

`Memory::ingest_with_stats_at` (the path LoCoMo + every production caller
uses) writes entities into the `entities` + `memory_entities` tables via
`Storage::upsert_entity`. It **does not** write to
`graph_entities` / `graph_entity_aliases`. `GraphEntityResolver` (used
by the Factual retrieval plan as its `EntityResolver` implementation)
reads exclusively from `graph_entity_aliases` → `graph_entities` via
`GraphRead::search_candidates`. Result: anchor resolution always returns
0 anchors in benchmarks, every Factual query degrades to
`DowngradedNoEntity`.

This is the second half of the ISS-144 root cause. ISS-144 L1
(commit `7eee30e`) fixes the extractor so the *first* table family
finally has Person entities (210x Caroline, 208x Melanie on conv-26).
This issue (L1b) is about wiring the *second* table family so the
resolver can see them.

## Evidence

`engram-bench/examples/iss144_entity_resolution_spike` on conv-26 after
the L1 fix landed (branch `iss144-l1-speaker-prefix-extraction`,
2026-05-23):

```
--- ENTITY CENSUS (Memory::list_entities, limit=10000) ---
total entities: 3
  "person"             2
  "technology"         1
top 30 entities by usage count:
  [ 210x] "person" "default" :: caroline
  [ 208x] "person" "default" :: melanie
  [   7x] "technology" "default" :: go

--- GraphEntityResolver::resolve direct probe ---
[conv-26-q3]  question: What did Caroline research?       n_anchors: 0
[conv-26-q7]  question: What is Melanie's marital status? n_anchors: 0
[conv-26-q11] question: Where does Caroline want...?      n_anchors: 0
[conv-26-q40] question: How many children does Melanie...? n_anchors: 0
[conv-26-q55] question: What does Caroline enjoy...?      n_anchors: 0
```

210 / 208 Person entities exist in the `entities` table; the resolver
sees zero. The two table families are wired to different writers and
different readers, with no overlap.

## Root cause (code-path audit)

Writers — what writes to which family during `ingest_with_stats_at`:

- `entities` + `memory_entities` ← `Storage::upsert_entity` called from
  `memory.rs:2850, 2899, 2978, 6462` during dedup/store paths.
- `graph_entities` + `graph_entity_aliases` ← `SqliteGraphStore::insert_entity`
  + `upsert_alias` (graph/store.rs:3778, 4013). Only writer in
  production is `crates/engramai/src/resolution/stage_persist.rs::build_delta`,
  driven by `ResolutionPipeline::resolve_entities`. ISS-075 commit
  `f95480b` made that path emit AliasUpsert correctly.

Reader — `GraphEntityResolver::resolve`
(`retrieval/adapters/graph_entity_resolver.rs:75`):

```rust
let matches = self.graph.search_candidates(&candidate_query);
```

`search_candidates` (graph/store.rs:2050+) does
`SELECT canonical_id FROM graph_entity_aliases WHERE alias = ?` →
joins `graph_entities`. Reads zero of the two `entities`/`memory_entities`
tables.

The unconnected wiring: `ingest_with_stats_at`
(memory.rs:7244) returns a *default* `ResolutionStats` after `store_raw`
completes. It does NOT call `ResolutionPipeline::resolve_entities`. So
the only production writer to `graph_entity_aliases` is dead code with
respect to LoCoMo and any other caller that goes through `ingest_*`.

## Why the existing ISS-075 fix doesn't cover this

ISS-075's fix correctly makes `stage_persist.rs::build_delta` emit
AliasUpsert rows on CreateNew/MergeInto. But that fix only fires if
`ResolutionPipeline` runs. The ingest path that LoCoMo (and the
default `Memory::add` user) goes through never invokes the pipeline —
it stops at `store_raw` + `upsert_entity`. Verified by `grep` in
`memory.rs::ingest_with_stats_at`: zero references to `ResolutionPipeline`
or `resolve_entities`.

## Three options for L1b

Ordered by my (current) preference; pick before implementing.

### Option A — Dual-write aliases at `upsert_entity` time

In `Storage::upsert_entity` (or in `memory.rs` next to its callers at
2850/2899/2978/6462), additionally call `insert_entity` +
`upsert_alias` on the graph-store side. Each extracted entity becomes:

1. an `entities` row (existing)
2. a `memory_entities` link (existing)
3. a `graph_entities` row (new)
4. a `graph_entity_aliases` row keyed on the surface form (new)

Cost: ~30 lines, symmetric to existing T13/T21 dual-write patterns
in the v04 substrate work. Doesn't run the LLM-driven resolution
pipeline (no semantic merge, just direct passthrough).

Risk: the resolution pipeline's whole point is to *merge* variants
("Caroline" / "caroline" / "Caroline (her sister)" all → one
canonical_id). Dual-writing from `upsert_entity` skips that — we'd
get one canonical_id per surface form variant. For LoCoMo that's
probably fine (low-variant), but it sidesteps the design intent.

### Option B — Wire `ingest_with_stats_at` to drive `ResolutionPipeline`

Make ingest invoke the existing (and ISS-075-fixed) resolution
pipeline after `store_raw`. Reuses the merge semantics, satisfies the
design intent.

Cost: bigger — pipeline expects extracted *triples* not just entities
(see `stage_extract.rs`), and triple extraction currently routes
through an LLM call per episode. For LoCoMo's 419-episode conversation
that's 419 LLM calls per replay = significant $$ and latency. Need
to decide whether to add a triple-free entity-only path or just accept
the cost.

### Option C — Repoint `GraphEntityResolver` at the `entities` table

Change `search_candidates` (or write an alternative resolver) to read
from `entities`/`memory_entities` directly. Smallest diff.

Risk: collapses two intentionally separate stores. The v04 substrate
design (`docs/design/v04-unified-substrate/design.md`) likely has a
position on whether `graph_entities` and `entities` are supposed to
converge — need to read that before picking C.

## Acceptance criteria

1. `iss144_entity_resolution_spike --conv conv-26` reports
   `n_anchors >= 1` on all 5 IDK-failed Factual probes.
2. `cargo test -p engramai --lib` stays green.
3. Full LoCoMo 152q run on conv-26 (or at minimum the existing 25q
   smoke) shows Factual category accuracy improves over the L1-only
   baseline.
4. One ADR-style note in the issue body documenting which Option
   was taken and why (especially if C — must justify against v04
   design).

## Out of scope

- L2 classifier `NullEntityLookup` wiring (api.rs:495). Tracked separately.
- L3 ISS-111 KC cluster-collapse on dense single-domain corpora.
- v0.3 resolution pipeline architecture changes (the pipeline itself
  is fine; the gap is its non-invocation from ingest).

## References

- ISS-144 — L1 extraction fix (commit `7eee30e`)
- ISS-075 — original alias-upsert wiring fix (commit `f95480b`)
- ISS-076 — embedding propagation in resolution
- `engram-bench/examples/iss144_entity_resolution_spike.rs` — direct probe
- `crates/engramai/src/retrieval/adapters/graph_entity_resolver.rs` — reader
- `crates/engramai/src/resolution/stage_persist.rs` — ISS-075 writer
- `crates/engramai/src/memory.rs:7244` — `ingest_with_stats_at` (the gap)

---

## 2026-05-24 update — Option D added (read-switch aligned with v04)

Re-audited after ISS-148 root cause work landed. The 2026-04-23 framing
of A/B/C is **incomplete** because v04 unified-substrate progressed
substantially since this issue was filed:

- **ISS-122 (fixed, `cb9e2e9`)**: `Storage::upsert_entity` already
  **dual-writes** to `nodes(node_kind='entity')`. The 210 Caroline /
  208 Melanie entity rows the spike found in `entities` ALREADY have
  matching `nodes` rows.
- **T29.5 (shipped)**: `Memory::get_entity` / `find_entities` /
  `list_entities` / `get_entities_for_memory` were read-switched to
  read from `nodes` when `unified_substrate=true`.
- **T31 (shipped)**: unified-substrate default flipped on
  (`MemoryConfig::default_unified_substrate=true`).

**What was NOT switched**: `GraphEntityResolver::search_candidates`
(`crates/engramai/src/graph/store.rs:1985-2080`). It still reads
`graph_entity_aliases → graph_entities` exclusively, even when the
runtime is in unified mode. The 210 entity rows in `nodes` are
invisible to it.

Per v04 design §5.6.2 (`docs/design/v04-unified-substrate/design.md:1328`):

> `graph_entities` (created graph/storage_graph.rs:219) — duplicate of
> `entities`, superseded by `nodes(node_kind='entity')`

Phase F drops `graph_entities` entirely. Any new code that *writes more*
to `graph_entities` (Options A and B) becomes dead weight the moment
Phase F lands. **Options A and B are now strategically wrong.**

### Option D — `search_candidates` read-switch (parallel to T29.6 FTS)

Add an `unified_substrate`-gated read path to
`GraphEntityResolver::search_candidates` that targets
`nodes(node_kind='entity')` + (TBD: which JSON-attribute key carries
the alias surface form). When the flag is on, the resolver reads from
the same store T29.5 already populates — closing the gap with
**zero** new writers, zero new tables.

Cost: similar size to T29.6 — a single read-switch in one method,
plus contract tests asserting parity with the legacy path on a fixed
fixture. Plus probably an indexing concern (the legacy path uses a
partial index `idx_graph_aliases_canonical`; the unified path needs
an equivalent index on `nodes` keyed by alias surface).

Open question for potato (do not implement without an answer):

1. **Alias storage in `nodes`.** Today `upsert_entity` writes the
   entity name as `nodes.name` (or as a JSON attribute?). The legacy
   path has a separate `graph_entity_aliases` table with multiple
   aliases per canonical id. If the unified projection only stores
   one surface form per node, alias variation is lost. Need to
   confirm via `nodes` schema + `upsert_entity` projection what's
   actually written. If aliases are flattened to one `name`, Option D
   needs an additional design step.

2. **Read-switch order.** T29.6 FTS read-switch shipped behind the
   flag with the caveat: "production `nodes_fts` has only the
   post-T12-dual-write era of memories; before T26c backfill,
   recall under `unified_substrate=true` is degraded for pre-dual-
   write rows." Same caveat applies here for `nodes(node_kind='entity')`:
   only post-ISS-122-dual-write entities are visible. Pre-ISS-122
   entities require backfill (T26c-equivalent for entities). For
   bench/LoCoMo this is fine — every run starts from an empty
   store. For prod this is a real migration concern.

3. **Sequencing with ISS-149.** ISS-149 (L2 classifier
   `NullEntityLookup` wiring) is a separate read path with the
   same underlying question (where does the classifier look up
   entity tokens). If Option D switches `GraphEntityResolver`,
   ISS-149's `GraphEntityLookup` should also read from `nodes`
   to stay consistent. Cleanest: ship Option D first, then layer
   ISS-149 on top.

### Recommendation

Defer this decision to potato. The cleanest path is Option D + answer
to the alias-storage question, but it touches design surface that
deserves a real conversation, not a midnight commit.

Status remains `open`. Blocks ISS-149 (L2 needs the same read store).
