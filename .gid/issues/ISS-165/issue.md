---
title: 'GraphEntityResolver lacks mention extraction: returns 0 anchors for any natural-language query'
status: open
priority: P1
severity: root-cause-confirmed
category: retrieval
created: 2026-05-26
relates:
- ISS-164
- ISS-148
- ISS-161
- ISS-162
- ISS-163
- ISS-166
discovered_in: ISS-164 Phase 2 A/B sweep 2026-05-26
blocked_by: ''
---

## Summary

The ISS-164 always-on entity channel — wired into the Associative
plan via Step 2b (inject ResolvedAnchors into `seed_entities`) and
Step 3b' (direct `memories_mentioning_entity` on each anchor) —
**produced zero single-fact lift and a −10.81pp multi-hop
regression** in a clean A/B sweep on LoCoMo conv-26.

The hypothesis behind ISS-164 — that surfacing entity-resolver
anchors directly into the Associative pipeline would close the
documented 0/152 Factual-dispatch gap and unlock ISS-148 AC-5a — is
**falsified for conv-26 single-fact** by this evidence.

`FusionConfig::locked().entity_channel_enabled = false` was kept,
so the regression is **inert in production**. The wiring code from
ISS-164 (commits 77ef3f3 + ebc9adf) is retained intact so this
issue's root-cause investigation can use the existing
instrumentation; the locked default keeps every non-opt-in caller
on the byte-identical pre-ISS-164 §4.3 pipeline.

## Bench evidence

Sweep run: `/tmp/iss164_bench_sweep.sh` STAMP `20260526T213218Z`.
Two arms on conv-26, K=10, temp=0, HyDE=off, MMR=off (locked),
cross-encoder=off, force-intent=off. Single-axis A/B isolating
`ENGRAM_BENCH_ENTITY_CHANNEL`.

Output dirs:
- `engram-bench/benchmarks/runs/ISS164-A-conv26-20260526T213218Z`
- `engram-bench/benchmarks/runs/ISS164-B-conv26-20260526T213218Z`

| Metric                  | A (off)    | B (on)     | Δ            |
|-------------------------|------------|------------|--------------|
| **overall**             | 0.3947     | 0.3618     | **−3.29pp**  |
| **single-fact (sub)**   | **3 / 12** | **3 / 12** | **0**        |
| list (sub)              | 3 / 20     | 5 / 20     | +2           |
| single-hop (agg)        | 6 / 32     | 8 / 32     | +2 (= list)  |
| **multi-hop**           | 17 / 37    | 13 / 37    | **−10.81pp** |
| temporal                | 33 / 70    | 30 / 70    | −4.29pp      |
| open-domain             | 4 / 13     | 4 / 13     | 0            |

Per-question single-fact: the same three questions pass in both
arms (q4 "Transgender woman", q13 "counseling", q55 "Sunsets")
and the same nine fail. Zero flips on the target metric.

Cross-bench sanity: Arm A single-fact 3/12 matches ISS-161 Arm A
exactly (same three IDs pass) — confirms `entity_channel=off` is
byte-identical to the ISS-161 baseline path, so the regression in
Arm B is genuinely caused by the entity channel and not by an
unrelated envelope drift.

## Code-layer hypotheses (need verification)

The Phase-1 implementation (associative.rs:418–451 Step 2b +
:524–566 Step 3b') has three ways to produce the observed
"zero-lift + multi-hop regression" profile. **No claim is
verified — these are starting points for the root-cause probe.**

### Hypothesis 1: resolver picks the wrong anchor

`GraphEntityResolver::resolve(query.text)` may return anchors whose
`entity_id` does not correspond to the gold-fact's mentioned
entity. For the nine failing single-fact queries (e.g. q3 "what
agencies has Caroline contacted for adoption?", gold "Adoption
agencies") the resolver would need to surface an "adoption
agency"-class entity for the channel to help. If the resolver
instead surfaces "Caroline" (a high-frequency person entity), the
returned `memories_mentioning_entity(caroline)` flood is
indistinguishable from the seed_recaller's existing output —
hence zero single-fact lift.

**Probe**: dump `query → resolved_anchors[]` for the nine failing
queries via a one-shot debug run. Compare against the gold-fact
memory's `entity_mentions` set.

### Hypothesis 2: anchor injection corrupts Step 3 fan-out

Step 3 in `associative.rs:460–495` iterates `seed_entities` and
calls `graph.edges_of(subj_entity)` to discover 1-hop neighbors.
Pre-ISS-164, `seed_entities` was populated only by Step 2 from the
seed memories' `entity_mentions`. Post-ISS-164, anchors are
**unioned into the same HashMap** before Step 3 runs. If the
injected anchor is a high-degree entity (a person like Caroline
with hundreds of outgoing edges), Step 3 spends its budget
expanding from the noisy anchor and short-circuits before
expanding from the **correct** seed-derived entities.

This would explain the multi-hop −10.81pp regression: multi-hop
questions depend on the 2-hop reasoning chain that Step 3
discovers; if Step 3 burns its budget on the wrong starting
point, the chain never surfaces.

**Probe**: instrument Step 3 to log `(subj_entity, edges_returned,
budget_remaining)` for both arms on the same 37 multi-hop
questions. Look for budget exhaustion on Arm B that doesn't happen
on Arm A.

### Hypothesis 3: Step 3b' direct anchor recovery doesn't help here

The conv-26 gold-fact memories may not mention the resolved
anchor entity directly. If gold-fact for q3 is "Caroline contacted
Lighthouse Adoption Agency" and the resolver surfaces only
"Caroline" (not "Lighthouse Adoption Agency"),
`memories_mentioning_entity(caroline, 16)` returns the same 16
Caroline-tagged memories the seed recall already returned. The
"direct anchor → memory" channel adds zero new candidates.

This would explain zero single-fact lift independently of
hypothesis 2.

**Probe**: for each of the nine failing queries, compute
`memories_mentioning_entity(anchor.entity_id, 16)` and check
whether the gold-fact memory ID is in the returned set.

## Why list went up (+2) and single-fact didn't (0)

A plausible reading: list questions (gold contains "," or " and ")
benefit from **wider** candidate pools regardless of relevance,
because the LLM judge accepts partial matches. Single-fact
requires **the right** candidate, so flooding with noise doesn't
help. The +2 list gain is therefore the same "more candidates =
slightly higher list recall" effect we've seen in ISS-138 K=10
and ISS-152 bm25_pool sweeps — it's a generic pool-widening side
effect of injecting more anchors, not evidence that the entity
channel is doing useful work.

## What this rules out

- **HyDE confound**: HyDE was off in both arms.
- **MMR confound**: MMR was off (locked λ=1.0) in both arms.
- **Cross-encoder confound**: off in both arms.
- **Envelope drift from ISS-161**: Arm A single-fact 3/12 with the
  same three IDs (q4/q13/q55) as ISS-161 Arm A confirms the
  baseline path is unchanged.
- **`min_confidence` filter regression**: GraphQuery did not set
  `min_confidence`, so the new filter (ebc9adf) was a no-op in
  this run — the regression is from the channel itself, not from
  the self-review fix.

## What this does NOT rule out

- **conv-26 specificity**: conv-44's inverted ratio (more
  single-fact than list) may give a different verdict. Not tested
  here per ISS-164 plan.
- **resolver tuning**: a stricter resolver (e.g. drop alias / fuzzy
  matches, require exact noun-phrase match) might isolate signal
  the current resolver dilutes. Not tested.
- **fan-out budget**: Step 3 budget caps (`PER_ENTITY_MEMORY_CAP =
  16`, `k_pool`) may need raising when the channel is on.

## Acceptance criteria

- [ ] **AC-1**: Probe hypothesis 1 — for the nine failing
  single-fact queries on conv-26, dump
  `(query_text, resolved_anchors[].canonical_name,
  anchors[].match_strength, gold_fact_memory_id,
  gold_fact_memory.entity_mentions)`. Record verdict per query:
  resolver surfaced the right entity / surfaced a related-but-wrong
  entity / surfaced no useful entity.
- [ ] **AC-2**: Probe hypothesis 2 — instrument Step 3 to log
  budget consumption per `subj_entity` on Arm A vs Arm B for the
  four multi-hop questions that flipped from pass to fail. Verify
  whether Arm B exhausts budget on injected anchors before reaching
  the seed-derived seed_entities.
- [ ] **AC-3**: Probe hypothesis 3 — for each of the nine failing
  single-fact queries, compute
  `memories_mentioning_entity(resolved_anchor.entity_id, 16)` and
  record whether the gold-fact `memory_id` is in the returned set.
  This isolates "channel found the right candidate but ranker
  dropped it" from "channel never had the right candidate".
- [ ] **AC-4**: Based on AC-1..3, write a one-paragraph verdict:
  - 4a) Resolver fundamentally can't see the needed entities →
    ISS-164 closed as falsified; gold-fact entities exist but
    extractor doesn't tag them (escalate to ISS-162 extraction
    context).
  - 4b) Resolver finds the right anchor but fan-out budget
    short-circuits → patch Step 3 budget split (reserve N slots
    for non-injected seed entities) and re-bench.
  - 4c) Anchor → memory edge exists but ranker drops the candidate
    → not a retrieval bug; route to fusion/scoring root-cause.
- [ ] **AC-5**: Decide ISS-164 disposition based on AC-4 verdict.
  Either close as falsified (Phase 1 commits stay inert behind
  locked-false default) or open a Phase 1.5 issue that addresses
  the identified root cause.

## Decision

`ISS-164` Phase 2 STOPS per the falsification rule in the sweep
script header (`B sf - A sf < +1.5` AND `B overall < A overall`).
Phase 1 code (commits 77ef3f3 + ebc9adf in engram, 908a83d in
engram-bench) is **NOT reverted** — the `FusionConfig::locked()`
default of `false` makes it inert for every non-opt-in caller, and
the AC-1..3 root-cause probes need the existing wiring to run.

Next architectural move per the Lever-X plan: **start ISS-162
(extraction context)** as the next weapon against AC-5a, on the
working hypothesis that resolver blindness (hypothesis 1) — not
fan-out budget (hypothesis 2) — is the dominant failure mode. AC-1
should be run first to validate that hypothesis before any further
implementation work.

## Repro

```bash
bash /tmp/iss164_bench_sweep.sh
# 2 arms × ~25min on M1; outputs in benchmarks/runs/ISS164-{A,B}-*
```

Analysis script (single-fact sub-bucket split):
the inline python in this issue's discovery turn, or copy from
`/tmp/iss161_final.py` and rename STAMP to `20260526T213218Z`.

---

## 2026-05-27 UPDATE — AC-1 verdict invalidated; blocked on ISS-166

AC-1 probe ran 3x (defaults / no-auth / unified-substrate-flag-on) and
all three returned **9/9 NO_ANCHORS** on the failing single-fact queries.
This looked like a clean H1 confirmation, but adding an entity census
to the probe surfaced a confounder:

- `Memory::list_entities` returns 3 entities (Caroline, Melanie, Go) —
  legacy `entities` table is populated
- `GraphRead::list_namespaces` returns `[]` — `graph_entities` table is
  empty
- Direct sqlite check on the fresh in-memory DB:
  - `entities` = 3 rows
  - `nodes` = 456 rows (453 memories + 3 entity-kind nodes)
  - `graph_entities` = **0 rows**
  - `graph_edges` = 0 rows

Root cause traced to **engram-bench harness never calling
`Memory::with_pipeline_pool(...)`** — the v0.3 graph subsystem
(ResolutionPipeline → WorkerPool → `apply_graph_delta`) is the only
production writer of `graph_entities`, and it is never wired up in
LoCoMo runs. `enqueue_pipeline_job` silently returns `None` when
`job_queue=None`. Therefore `GraphEntityResolver::resolve()` reads an
empty table and returns `Vec::new()` for **every** query — the entity
channel in ISS-164 Arm B was physically a no-op.

**AC-1 verdict ("H1 CONFIRMED 9/9 NO_ANCHORS") is therefore invalid** —
the probe measured the harness confounder, not the hypothesis. ISS-164
Phase 2 falsification is similarly suspect because Arm A vs Arm B was
"channel-off vs channel-off-with-extra-noop-code" rather than the
intended A/B isolation.

Filed **ISS-166** (P0 blocker) with full evidence chain. ISS-165 is
blocked on ISS-166. After ISS-166 lands:

- Re-run AC-1 probe on the fixed substrate.
- If resolver returns non-empty anchors → cleanly test H1 vs H3.
- If resolver still returns empty → LoCoMo extraction only produces 3
  entities (no Sweden / sunset / Becoming Nicole), which is H1 via a
  different channel (extraction-too-thin) — file as ISS-162 input.

Artifacts:
- `/tmp/iss165-ac1-probe-v3.log` (defaults, with entity census)
- `/tmp/iss165-ac1-probe-v4-unified.log` (UNIFIED_SUBSTRATE=1, same result)
- `/Users/potato/clawd/projects/engram-bench/examples/iss165_ac1_resolver_probe.rs`

---

## 2026-05-27 — AC-1 RE-VALIDATED on post-ISS-166/167 substrate; H1 CONFIRMED in stronger form

### Probe run

Validation probe PID 16259 (2026-05-27 23:48 EDT), using
`engram-bench/examples/iss165_ac1_resolver_probe.rs` against
LoCoMo conv-26 with `ENGRAM_BENCH_PIPELINE_POOL=1
ENGRAM_BENCH_PIPELINE_WORKERS=4 ENGRAM_BENCH_PIPELINE_DRAIN_SECS=600`.

This is the same 9-question probe as the original 2026-05-26 run,
but on the post-ISS-166/167 substrate where the v0.3 pipeline
actually runs end-to-end. Full log: `/tmp/iss167-probe-validate2.log`.

### Substrate state at probe time

- `WorkerPoolStatsSnapshot { jobs_processed: 456, jobs_failed: 0,
  jobs_in_flight: 0, jobs_dropped_inbox_full: 0 }`
- `graph_entities`: 666 rows (vs 0 pre-fix)
- All gold-fact-relevant entities exist:
  - q3 "adoption agencies" → present (17 adoption-related entities)
  - q11 "Sweden" → present (place)
  - q40 "art camp" / "experience '2'" → camp/event entities present
  - q43 "abstract painting" → present (concept)
  - q55 "sunsets" → present (concept; also "beach sunset")
  - q71 "Becoming Nicole" → present (artifact)
  - q75/q76/q37 — verified by inspection, supporting entities present

The substrate has the data. The resolver still can't find it.

### Verdict

```
NO_ANCHORS           : 9/9
ANCHOR_FOUND_NO_GOLD : 0/9
ANCHOR_FOUND_GOLD    : 0/9
```

**H1 CONFIRMED, in a stronger form than originally hypothesized.**

The original H1 framing was "resolver picks the wrong anchor"
(implies it picks _some_ anchor, just not the right one). The
actual failure mode is more fundamental: the resolver picks
**zero** anchors because **it has no mention extraction step at
all**.

### Root cause: GraphEntityResolver lacks mention extraction

Code-layer evidence (verified 2026-05-27 against working tree):

1. **Resolver passes the entire query string as a single mention.**
   `crates/engramai/src/retrieval/adapters/graph_entity_resolver.rs:97-108`:
   ```rust
   let candidate_query = CandidateQuery {
       mention_text: query.to_string(),    // <-- the whole question
       mention_embedding: None,
       kind_filter: None,
       namespace: ns,
       top_k: PER_NAMESPACE_TOP_K,
       recency_window: None,
       now,
   };
   let matches = self.graph.search_candidates(&candidate_query);
   ```

2. **`search_candidates` does exact alias matching.**
   `crates/engramai/src/graph/store.rs:2046-2073`:
   ```rust
   let alias_norm = normalize_alias(&query.mention_text);
   // ...
   let mut stmt = self.conn.prepare_cached(
       "SELECT canonical_id FROM graph_entity_aliases
        WHERE namespace = ?1 AND normalized = ?2
        ORDER BY canonical_id ASC LIMIT 1",
   )?;
   ```
   The lookup is `WHERE normalized = ?` — a single exact-equality
   match against one row of `graph_entity_aliases`. There is no
   substring scan, no tokenization, no NER.

3. **Therefore:** a question like `"Where did Caroline move from
   before settling down?"` is normalized to roughly the same long
   string and looked up as a single alias. No entity alias in the
   database equals that string, so `alias_hit_id = None`, the
   embedding cohort is empty (we don't pass an embedding for
   Factual), and `search_candidates` returns `vec![]`. The
   resolver returns 0 anchors. Every time.

The resolver works (by accident) only when the query text _is_ an
entity name verbatim — e.g. `"Caroline"` would match. For any
natural-language question, it cannot work as currently
implemented.

### Why this explains ISS-148 / ISS-149 / ISS-164 history

- **ISS-148 AC-5a never reachable via Factual plan.** Even when
  the classifier routes to Factual (which itself is rare — see
  ISS-164 dispatcher silent-downgrade evidence), the resolver
  inside Factual returns 0 anchors → Factual plan has nothing to
  traverse from.
- **ISS-164 Phase 2 falsification (overall -3.29pp,
  multi-hop -10.81pp).** Step 2b's
  `seed_entities` injection received 0 anchors. Step 3b' direct
  `memories_mentioning_entity` had no entity ids to expand from.
  The entity channel was wired correctly to a resolver that
  cannot resolve. The regression was pure pipeline overhead with
  no benefit.
- **ISS-149 classifier downgrade.** Even if we fixed the
  classifier to route Factual queries to the Factual plan
  (instead of silently downgrading to Associative), Factual would
  still return zero anchors → no improvement.

### Fix direction (separate work)

The resolver needs a **mention extraction step** before the alias
lookup. Options (in order of complexity/value):

1. **Token-level alias scan** (cheapest, no LLM): tokenize the
   query, slide an n-gram window (n ∈ {1..4}), check each n-gram
   against `graph_entity_aliases`. Take all matches as candidate
   mentions; resolve each.
2. **FTS5-backed mention search**: add an FTS5 index over
   `graph_entity_aliases.normalized` and query with the question
   text; take top-k aliases by BM25.
3. **LLM-based NER** (most accurate, slowest): a small per-query
   LLM call to extract entity mentions, then alias-resolve each.
   Could share a worker pool with the triple extractor.

Option 1 should be tried first — it's free, deterministic, and
will close the obvious gap immediately. Options 2 and 3 can
follow as separate ISSes if Option 1 alone doesn't reach the
target.

### AC status (revised)

- **AC-1** dump resolver anchors vs gold entity_mentions for 9
  failing single-fact queries: **DONE.** Verdict: H1 CONFIRMED in
  stronger form (NO_ANCHORS 9/9 on post-fix substrate with all
  gold entities present).
- **AC-2..AC-5** (originally framed around the
  thin-extraction hypothesis): **superseded.** Mention extraction
  must land before these can be meaningfully re-tested.

### Issue retitle

The current title ("ISS-164 entity channel falsified on conv-26:
zero single-fact lift + multi-hop −10.81pp regression") describes
the symptom. The actual root cause is `GraphEntityResolver`
lacks mention extraction.

Retitling to: **"GraphEntityResolver lacks mention extraction:
returns 0 anchors for any natural-language query"** —
done via `gid_artifact_update`.

### Status

- `blocked` (on ISS-166) → `open` (ISS-166 resolved, root cause
  now known and actionable)
- Was `blocked_by: [ISS-166]` → cleared
- Severity raised: this is the highest-leverage retrieval bug
  currently known. Fixing it should unblock ISS-148 AC-5a,
  ISS-149 routing decisions, and ISS-164 entity channel.
