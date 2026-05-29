---
id: ISS-189
title: "LoCoMo recall failure root cause: misclassification → Hybrid → crippled sub-plan recall + BM25 dominated by common tokens"
status: open
priority: P0
severity: degradation
tags: [retrieval, classifier, hybrid-plan, bm25, locomo, root-cause]
created: 2026-05-29
relates_to: [ISS-149, ISS-175, ISS-147, ISS-152, ISS-188]
---

# ISS-189: The real LoCoMo recall defect (code-verified)

## Why this issue exists

10+ retrieval-side levers were falsified in one week (MMR, BM25 pool widening,
entity channel, factual reweight, cross-encoder, populate-embeddings, …). They
all failed because they tune **fusion / ranking**, but for the failing questions
the gold episode is dropped **before fusion** — in classification and sub-plan
recall. All prior levers were also tuned on **conv-26**, whose bottleneck is
generation/judge, not retrieval. This investigation used **conv-44** (cleaner
annotation: only 4/93 gold items lack a source keyword) and dumped the full
pre-truncation candidate pools (`ENGRAM_DUMP_FUSED_POOL_DIR`, the ISS-187
mechanism) to read what actually happens at each layer.

**Probe run:** `ISS189-conv44-dump-20260529T065211Z`, ISS-161 Arm A envelope
(K=10, temp=0, HyDE off, MMR off, entity_channel off, pipeline_pool=1).
Dumps: `/tmp/iss189-dump/conv-44-q{29,35,36,21,9,2}-*.jsonl`.

## Evidence summary

conv-44 list-questions, all scored 0.0 ("I don't know"):

| qid | question | classified | prefusion pool | vector/bm25 present | gold in pool |
|---|---|---|---|---|---|
| q29 | What **are** the names of Audrey's dogs | Hybrid | 10 | none (all None) | 0 |
| q35 | What **are** the names of Andrew's dogs | Hybrid | 10 | none | 0 |
| q36 | What **are** some foods Audrey likes | Hybrid | 10 | none | 0 |
| q21 | What **are** the classes Audrey took | Hybrid | 10 | none | 0 |
| q9  | What **kind of** classes…**has** Audrey joined | Factual | 281 | all present | 26 hits, ranks 21–187 |
| q2  | What **kind of** indoor activities…**has** Andrew | Factual | 178 | all present | 23 hits |

Same DB, same ingestion. The data, embeddings, and extraction are all fine —
q9/q2 prove the gold episodes are recallable. The failure is structural, in two
chained places, plus a downstream BM25 defect on the Factual path.

---

## Bug 1 — Classifier over-generalizes the `abstract` signal

**File:** `crates/engramai/src/retrieval/classifier/heuristic.rs:232`

```rust
abstract_regex = r"\b( what\s+(has|have|am\s+i|are|were|did) | summari[sz]e | ... )\b"
```

The doc comment one block up (line 225) says the design §3.2 set is only
`what (has|have)`. The code added `are | were | did | am i`. Enumeration
fact-questions ("What **are** the names of X", "What **did** X do") are NOT
thematic/summary queries, but they match `abstract = 1.0`.

Combined with `entity("Audrey") ≥ 0.7`, that is **2 strong signals**, so
`route_stage1` (`classifier/mod.rs:216`) returns `Intent::Hybrid`
(rule: `|strong| ≥ 2 and each ≥ τ_high → Hybrid`).

Verified: q21/q29/q35/q36 start "What are" → match → Hybrid. q2/q9 are
"What kind of … has" → `what` not adjacent to `has` → no match → single Factual.

**This is a code/design deviation, not intended behavior.**

## Bug 2 — Hybrid's Factual sub-plan recall channel is crippled

**File:** `crates/engramai/src/retrieval/orchestrator.rs:784`
(`HybridDispatchExecutor::run`, `SubPlanKind::Factual` arm)

The Hybrid Factual sub-plan calls `FactualPlan::execute(entity_resolver, graph)`
— a **graph-anchor-only** path. It skips the vector + BM25 rerank that the
single-Factual-plan path runs (design §4.1 step 5: "rerank candidate memories
through hybrid_recall-style scoring (vector + BM25)").

Result: q29 (Hybrid) pool = 10 candidates, `vector_score`/`bm25_score`/
`graph_score` **all None**, 0 gold. q9 (single Factual) pool = 281, all three
sub-scores populated (vector 0.60–0.70), 26 gold hits.

**Hybrid is weaker than its own Factual component.** Design §4.7 (lines 232–233)
says `vector_score` applies to Hybrid and `bm25_score` to "all", so the
implementation deviates: the sub-plan should carry vector/bm25 scores for RRF.

## Bug 3 — Top-10 truncation drops mid-ranked gold on the Factual path

**File:** fusion truncation (api.rs Stage-C truncate to `query.limit`)

Even when classified correctly (q9, Factual), the gold lands at fused ranks
**21, 28, 57, 65, 87, 88, 114, 138, 187** — all outside top-10. The top-10 is
filled with generic "Audrey + dogs" memories (fetch, adventures, hikes, pet
store) at score 0.83–0.86; gold ("agility classes", "dog training course",
"positive reinforcement workshop") sits at 0.65–0.73. So even a recalled gold
is truncated away before generation. This is why the answer is still "I don't
know" despite 26 gold candidates in the pool.

## Bug 4 — BM25 is dominated by common tokens (the deepest cause of Bug 3)

**File:** `crates/engramai/src/storage.rs:3017` (`search_fts_with_scores`),
called from `orchestrator.rs:1041` via `fts_scores(&query.text, bm25_pool)`.

In q9's 281-candidate pool, only **12 (4%)** have `bm25 > 0`. Worse, gold
candidates containing the **exact query words** score `bm25 = 0.000`:

- "Audrey is taking a dog **training course**…" → bm25 = 0.000 (rank 87)
- "…**positive reinforcement** dog training workshop…" → bm25 = 0.000 (rank 65)
- only "agility classes" got bm25 = 0.595 (rank 21)

Mechanism: `search_fts_with_scores` builds the FTS query as
`"word1" OR "word2" OR …` over the **entire** question, then `ORDER BY rank
LIMIT bm25_pool`. FTS5's BM25 is dominated by the high-frequency common tokens
("dogs", "care", "Audrey", "take", "better") that appear across the whole conv,
so rows matching only the **rare, discriminating** tokens ("training course",
"agility", "positive reinforcement") rank below `bm25_pool` and fall to
`unwrap_or(0.0)`. This explains why ISS-152's pool-widening (100/200) did
nothing — the problem is the OR-query letting common tokens drown rare ones,
not pool size. Compounded by the combiner's `text = max(vector, bm25)`
aggregate (ISS-175 Bug 3), which discards even the bm25 that does fire.

---

## The failure chain

```
"What are the names of Audrey's dogs"
  → Bug 1: "what are" matches abstract regex → 2 strong signals → Hybrid
  → Bug 2: Hybrid Factual sub-plan = graph-anchor only, no vector/bm25
  → gold episode never enters the 10-candidate pool
  → fusion/ranking levers operate on a pool that already lost gold
  → all 10 prior levers falsified

"What kind of classes has Audrey joined"  (classifier OK, Factual)
  → Bug 4: BM25 OR-query dominated by common tokens, gold bm25 = 0
  → Bug 3: gold lands at fused rank 21–87, top-10 truncation drops it
  → "I don't know" despite 26 gold candidates in the 281-pool
```

Same class as ISS-149 (classifier death). The retrieval levers were never the
binding constraint for these questions.

## UPDATE 2026-05-29: Bug-1 fix verified INSUFFICIENT — embedding recall is the true root

Force-intent experiment (`ENGRAM_BENCH_FORCE_INTENT=factual`, run
`ISS189-force-factual-20260529T073609Z`, dumps `/tmp/iss189-force/`) forced the
6 enumeration questions onto the single Factual plan, bypassing the
mis-classifying regex. Result: **fixing Bug 1 is NOT enough.**

With the correct plan, gold enters the large pool (255–302 candidates, all with
vector/bm25) but still ranks outside top-10:

| qid | pool | gold ranks | gold in top-10 |
|---|---|---|---|
| q29 | 302 | 12, 117, 186, 261, 276 | 0 |
| q35 | 257 | 6, 14, 29, … | 1 |
| q36 | 286 | 160, 191, 233 | 0 |
| q21 | 255 | 22, 48, 65, … | 0 |
| q9  | 255 | 22, 48, 49, … | 0 |
| q2  | 194 | 14, 21, 32, … | 0 |

**The decisive finding:** for q29, the episode that *literally answers the
question* — "Their names are Pepper, Precious and Panda" — is **NOT in the
302-candidate pool at all**. Its vector similarity to "What are the names of
Audrey's dogs" ranks it below 302. Generic topic-matches ("Audrey bought toys
for her dogs", "Pixie adjusted well") outrank the exact answer.

**Root cause is the bi-encoder embedding (Bug 5, new):** a declarative
statement ("their names are X, Y, Z") has low cosine similarity to the
interrogative ("what are the names?"). The bi-encoder matches topical similarity
("dogs / Audrey"), not answerhood. This is a known asymmetry weakness of
dual-encoder retrieval. BM25 should rescue it (the answer contains the exact
rare tokens), but Bug 4 (common-token domination + `max(vec,bm25)` aggregate)
disables the lexical channel.

**Revised priority: Bug 4 (BM25) is the highest-leverage fix, not Bug 1.**
No plan choice or pool size helps if the answer-bearing episode's vector rank is
below the pool cutoff — the lexical channel is the only path that can surface it,
and it is broken. Bug 1 alone just changes the failure mode ("not in pool" →
"in pool but un-rankable").

Also observed: every candidate's fused `score` was `0.000` in this run
(factual_reweight=off) — sort fell back to vector tiebreak. Worth checking the
default-fusion scoring path produces non-zero composite scores.

## Proposed fixes (ordered by leverage / cost) — REVISED

1. **Bug 1 (cheap, high-leverage):** tighten `abstract_regex` to the design's
   `what (has|have)` only — drop `are | were | did | am i`. This restores the
   single-Factual path for enumeration questions. **First experiment:** force
   q21/q29/q35/q36 to Factual and confirm gold recalls.
2. **Bug 4 (high-leverage):** stop letting common tokens dominate. Options:
   IDF-weight / require rare-token matches, drop stopwords from the FTS query,
   or use AND on the discriminating terms. Make BM25 actually surface
   exact-phrase gold.
3. **Bug 3:** revisit top-10 truncation vs. fused score distribution — gold at
   0.73 should beat generic 0.83 only if Bug 4 is fixed (BM25 lifts the
   discriminating rows). Likely resolved as a consequence of fix 2; verify.
4. **Bug 2 (deeper):** make the Hybrid Factual sub-plan run the same
   vector+BM25 recall as the single Factual plan, so Hybrid ≥ its parts.
   Lower priority if Bug 1 removes the misroute, but it is a latent correctness
   bug for genuinely-hybrid queries.

## Acceptance criteria

- [ ] AC-1: `abstract_regex` matches design §3.2 (`what (has|have)` only); the
      doc comment and code agree. Unit test asserts "What are the names…" does
      NOT score abstract = 1.0.
- [ ] AC-2: conv-44 q21/q29/q35/q36 classify as Factual (not Hybrid) after fix 1.
- [ ] AC-3: BM25 surfaces exact-phrase gold — for q9, ≥1 of the
      "dog training course" / "positive reinforcement" rows gets bm25 > 0 and
      enters top-10 after fix 2.
- [ ] AC-4: conv-44 list-SF questions (q2/q9/q21/q29/q35/q36) — measure recall
      lift; target ≥ +3/6 moving off "I don't know".
- [ ] AC-5: Hybrid Factual sub-plan carries vector/bm25 sub-scores (fix 4),
      verified via prefusion dump showing non-None scores.

## Artifacts

- Probe run: `benchmarks/runs/ISS189-conv44-dump-20260529T065211Z/`
- Pool dumps: `/tmp/iss189-dump/conv-44-q{29,35,36,21,9,2}-{prefusion-,}*.jsonl`
- conv-44 baseline: `benchmarks/runs/CONV44-baseline-20260529T060701Z/`

## UPDATE 2026-05-29 (2): DEEPEST root cause — Factual recall is graph-entity-mention bound

Read `plans/factual.rs` + `adapters/graph_entity_resolver.rs`. The Factual plan
candidate pool is NOT vector-top-N. Per design §4.1 it is built purely by graph
traversal:

```
query → extract_mentions → ["Audrey","dogs",...]
      → search_candidates (EXACT-equality on graph_entity_aliases.normalized)
      → resolve anchor "Audrey"
      → memories_mentioning_entity("Audrey")  ← the ENTIRE candidate pool
```

vector/bm25 only *re-score* this graph-sourced pool; they cannot add candidates
the graph traversal missed. So q29's answer episode "Their names are Pepper,
Precious and Panda" can only enter the pool if the **extractor wrote an
`Audrey`-mention edge on that episode**. That sentence's surface form is "Their
names are…" — no literal "Audrey" token, the referent is anaphoric ("their" →
Audrey's dogs). LoCoMo dialogue is full of this. If extraction/coref didn't
attach the Audrey entity to that episode, it is **structurally unreachable** by
the Factual/Hybrid path, regardless of plan choice, pool size, BM25 fix, or
reranker.

This is the same systemic defect as ISS-145 (GraphEntityResolver blindness) and
ISS-149 (classifier death): v0.3's Factual + Hybrid paths are gated on a graph
entity layer whose extraction recall is poor on conversational, anaphoric text.

### Revised root-cause ranking (deepest first)

1. **Bug 6 (deepest): Factual/Hybrid recall is entity-mention-gated.** Answer
   episodes without an extracted entity-mention edge are unreachable. Coref /
   anaphora ("their", "they", "she") drops the subject, so declarative answers
   to "what are X's ..." questions routinely lack the gating entity edge.
2. **Bug 5: bi-encoder answerhood asymmetry** — even on a vector path,
   declarative answers rank below topical chatter (q29 answer not in vector
   top-302).
3. **Bug 4: BM25 lexical channel disabled** — common-token domination +
   `max(vec,bm25)` aggregate; the one channel that could match rare answer
   tokens is off.
4. **Bug 1/2/3:** classifier misroute, crippled Hybrid sub-plan, top-10
   truncation — all real, all secondary.

### Implication for fixes

Fixing BM25 (Bug 4) only helps episodes that are *already in the pool*. For the
Factual path, the pool is entity-gated, so Bug 4 helps the q9-class
("in pool, mis-ranked") but NOT the q29-class ("not in pool"). The q29-class
needs either:
- (a) a recall path that is NOT entity-mention-gated (e.g. a true vector/BM25
  candidate-generation channel that unions into the Factual pool), or
- (b) better extraction/coref so anaphoric answer episodes get the entity edge.

Recommend (a) as the architecturally cleaner fix: add lexical+vector
candidate *generation* (not just re-scoring) to the Factual pool, so answer
episodes can enter even without a graph entity edge. This also subsumes Bug 4's
benefit.
