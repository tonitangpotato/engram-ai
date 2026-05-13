# RUN-0026 Substrate Evidence — clustering collapse confirmed

**Date:** 2026-05-06
**Source:** RUN-0026 substrate.db inspected directly (path:
`/var/folders/.../tmp7RP9O3/substrate.db`, the in-flight DB the bench
driver wrote during the ISS-106-patched run).
**Verifier:** potato (manual sqlite queries)

---

## Raw findings (cite-before-claim)

| Table | Row count |
|---|---|
| `memories` | 452 |
| `knowledge_topics` | **1** |

The single topic:

- **title**: `"Caroline and Melanie's Journey of Self-Discovery and Mutual Support"`
- **source_memories**: 441 of 452 memories (97.6%)
- **contributing_entities**: 0

## What this proves

1. The ISS-106 patch *runs* — `compile_knowledge` is called and
   produces output. Patch is not silently skipping work.
2. `EmbeddingInfomapClusterer` collapses the 452-memory single-conv
   substrate into one super-cluster.
3. Entity extraction is also failing on this corpus
   (contributing_entities=0).

## What this disproves (my earlier wrong claims, recorded for honesty)

- ❌ "Abstract sub-plan still downgrades l5_unavailable on every
   call." Wrong — substrate has 1 topic, Abstract returns it as a
   candidate every time.
- ❌ "Topic summaries are leaking into non-Abstract retrieval and
   crowding out raw memories." Wrong direction — there's only one
   topic, and the contamination is via the Abstract sub-plan
   contributing it to fuse, not via topic summaries leaking
   sideways.
- ❌ Stating both of the above with confident voice instead of as
   hypotheses. Cite-before-claim violation; documented for future-me
   not to repeat.

## Mechanism (corrected)

1. `compile_knowledge` runs after ingest (ISS-106 patch).
2. Clusterer with `similarity_threshold = 0.5` produces 1 cluster
   covering 441/452 memories.
3. LLM summarizer compresses the cluster into one universal title.
4. Every Abstract-sub-plan query semantic-matches this title (because
   it's about "everything in conv-26").
5. Fuse stage promotes the super-topic → it occupies a top-K slot
   that previously held a specific Factual / Episodic memory.
6. Net per-query effect: −33 queries flip correct → wrong across all
   four LoCoMo categories, J-score drops 0.559 → 0.342.

## Decision (2026-05-06, after potato review)

- ✅ **Revert ISS-106 patch.** Done in
  `engram-bench/src/drivers/locomo.rs` — replaced the
  `memory.compile_knowledge("default")` call with a NOTE block citing
  this evidence file. `cargo check` passes.
- ✅ **Mark ISS-106 status = blocked**, add label
  `blocked-by-clustering`.
- ✅ **Open ISS-107** (knowledge clustering collapses on
  single-conversation corpora). Root-cause issue, owns the fix.
- ✅ **Open ISS-108** (micro-benchmark) as prerequisite for
  ISS-107 — fixes verified against micro-bench *before* re-running
  LoCoMo, so we don't tune to the conv-26 degenerate case.
- ❌ **Did NOT pursue** the cheap path (re-run with
  `similarity_threshold = 0.75 / 0.85`). Reason: that's
  benchmark-driven tuning. Without a non-LoCoMo verification
  (ISS-108), a "good" threshold is just LoCoMo overfit.
- ❌ **Did NOT pursue** defensive gating in Abstract sub-plan
  (`source_memories / total > 50% → skip`). Reason: patch-not-fix.
  Symptom suppression, would silently misbehave on legitimately
  large topics in production.

## What is *not* fixed by the revert

- LoCoMo's Abstract sub-plan is back to returning
  `DowngradedL5Unavailable` for every query. RUN-0024 baseline
  (J=0.559) is the ceiling under that regime. Real upside from a
  working Abstract is unmeasured and currently unreachable.
- ISS-107 must land before that upside is recoverable.

## Lessons (for future-me)

1. **Always read substrate.db before declaring a mechanism.** Internal
   "I remember this" is unreliable, especially across long sessions
   on retrieval mechanics.
2. **Distinguish "patch ran" from "patch fixed".** RUN-0025 vs
   RUN-0026 isolation showed the patch alone regresses — needed
   that to even know what to look for.
3. **Cite-before-claim covers your own forensic theories.** Saying
   "Abstract sub-plan still downgrades" without checking
   `knowledge_topics` count = same failure as quoting wrong issue
   IDs. Hypothesis voice ("I think") is fine; assertion voice
   without verification is not.
4. **Tune-to-bench is overfit if the bench has a single data
   shape.** This is the lesson that produced ISS-108.
