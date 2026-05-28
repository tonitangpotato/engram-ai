---
title: 'GraphEntityResolver: verb/common-noun false positives in n-gram mention extraction'
status: deferred
priority: P3
severity: noise
category: retrieval
created: 2026-05-27
relates:
- ISS-165
- ISS-164
discovered_in: ISS-165 post-fix verification probe (engram-bench:f28b41d) on conv-26 q76
deferred_at: 2026-05-28
deferred_reason: 'ISS-164 entity_channel falsified — ISS-169 noise floor only matters if entity_channel revives. Per ISS-169 own recommendation: moot.'
---

## Summary

The ISS-165 token n-gram mention extractor (added at `engram:a5b0407`)
is intentionally lexical — every alphanumeric token of length ≥
`MIN_UNIGRAM_CHARS` becomes a candidate mention, and each is
search_candidates-resolved against `graph_entities`. This means
any common English verb or noun that happens to **also** be a
stored entity will resolve as an anchor, even when it's clearly
being used as a verb in the question.

Concrete case from the ISS-165 validation probe:

```
q76: "When did Melanie go on a hike..."
  anchors: Melanie, Go, roadtrip, hike   (4 anchors)
```

`Go` was extracted from an earlier conversation episode (probably
the programming language) and persisted in `graph_entities`. The
question is asking about Melanie's hiking trip — `go` here is the
auxiliary verb, not a topic entity. The anchor adds noise to the
Associative plan's seed_entities and may pull in unrelated
"Go (language)" episodes via `memories_mentioning_entity`.

## Why it isn't blocking

- The Factual / Associative plan's downstream ranking (BM25 +
  embedding fusion + MMR) is generally strong enough to push the
  irrelevant Go episodes below the K=10 cut on conv-26.
- Multi-anchor queries still benefit from the correct anchors
  (Melanie, hike, roadtrip in this case).
- The bigger lift in ISS-165 (0/9 → 9/9 anchored) dominates this
  residual ~1 false-anchor-per-query noise floor.

## Why it's worth fixing

- It's measurable noise. If we later turn entity_channel on by
  default and a query's only correct anchor is a person name,
  the verb false-positives could outvote the right anchor in
  Step 3b' direct fan-out.
- Fix is cheap and self-contained — no new deps, no schema
  change.

## Proposed fix options

**Option A (simplest, ship-friendly): stopword list.**
Add a small hardcoded `VERB_AND_COMMON_NOUN_STOPWORDS` set to
`graph_entity_resolver.rs::extract_mentions`. Filter tokens
matching the set **before** the search_candidates call. Initial
set (English-only, ~30 words):

```
go, goes, going, gone, get, got, getting, take, took, taking,
make, made, making, do, did, doing, have, had, having, be,
been, being, was, were, is, are, say, said, says, see, saw,
seeing, know, knew, knowing, think, thought, want, wanted,
give, gave, find, found, work, look, looked, come, came
```

Pros: O(1) lookup, zero false negatives on entity names like
"Sweden" (since stopwords are English verbs, not proper nouns).
Cons: English-only; would need per-locale config later.

**Option B (more principled): entity-prior filtering.**
Track `mention_count` per entity in `graph_entities` (already
plumbed through Resolution pipeline). At resolve time, if a
mention matches an entity whose `mention_count` is in the
bottom Nth percentile **and** the mention token is in a
syntactic-position-likely-to-be-verb context, drop it. Needs
schema work + PoS tagging — heavier than the noise warrants.

**Option C (defer to LLM NER): supersede this skill with the
classifier pipeline.**
ISS-165's n-gram scan is explicitly the cheap path. The
long-term direction (per `v04-unified-substrate/design.md
§4.13`) is to lift NER out of resolve() and into the
classifier/extractor stage. If we're ~2 weeks from that, just
sit on the noise.

## Recommendation

Option A behind a config flag (default ON, opt-out only for
benchmark reproducibility), ship if-and-only-if ISS-164 Phase
2 sweep shows entity_channel is worth keeping. If sweep
falsifies entity_channel altogether, this issue is moot.

## Acceptance criteria

- [ ] **AC-1**: `extract_mentions` accepts a stopword set and
  filters tokens matching it before search_candidates.
- [ ] **AC-2**: Default stopword set documented in module
  doc + covers the ~30 English verbs above.
- [ ] **AC-3**: Test `test_extract_mentions_filters_stopwords`
  passes — `"When did Melanie go on a hike"` extracts only
  `Melanie`, `hike` (not `go`, `did`, `When`).
- [ ] **AC-4**: All 6 ISS-165 regression tests still pass
  (no false negatives on entity names).
- [ ] **AC-5**: If kept opt-in, config flag is in
  `FusionConfig` or `ResolverConfig` (not env-var-only).

## References

- Token extractor: `crates/engramai/src/retrieval/adapters/graph_entity_resolver.rs::extract_mentions`
- ISS-165 fix commit: `engram:a5b0407`
- Validation probe output: `engram-bench/examples/iss165_postfix_probe.rs`
  (q76 line shows the `Go` false positive)

---

## 2026-05-28 — moot, deferred behind entity_channel revival

ISS-169's own recommendation:

> Option A behind a config flag (default ON, opt-out only for
> benchmark reproducibility), ship if-and-only-if ISS-164 Phase
> 2 sweep shows entity_channel is worth keeping. If sweep
> falsifies entity_channel altogether, this issue is moot.

ISS-164 status = **falsified** (Phase 2 sweep, conv-26: SF +0,
overall −3.29 pp, multi-hop −10.81 pp; locked default `false`).

The `Go` / verb / common-noun false-positive anchors only matter
when those anchors are actually consumed by a retrieval channel.
With `entity_channel` off by default (locked envelope), the
`graph_entity_resolver` n-gram mention extractor's output is not
flowing into Associative Step 3b' direct fan-out in production
benches.

Status: **deferred**. Re-open if:

1. ISS-164 entity_channel is revived under a different design
   (e.g. anchor-confidence-weighted, gold-question-distilled),
   **or**
2. The classifier's `EntityLookup` (ISS-149) gets unblocked and
   starts routing Factual on a substantial query share, **or**
3. A separate consumer of `graph_entity_resolver::resolve` lands
   that's sensitive to anchor noise.

None of the above are on the near-term roadmap as of 2026-05-28.
Keep the issue filed for traceability but mark deferred so we
don't ship dead code.
