---
title: 'GraphEntityResolver: no fuzzy/lexical-variant alias matching (e.g. art vs abstract painting)'
status: open
priority: P3
severity: limitation
category: retrieval
created: 2026-05-27
relates:
- ISS-165
- ISS-164
discovered_in: ISS-165 post-fix verification probe (engram-bench:f28b41d) on conv-26 q43
---

## Summary

The ISS-165 mention extractor (`engram:a5b0407`) resolves each
n-gram via `Storage::search_candidates`, which does **exact
normalized-alias equality** against `graph_entities.alias`. There
is no fuzzy / synonym / embedding-based fallback.

This means lexical variants of the same concept don't anchor each
other. Concrete case from the ISS-165 validation probe:

```
q43: "What kind of art does Caroline like?"
  query tokens:    art, Caroline, like, kind
  stored entity:   "abstract painting" (canonical)
  anchors:         Caroline, art         (art DID anchor on its own)
```

Wait — `art` *did* anchor here (probe output line shows "Caroline,
art"). So in this specific case it's fine because we happen to
have **both** an `art` entity AND an `abstract painting` entity
in `graph_entities`, and both are correct entry points.

The real failure mode is different and harder: **if the stored
entity were ONLY `abstract painting` and the query said `art`,
the resolver would return zero anchors for `art`**. We don't know
of a current LoCoMo query that hits this, but it's a structural
gap.

## Why it isn't blocking now

- conv-26 ingest stores both broad ("art") and specific
  ("abstract painting") forms because the LLM triple extractor
  emits both. Most LoCoMo questions use one of the two forms
  the extractor already saw.
- The Factual plan's downstream BM25 channel does match on
  `art` as a query term even if the entity resolver misses it.
- We can't quantify the gap until we have a query set that
  deliberately uses synonyms not in the entity table.

## Why it's worth tracking

- It's the limit of the lexical approach. Any future eval set
  with paraphrased questions (HotpotQA-style, or LoCoMo
  authors' rewrites) will surface this.
- ISS-148 AC-5a (single-fact ≥0.60 on conv-26) may be near a
  ceiling imposed by this — single-fact questions rely on the
  resolver pulling the right anchor, and lexical-only resolve
  caps recall.

## Proposed fix options

**Option A: embedding-based fallback.**
If `search_candidates(mention)` returns zero results, fall back
to a cosine search over `entity_embeddings` (already populated
by the Resolution pipeline). Threshold ~0.7. Pros: catches
`art` ↔ `abstract painting`, `book` ↔ `novel`. Cons: another
embedding call per unresolved mention (~30ms each); needs
threshold tuning.

**Option B: hand-curated synonym table.**
`graph_entity_aliases` table mapping canonical entity → list of
known surface forms. Pros: deterministic, debuggable, no extra
LLM/embedding cost. Cons: doesn't scale, needs maintenance.

**Option C: LLM-side at extraction time.**
Modify the triple extractor (Haiku) to emit `also_known_as`
field per entity, populating multiple aliases for the same
entity at ingest time. Pros: no read-time cost. Cons: ingest
becomes more expensive; existing substrates need re-ingest.

**Option D: do nothing.**
The lexical floor is acceptable for now. Revisit if/when an
eval set surfaces the gap.

## Recommendation

Defer. The current ISS-165 fix already moved us from 0/9 to
9/9 anchored on the LoCoMo single-fact set; this issue is at
best a +1–2 question lift. Re-prioritize **only if**:
- ISS-164 Phase 2 sweep shows entity_channel is the right
  direction (i.e. anchors actually drive the metric), AND
- a clear LoCoMo question is shown to fail solely because of
  a lexical-variant miss.

## Acceptance criteria (when revisited)

- [ ] **AC-1**: Pick option (A/B/C); document the choice in
  this issue.
- [ ] **AC-2**: For Option A: add embedding fallback path in
  `GraphEntityResolver::resolve` with configurable threshold;
  default off.
- [ ] **AC-3**: Regression test demonstrating the chosen path
  resolves a known synonym pair.
- [ ] **AC-4**: A/B bench on conv-26 (or a synonym-stress
  fixture) showing whether the lift is real.

## References

- Resolver: `crates/engramai/src/retrieval/adapters/graph_entity_resolver.rs`
- Search path: `crates/engramai/src/graph/store.rs:2046-2073`
  (search_candidates exact-equality)
- ISS-165 fix commit: `engram:a5b0407`
- Validation probe: `engram-bench/examples/iss165_postfix_probe.rs`
