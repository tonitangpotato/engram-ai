---
title: "Dedup at cosine ≥0.95 is exact-match dedup, not Mem0-style semantic UPDATE"
status: open
priority: P1
severity: architecture-gap
category: ingestion
created: 2026-05-26
relates:
  - ISS-148
  - ISS-161
  - ISS-162
  - ISS-164
discovered_in: ISS-161 root-cause audit 2026-05-26
---

## Summary

Engramai's dedup pass triggers only at cosine ≥ 0.95. This is
near-exact-match dedup, not the semantic-reconcile UPDATE phase that
Mem0 and other production memory systems use. Below 0.95, two
versions of the same fact persist as independent memories — even
when one is strictly more specific than the other ("Caroline went
to research" + "Caroline researched adoption agencies"). The vague
earlier version dilutes retrieval scoring against the specific later
version.

This is the **second largest** contributor to ISS-148 AC-5a
single-fact failures, especially for cross-session questions where
the most-specific answer appears later in the conversation than the
first mention (q11 Sweden, q3 adoption agencies after extractor
fix).

## Code-layer evidence

Verified 2026-05-26 against current working tree (commit `5adf83e`):

1. **Dedup threshold = 0.95** —
   `crates/engramai/src/config.rs:351`:
   ```rust
   fn default_dedup_threshold() -> f64 {
       0.95
   }
   ```

2. **Dedup is binary merge-or-create** —
   `crates/engramai/src/memory.rs:2820-2880` (Phase A: entity-aware
   match) and 2884-2925 (Phase B: embedding-only). Both code paths
   are:
   ```
   if cosine(new, existing) >= 0.95:
       merge into existing (entity links updated, merge_count++)
   else:
       create new memory
   ```

   There is no third branch for "0.80-0.95 → LLM reconciliation".

3. **No semantic conflict resolution** — `grep -rn "ADD.*UPDATE.*DELETE
   \|semantic_reconcile\|llm_dedup\|update_phase" crates/engramai/src/`
   returns zero hits in non-test code.

4. **Consolidation cycle is ACT-R activation decay, not semantic
   merge** — `crates/engramai/src/models/consolidation.rs:80
   run_consolidation_cycle` does (a) working-layer consolidation
   per ACT-R, (b) interleaved archive replay, (c) Hebbian decay,
   (d) layer rebalancing. None of these touch content-level
   reconciliation.

## Comparable systems

**Mem0** UPDATE phase (Chhikara et al., arXiv:2504.19413, §2.1):

For each new extracted memory $m_{new}$:
1. Retrieve top-K similar existing memories (default K=10) by
   embedding cosine.
2. LLM (default GPT-4o-mini, in our case Haiku) sees $m_{new}$ +
   the K candidates and emits one of four actions per candidate:
   - `ADD` — net-new fact, keep both
   - `UPDATE` — new fact supersedes/refines old; replace old's
     content with merged version
   - `DELETE` — new fact contradicts old; mark old as superseded
   - `NOOP` — duplicate, drop new
3. Apply decisions transactionally.

This catches the cosine 0.80-0.95 band where two memories are
semantically related but not embedding-identical:
- "Caroline went to research" (vague) + "Caroline researched
  adoption agencies" (specific) → UPDATE: vague replaced
- "Caroline lives in NYC" + "Caroline moved to Brooklyn" → both
  kept (subsumption, ADD)
- "Caroline likes coffee" + "Caroline switched to tea" → DELETE
  the old, ADD the new

**Zep** uses a similar but graph-rooted reconciliation: each new
fact is checked against entity-anchored existing edges before
insertion, with explicit invalidation of contradicted prior edges.

## Concrete failure example (conv-26 q3, post-ISS-162)

Assume ISS-162 ships and the extractor sees `(prev, curr)`. The
two memories extracted from the relevant conv-26 exchange become:

- M1 (from Melanie's question + Caroline's first reply):
  `"Caroline is researching adoption agencies"`

Later in conv-26, Caroline says:
> "I've been looking into private agencies vs. agencies that work
> with foster care ..."

Without ISS-162's `(prev, curr)` context, this turn extracts as:

- M2: `"Caroline is comparing private and foster-care agencies"`

With ISS-162 context, this turn extracts as:

- M2': `"Caroline is comparing private adoption agencies and
  foster-care adoption agencies"` (noun "adoption agencies"
  inherited from the rolling summary).

Cosine(M1, M2') is plausibly 0.85-0.92 — semantically the same
topic, lexically overlapping. At our 0.95 threshold, both persist.
Retrieval for "What did Caroline research?" returns both with
similar scores; the LLM judge sometimes picks M2' (which is more
specific but doesn't contain "adoption agencies" as the answer
phrase) over M1.

With Mem0-style UPDATE, M2' supersedes M1; the consolidated memory
content is the union of both ("Caroline is researching adoption
agencies, comparing private vs. foster-care variants"). Retrieval
returns one memory containing the gold phrase, ranked first.

## Acceptance criteria

**AC-1 (mechanism)**: Implement a post-extraction UPDATE phase. For
each newly extracted memory $m_{new}$:
- Retrieve top-K = 5 candidates from the same namespace with cosine
  in `[0.80, 0.95)` (below 0.80 → too unrelated; ≥ 0.95 → already
  handled by exact dedup).
- Single LLM call (Haiku temp=0) emits an action per candidate from
  `{ADD, UPDATE, DELETE, NOOP}`.
- Apply atomically inside the existing `store_raw` transaction.

**AC-2 (config knob)**: Behaviour is opt-in via
`MemoryConfig.semantic_update_enabled: bool` (default `false` until
benched). When false, current 0.95-exact-merge behaviour is
preserved byte-for-byte.

**AC-3 (observability)**: Emit a `StoreEvent::SemanticUpdate {
action, source_id, target_id, similarity }` for each non-NOOP
decision. Count `semantic_update_applied` per namespace in
write_stats.

**AC-4 (cost ceiling)**: Per-ingest LLM overhead must be bounded.
Skip UPDATE phase when fewer than 2 candidates are in the
[0.80, 0.95) band — this matches Mem0's "no related memories →
skip LLM call" optimisation. Target: ≤30% of ingests incur the
extra LLM call, with budget ≤ 200 input tokens + ≤ 100 output
tokens per call.

**AC-5 (LoCoMo measurement)**: Re-run conv-26 K=10 temp=0 HyDE=off
with `semantic_update_enabled = true` (and ISS-162 shipped). Target:
single-fact sub-bucket ≥ 13/27 = 0.48 (additional +2 questions on
top of ISS-162's projected 11/27).

**AC-6 (no regression)**: Multi-hop bucket on conv-26 must not
regress more than 2 questions. Cross-validate on conv-44 within
±2pp on overall.

## Out of scope

- **Streaming UPDATE during ongoing session** — first pass is
  per-ingest. A future optimisation could batch UPDATE at
  session-end (Zep paradigm).
- **Contradiction detection across sessions** — handled by the
  basic DELETE action but full cross-session contradiction graph
  is a separate, larger ISS.
- **The extractor input shape** — ISS-162.
- **Entity-aware retrieval routing** — ISS-164.

## Estimated effort

1-2 weeks. The candidate-fetch (top-K cosine in
`[0.80, 0.95)`) is one new SQL query against `embeddings` table. The
LLM call piggybacks on the existing Anthropic client. The mutation
logic (UPDATE = content rewrite + merge_count++, DELETE = soft-delete
flag) is small. Most of the time is in the LoCoMo bench + tuning the
LLM prompt that picks actions.

## Expected lift

Per ISS-161 audit: 2-3 of 9 currently-missing single-fact questions
(specifically q3, q11, q43 after ISS-162) involve a more-specific
later version of the same fact. After ISS-162 establishes context,
ISS-163 reconciles the multiple extracted versions. Combined
projection: 13/27 = 0.48 (still below AC-5a 0.60, but the third
lever ISS-164 + cross-encoder ISS-159 fill the remaining gap).
