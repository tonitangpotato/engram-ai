---
title: Pipeline candidate-survival audit — log gold rank at each stage for 19 A-bucket queries
category: retrieval-foundation
discovered_in: ISS-186 resolved 2026-05-28 — 19/32 SH queries have gold in top-10 of pure bi-encoder probe but conv-26 SF scores 5-8/27. Gold is being dropped between Stage B (execute_plan) and Stage D (top-k truncate).
priority: P0
severity: diagnostic
status: in_progress
relates: [engram:ISS-186, engram:ISS-148, engram:ISS-149, engram:ISS-159, engram:ISS-164, engram:ISS-175, engram:ISS-178, engram:ISS-179]
---

## Why this issue exists

ISS-186 settled the candidate-pool question: for 19/32 conv-26
single-hop queries, the gold memory sits within rank 10 of a pure
bi-encoder cosine probe over the full substrate. Yet the production
pipeline scores only 5-8/27 SF. The gold IS being retrieved by the
bi-encoder; **something in the plan-classifier → channel-fusion →
MMR → truncate pipeline drops it before the LLM judge sees it**.

This issue does not redesign anything. It instruments the pipeline
to name **which stage** drops the gold candidate for each
A-bucket query. Without that data, every future ranker/fusion
issue is guessing.

## What we need to see

For each of the 19 A-bucket queries, log the position of the gold
memory_id at four checkpoints:

```
Stage A  classifier:    chosen plan_kind + confidence
Stage B  execute_plan:  raw candidate list (pre-fusion, post-channel)
                        -> rank of gold_memory_id, or "absent"
Stage C  fuse_and_rank: post-fusion, post-sort, pre-truncate
                        -> rank of gold_memory_id  (already covered
                           by ISS-175 maybe_dump_fused_pool)
Stage D  top_k truncate: final returned list
                        -> rank of gold_memory_id, or "absent"
```

The gap that matters is **B vs C vs D**:

- **Gold present at B, absent at C** → fusion weights drop it
  (weights wrong, or wrong channel scored it low)
- **Gold present at C above K, absent at D** → top_k truncate
  drops it (need wider K_seed or post-fusion reranker)
- **Gold present at C below K, absent at D** → fusion ranked it
  below K_seed in the wrong order (reranker / MMR question)
- **Gold absent at B already** → plan classifier picked the wrong
  plan_kind, channels didn't retrieve it (plan routing bug, not
  a fusion bug)

This four-way split is what every "lever falsified" post-mortem
has been blind to since ISS-159.

## Method

**No new code paths. Just logging.** Add a Stage-B candidate dump
that mirrors the existing ISS-175 Stage-C dump (`fusion/dump.rs`
`maybe_dump_fused_pool`). Both stages share the same env-gated
infrastructure (`ENGRAM_DUMP_FUSED_POOL_DIR` directory,
`ENGRAM_DUMP_FUSED_POOL_QIDS` whitelist, `set_dump_label` per
query) so the operator runs **one** bench with one env var and gets
two JSONL files per query: `<qid>-prefusion-<intent>.jsonl` and
`<qid>-fused-<intent>.jsonl`.

The analyse script reads both, joins them on `memory_id`, and
produces a per-query four-row table:

```
qid       plan  stage_B_rank  stage_C_rank  stage_D_rank  gold_id
q3        Fact  4             8             —             m_2626c
q4        Hyb   12            —             —             m_8a1bc
q13       Fact  1             1             1             m_xxxx
...
```

Then bucket the 19 A-bucket queries by drop point:
- **drop_AB**: present in bi-encoder, absent at Stage B   → routing
- **drop_BC**: present at B, absent at C                  → fusion
- **drop_CD**: present at C, absent at D (rank > K_seed)  → truncate
- **drop_none**: present at D                             → wins

Whichever drop bucket dominates names the next attack surface.

## Approach (concrete plan)

1. **engramai/src/retrieval/fusion/dump.rs** — generalize the dump
   API: keep `maybe_dump_fused_pool(intent, candidates)` working
   byte-identically (don't break ISS-175 artifact filenames), but
   add a sibling `maybe_dump_prefusion_pool(intent, candidates)`
   that writes `<label>-prefusion-<intent>.jsonl` under the same
   `ENGRAM_DUMP_FUSED_POOL_DIR` env var. Same qid whitelist. Same
   project_row schema (no new fields).
2. **engramai/src/retrieval/api.rs** — at the existing call site
   immediately before `fuse_and_rank` (api.rs:846 area), insert
   `crate::retrieval::fusion::dump::maybe_dump_prefusion_pool(intent, &candidates);`.
   One line. Hot path stays single env-var read when disabled.
3. **engram-bench/examples/iss187_pipeline_audit.rs** — new
   example. Ingests conv-26 with the locked ISS-178 Arm A envelope
   (FACTUAL_REWEIGHT=off ENTITY_CHANNEL=off PIPELINE_POOL=1
   WORKERS=4). Sets `ENGRAM_DUMP_FUSED_POOL_DIR` to a stamped
   output dir. For each of the 32 conv-26 single-hop queries: sets
   `set_dump_label(qid)`, runs `Memory::recall(question)`, captures
   final returned ids. Emits one summary JSONL row per query with
   `qid`, `gold_id`, `gold_text_match`, `stage_D_rank` (from the
   returned ids), plus pointers to the two dump files.
4. **/tmp/iss187_analyse.py** — joins `prefusion-*.jsonl`,
   `fused-*.jsonl`, and the summary file. For each query computes
   the four drop-point buckets above. Emits a markdown table +
   aggregate counts.

## Decision rule

Aggregate the 19 A-bucket queries (from ISS-186) by drop point:

- **drop_AB ≥ 8/19 (~42%)** → plan classifier is the lever.
  File ISS for classifier rework (route SF queries through a plan
  that actually retrieves their candidate type).
- **drop_BC ≥ 8/19** → fusion is the lever. File ISS for fusion
  weight tuning **with concrete gradient evidence** (not guess-and-bench).
- **drop_CD ≥ 8/19** → K_seed too small or sort order is wrong.
  Cheap fix: bump K_seed. Park reranker work.
- **distributed (no bucket ≥ 8/19)** → the gap is structural, not
  single-stage. Escalate to v0.4 redesign discussion.
- **drop_none ≥ 5/19 within the A-bucket** → bench harness or LLM
  judge is rejecting candidates the pipeline already delivered.
  Falsifies "pipeline drops it" — points at engram-bench / judge.

## Acceptance criteria

- [x] AC-1: `maybe_dump_prefusion_pool` lands in
  `engramai/src/retrieval/fusion/dump.rs` with unit test
  `prefusion_dump_writes_jsonl_when_enabled` and contract test
  `prefusion_dump_noop_when_disabled` proving zero allocation /
  zero file writes when env var unset.
- [x] AC-2: Stage-B dump hook lands in `api.rs` at the
  pre-fuse_and_rank call site. Hybrid plan path (which bypasses
  fuse_and_rank) gets its own dump call with intent=Hybrid so
  Hybrid queries also produce a prefusion file.
- [x] AC-3: ISS-175 fused-pool dump filename and schema unchanged
  (`<label>-<intent>.jsonl`). Regression test pins the existing
  filename pattern.
- [ ] AC-4: `engram-bench/examples/iss187_pipeline_audit.rs`
  builds clean (default features, no cross_encoder feature
  required) and runs over conv-26 in <20min wall.
- [ ] AC-5: `iss187_analyse.py` produces the four-drop-point
  bucketing for the 19 A-bucket queries identified in ISS-186.
  Output committed to `.gid/issues/ISS-187/artifacts/`.
- [ ] AC-6: Decision section in this issue body picks ONE
  follow-up issue (classifier / fusion / K_seed / structural /
  harness) based on the bucket counts, citing per-query data.

## Status

In progress 2026-05-28 — issue opened, implementation plan locked,
starting on AC-1 (dump.rs generalisation).
