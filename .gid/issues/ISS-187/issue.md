---
title: Pipeline candidate-survival audit — log gold rank at each stage for 19 A-bucket queries
category: retrieval-foundation
discovered_in: ISS-186 resolved 2026-05-28 — 19/32 SH queries have gold in top-10 of pure bi-encoder probe but conv-26 SF scores 5-8/27. Gold is being dropped between Stage B (execute_plan) and Stage D (top-k truncate).
priority: P0
severity: diagnostic
status: resolved
relates:
- engram:ISS-186
- engram:ISS-148
- engram:ISS-149
- engram:ISS-159
- engram:ISS-164
- engram:ISS-175
- engram:ISS-178
- engram:ISS-179
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
   Hybrid plan path is included — it also flows through this hook
   before bypassing fuse_and_rank.
3. **No engram-bench code change.** The existing `LocomoDriver`
   already wraps each query with `set_dump_label(qid)` /
   `clear_dump_label()` around `graph_query_locked`. Setting
   `ENGRAM_DUMP_FUSED_POOL_DIR=<stamped dir>` and
   `ENGRAM_DUMP_FUSED_POOL_QIDS=<32 conv-26 SH qids>` is all
   that's needed for the bench to produce both pre-fusion and
   post-fusion JSONL per query.
4. **/tmp/iss187_run.sh** — sweep wrapper. Sets locked envelope
   (FACTUAL_REWEIGHT=off ENTITY_CHANNEL=off PIPELINE_POOL=1
   WORKERS=4 K=10) + `ENGRAM_DUMP_FUSED_POOL_DIR` +
   `ENGRAM_DUMP_FUSED_POOL_QIDS`, launches `engram-bench locomo
   conv-26`. Stamped output dir under
   `.gid/issues/ISS-187/artifacts/`.
5. **/tmp/iss187_analyse.py** — joins `prefusion-*.jsonl`,
   `<qid>-*.jsonl` (fused), and the bench's `locomo_per_query.jsonl`
   (final stage D). For each of the 19 A-bucket queries computes
   the four drop-point buckets above. Emits a markdown table +
   aggregate counts. Output committed to
   `.gid/issues/ISS-187/artifacts/conv26-drops-<STAMP>.md`.

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
- [x] AC-4: Engram-bench produces both `<qid>-<intent>.jsonl`
  (fused / ISS-175) and `<qid>-prefusion-<intent>.jsonl`
  (ISS-187) under one run when `ENGRAM_DUMP_FUSED_POOL_DIR` is
  set. No new example needed — the existing LocomoDriver already
  calls `set_dump_label(qid)` around `graph_query_locked`, so
  the api.rs hook fires automatically. Verified by a sweep
  script that launches the bench with the 32 conv-26 SH qids
  whitelisted via `ENGRAM_DUMP_FUSED_POOL_QIDS`, runs in <20min
  wall on the locked ISS-178 Arm A envelope.
- [x] AC-5: `iss187_analyse.py` produces the four-drop-point
  bucketing for the 19 A-bucket queries identified in ISS-186.
  Output committed to `.gid/issues/ISS-187/artifacts/`
  (`conv26-drops-20260529T011704Z.md`). NOTE: analyse script had a
  schema bug (assumed nested `r["query"]["id"]`/`gold_answer`; actual
  per_query schema is flat `r["id"]`/`r["gold"]`) that produced an
  all-drop_AB false verdict on first run — fixed before recording.
- [x] AC-6: Decision section picks ISS-188 (populate factual-plan
  candidate embeddings so MMR diversity works on list-questions),
  citing q18 worked example + drop_CD 22/32 distribution.

## Status

Resolved 2026-05-29 — all 6 ACs met. Diagnostic complete: drop_CD
dominates, root cause = factual-plan candidates carry no embedding so
MMR diversity is inert. Next attack surface = ISS-188 (embedding
population for diversity reranking on list-questions).

## Decision (2026-05-29, STAMP=20260529T011704Z)

**Verdict: `drop_CD` dominates (22/32 full SH set), and within the
13 SF-subset drop_CD queries, 10/13 are LIST-type gold scoring 0.**
This satisfies the `drop_CD ≥ 8/19` branch — but the *root cause* is
deeper than "K_seed too small". The data names a specific structural
defect: **MMR's diversity channel is dead on the factual plan.**

### Evidence chain (q18 worked example)

q18 gold = `"beach, mountains, forest"` (3 camping locations).
Final predicted = `"In the mountains"` → score 0.0 (judge: incomplete,
missing beach + forest).

- Fusion pool (Stage C) = 186 candidates.
- top-6 of fusion are ALL mountains/forest memories (cosine clusters
  hard on the query's "camping" signal).
- `beach` memories ARE in the pool but ranked **C#38 / C#46 / C#152**.
- top-10 truncate (Stage D) → LLM sees only mountains/forest → partial
  answer → judge 0.

The gold is retrieved (pool has it) and well-clustered for ONE list
item, but the other items are pushed to rank 38-152 by pure relevance
ranking. **This is exactly the problem MMR/diversity is designed to
fix** — break the redundant top cluster, surface the
relevant-but-distant items before truncation.

### Why every retrieval lever since ISS-159 falsified

`crates/engramai/src/retrieval/fusion/mmr.rs` module docs (lines
58-70) state: candidates with `embedding: None` get **0 diversity
penalty**, so on plans with no populated embeddings (factual /
episodic) **MMR degenerates to a no-op**. The factual plan — which
all 10 failing list-questions route through — carries
`embedding: None` (confirmed: q18 prefusion scores are all 0.0).

Therefore:
- ISS-139 MMR λ-sweep saw no signal on factual plan (MMR was inert there).
- ISS-159 (cross-encoder), ISS-164 (entity channel), ISS-175 (factual
  reweight), ISS-178 (prev-turn) all tuned **single-point relevance**.
  List-questions don't lack relevance — they lack **coverage/diversity**,
  and the diversity channel was structurally dead.

### Distribution (32 conv-26 single-hop)

- drop_CD: 22/32 (gold in pool, lost at top-10 truncate)
- drop_AB: 7/32 (4 are pool=10 non-factual routes; only q40/q23/q75
  are true bi-encoder misses — numeric "2", book titles)
- drop_BC: 1/32 (q3)
- SF-subset drop_CD: 13 queries, **10/13 LIST-type, all score ~0**

### Follow-up issue (root fix, NOT K_seed bump)

File **ISS-188: populate candidate embeddings on the factual (and
episodic) plan so MMR can perform diversity reranking on
list-questions.** Implements the "Future work" already noted in
mmr.rs:73-74 (opt-in Storage-backed `get_embedding` fallback per
candidate). Then re-run the ISS-139 λ-sweep **on the 10 LIST-type SF
queries specifically** (not the diluted full conv-26 set) to find the
λ that maximizes list coverage without regressing single-value SF.

Cheap K_seed bump is explicitly REJECTED as the primary fix: the gold
is already in a 186-deep pool, so widening the pool doesn't help —
the problem is ordering within the pool, which only diversity
reranking addresses. K bump only helps if we also feed >10 candidates
to the LLM, which inflates context cost without fixing the ordering
defect.
