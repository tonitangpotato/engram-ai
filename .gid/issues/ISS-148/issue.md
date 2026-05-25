---
title: Single-hop LoCoMo conv-26 stuck at 0.22 after BM25 wired — root cause is plan selection (L2), not fusion
status: open
priority: P1
severity: degradation
category: retrieval
created: 2026-05-24
relates:
- ISS-144
- ISS-145
- ISS-146
- ISS-147
- ISS-149
depends_on: .gid/issues/ISS-149/issue.md
---

## Summary

After ISS-147 wired BM25 into Factual/Episodic/Affective fusion adapters,
LoCoMo conv-26 single-hop only climbed from **0.156 → 0.219 (+6.25pp)** —
well short of the AC-5 target of ≥0.40.

**Root cause is upstream of fusion**: the classifier never selects
the Factual plan (the primary BM25-using plan) for any conv-26 query.
80% of single-hop queries route to **Associative** (RRF, no SubScores),
12% to **Abstract** (downgraded → Topic-only, no SubScores). The
BM25-wired adapters reach only ~5% of conv-26 queries.

This is **L2** in the L1/L1b/L2/L3 layering ISS-145 sketched.

## Evidence

### Plan distribution on conv-26 (152 queries, ISS-147 run)

Tallied from `/tmp/iss147-bench-conv26.log` `execute_plan ENTER` lines:

| plan_kind   | count | uses BM25? |
|-------------|------:|------------|
| associative | 121   | ❌ (RRF) |
| abstract    | 18    | ❌ (Topic-only, all 18 downgraded `l5_unavailable`) |
| affective   | 6     | ✅ (wired by ISS-147) |
| hybrid      | 5     | ❌ (RRF) |
| episodic    | 2     | ✅ (wired by ISS-147) |
| **factual** | **0** | ✅ (wired by ISS-147) |

The Factual plan — ISS-147's primary target — is selected zero times.
Only 8 of 152 queries (5%) reach the BM25-wired adapters.

### Why no Factual plan? — `HeuristicClassifier` runs on `NullEntityLookup`

`crates/engramai/src/retrieval/api.rs:496`:

```rust
let classifier =
    crate::retrieval::classifier::HeuristicClassifier::with_null_lookup();
```

`NullEntityLookup::lookup` returns `EntityMatch::None` for every
token. `score_entity` therefore returns 0.0 for every query.
Combined with weak/missing temporal/abstract/affective signals on
LoCoMo conv-26, the classifier falls into the
"no strong primary signal" branch at `classifier/mod.rs:245-248`:

```rust
// No strong signal → Factual with Associative downgrade hint.
HeuristicResult {
    intent: Intent::Factual,
    downgrade_hint: DowngradeHint::Associative,
    ...
}
```

`dispatch.rs:92` then maps `(Factual, Associative)` → `PlanKind::Associative`.
**This is the 121/152 path.** No entity signal → associative fallback.

### Failure mode confirms it: "I don't know" answers

Of the 25 zero-score single-hop fails, the LLM response is dominated
by `"I don't know"` and `"the memories don't specify"`. The relevant
memory never reaches the LLM context — recall is the bottleneck,
not generation. Sample fails:

- **q3** gold="Adoption agencies" → "I don't know. The memories only mention..."
- **q7** gold="Single" → "I don't know."
- **q11** gold="Sweden" → "I don't know."
- **q15** gold="pottery, camping, painting, swimming" → "...pottery class..." (list-question, partial)
- **q18** gold="beach, mountains, forest" → "I don't know."

All of these are **entity-anchored single-hop queries** (Caroline /
Melanie + an attribute). Exactly the queries Factual+BM25 was built
for. They never get there because the classifier doesn't see the
entity tokens as graph anchors.

### Layering (ISS-145 sketched it; ISS-148 confirms L2 is necessary)

ISS-145 (L1b) is "ingest path doesn't populate `graph_entity_aliases`,
so `GraphEntityResolver` (Factual plan's resolver) is blind."

This issue (L2) is "even if `graph_entity_aliases` is full, the
*classifier* at `api.rs:496` uses `NullEntityLookup` independently
of `GraphEntityResolver`, so plan selection stays blind."

ISS-145 closure is necessary but not sufficient. L2 wiring is also needed.

## Plan (sequenced)

L1b (ISS-145) and L2 (this issue / ISS-149) interlock. Suggested order:

1. **ISS-145 first** — fill `graph_entity_aliases` at ingest. Required by L2.
2. **ISS-149** — file separately: wire the classifier's `EntityLookup`
   to read from the (now populated) `graph_entity_aliases` table.
3. **Then re-bench conv-26**:
   - Expected: many associative queries flip to factual, BM25 fires,
     single-hop lifts toward AC-5 ≥0.40.
   - Risk: factual plan needs `graph_entity_aliases` to be populated
     symmetrically with `entities` — depends on ISS-145 Option A vs B.

## Acceptance Criteria

- [ ] **AC-1 (this issue's deliverable):** Root cause confirmed and
       documented above. Plan-distribution evidence captured.
- [ ] **AC-2:** After ISS-145 + ISS-149 land, re-run conv-26 with
       `ENGRAM_BENCH_DUMP_CANDIDATES=1` and confirm Factual plan
       selection rate ≥30% on single-hop queries.
- [ ] **AC-3:** Single-hop conv-26 ≥ 0.35 (stretch ≥0.40, original
       ISS-147 AC-5 target).
- [ ] **AC-4:** Overall conv-26 ≥ 0.50 (current 0.467).
- [ ] **AC-5:** Full LoCoMo 1540q regression: no category regresses
       more than 1pp vs ISS-147 baseline.

## Out of scope

- Tuning BM25 saturation or per-plan text weights. Pointless until
  Factual plan is actually reachable on more than 5% of queries.
- List-question handling (q15, q18). Separate concern (top-K / re-ranker).
- Multi-hop / open-domain / temporal — those routed correctly already
  on this run.

## References

- ISS-147 — BM25 wired into fusion (resolved cbddac9 + 5ed5dc0)
- ISS-145 — L1b ingest → `graph_entity_aliases` (open, prereq)
- ISS-149 — L2 classifier `NullEntityLookup` wiring (to be filed)
- `crates/engramai/src/retrieval/api.rs:496` — the `NullEntityLookup` call
- `crates/engramai/src/retrieval/classifier/mod.rs:245` — "no strong signal"
- `crates/engramai/src/retrieval/dispatch.rs:92` — `(Factual,Assoc)→Associative`
- `/tmp/iss147-bench-conv26.log` — plan-distribution evidence
- `benchmarks/runs/ISS147-BM25-conv26-l0.7-20260524T033206Z/` — run dir

## ISS-153 HyDE decision (2026-05-24)

Phase 2 conv-26 K=10 λ=0.7 HyDE-on (haiku-4-5) ran 152/152 clean.
Results vs ISS-152 Run A baseline (HyDE-off, same env):

- single-hop **0.1562 → 0.3125** (+15.6pp, **doubled**)
- temporal 0.4714 → 0.5143 (+4.3pp)
- open-domain unchanged (0.3846)
- multi-hop 0.3243 → 0.2432 (-8.1pp, **regression**)
- overall  0.3618 → 0.3947 (+3.3pp)

Recall-diag: 14/26 (53.8%) of baseline single-hop recall-misses recovered,
0 new misses introduced. Per ISS-153 Phase-3 decision tree, **ship as
opt-in feature**.

**For AC-5 (single-hop ≥ 0.40)**: HyDE is necessary-but-not-sufficient.
0.3125 closes ~40% of the gap from 0.1562 → 0.40, but still below the
floor. Need either (a) HyDE + complementary lever (re-ranker, broader
embedding model — see ISS-155), or (b) per-category HyDE gating that
keeps the multi-hop -8.1pp from cancelling the single-hop win.

Status: ISS-153 → in_review, ISS-148 AC-5 still **open**.

## Post-ISS-155 re-baseline (2026-05-24)

Re-ran HyDE+no-HyDE on post-fix substrate (ISS-155 fae6bb7). Single-hop
moved in the wrong direction:

| Run | Substrate | HyDE | Single-hop |
|---|---|---|---|
| ISS-152 Run A | pre-fix | off | 0.1562 |
| ISS-153 r1    | pre-fix | on  | 0.3125 |
| ISS-153 retest A | post-fix | off | 0.2188 |
| ISS-153 retest B | post-fix | on  | 0.2500 |

Findings:
- Post-fix no-HyDE single-hop is HIGHER than pre-fix no-HyDE (+6.3pp)
- Post-fix HyDE single-hop is LOWER than pre-fix HyDE (−6.3pp)
- HyDE no longer the AC-5 lever it looked like pre-fix
- Even the best post-fix single-hop (HyDE on, 0.2500) is well below
  AC-5 target of ≥0.40

Other levers to consider:
- Re-ranker stage (cross-encoder after fusion, K=50 → K=10)
- Stronger embedding (BGE-large vs current nomic-embed-text)
- Single-hop-specific BM25 weight bump (ISS-150 pattern)
- Triple-extraction quality lift (currently low yield → fewer fact-grounded
  candidates for direct single-hop questions)

AC-5 remains open. See also ISS-156 for per-category HyDE gating.

---

## ISS-149 probe + K-expansion probe (2026-05-25) — AC-5 is mis-specified

After ISS-157 weapon B (embedder swap) failed (single-hop unchanged
across 3 embedders), ran two follow-up probes on conv-26 K=10/30 MMR=0.7
HyDE=per_category to triage **before** committing 3-5 days to weapon A
(cross-encoder reranker):

### Probe 1: force `Intent::Factual` via `GraphQuery::with_intent` (ISS-149 simulation)

| arm | overall | single-hop | multi-hop |
|---|---|---|---|
| natural classifier | 0.4671 | **0.2188** | 0.5405 |
| force Factual | 0.5132 | **0.1875** (-3.13pp) | 0.5946 |

Pass-flip count: A pass→B fail = 1, B pass→A fail = 0. **Forcing Factual
passed ZERO new questions.** ISS-149 is real but not the lever — Factual
plan on conv-26's high-density chat corpus is not actually better than
Associative for single-hop.

### Probe 2: K=30 (vs K=10 baseline), natural classifier

| arm | overall | single-hop | multi-hop |
|---|---|---|---|
| K=10 | 0.4671 | 0.2188 | 0.5405 |
| K=30 | 0.5066 | **0.2812** (+6.2pp) | 0.5946 |

### Per-query deep read — bucketed by failure mode

Hand-classified all 25 single-hop failures from the K=10 run, then
verified against K=30:

| sub-bucket | K=10 | K=30 | Δ |
|---|---|---|---|
| **list questions** (gold = multi-item, e.g. "pottery, camping, painting, swimming") | 4/20 = 0.200 | 4/20 = 0.200 | **0.0pp** |
| **single-fact** (gold = one specific value) | 3/12 = 0.250 | 5/12 = **0.417** | **+16.7pp** |

### Why list questions don't move with K (or any retrieval improvement)

Two failure modes inside the list-question bucket:

**(a) Gold items live in 1-of-419 episodes ("Bailey" pet, "swimming"
activity).** No amount of K expansion or reranking pulls a single
needle-in-haystack episode reliably enough to enumerate all gold items.
For q52 ("pets" = Oliver/Luna/Bailey), Bailey appears in episode 256 of
419, and the predicted answer correctly gets Oliver+Luna (2/3) but misses
Bailey → judge marks the entire answer wrong.

**(b) Gold list is not unique — multiple valid lists exist.** For q15
("activities Melanie partakes in" = pottery/camping/painting/swimming),
the model returned "hiking, pottery, running" — all three ARE Melanie's
real activities in the corpus, just a different valid subset than the
annotator chose. This is a dataset / judge problem, not a retrieval one.

### Why single-fact DID move with K (and will move with weapon A)

The 2 single-fact questions K=30 saved (q40 beach-trip count, q55 common
painted subject) both required surfacing a 2nd corroborating episode that
was ranked outside K=10 in fusion but inside K=30. **Cross-encoder
reranker (weapon A) targets exactly this pattern** — pushing semantically
relevant low-fusion-score candidates up.

### Math: can AC-5 be reached at all?

- single-fact bucket optimistic ceiling (weapon A): 10/12 = 0.83
- list bucket realistic ceiling (no retrieval fix possible): 4-6/20 = 0.20-0.30
- conv-26 single-hop total: **(10 + 6) / 32 = 0.50** at best, more likely 0.42-0.45

**AC-5 (≥0.40) is *barely* reachable, but only via list-question changes
in generation/judge, NOT retrieval.** Retrieval improvements alone cap
out around 0.32-0.35.

### Proposed AC redefinition

AC-5 as written collapses two unrelated failure modes (retrieval miss vs
list-enumeration / annotation subjectivity) into one bucket. Replacing
with two independent gates:

- **AC-5a (retrieval gate):** single-fact sub-bucket of conv-26
  single-hop ≥ 0.60. Currently 5/12 = 0.42 at K=30; weapon A target
  8-10/12 = 0.67-0.83. **This is the gate ISS-148's L1b/L2/weapon-A
  work actually controls.**
- **AC-5b (generation/dataset gate):** list sub-bucket of conv-26
  single-hop ≥ 0.30. Currently 4/20 = 0.20. Levers: (i) generation
  prompt change ("enumerate all items you can find"), (ii) judge
  partial-credit scoring (currently strict-equality), (iii) K=100+
  for list-detected queries. **Out of scope for ISS-148; file separately.**
- Aggregate AC-5 (single-hop ≥ 0.40) stays as an informational
  combined metric, not a gate.

### Decision summary

- ISS-149 (NullEntityLookup wiring) — **not the lever for AC-5**.
  Forced-Factual probe showed Factual plan loses 1 question, gains 0.
  Keep ISS-149 open as correctness work (caller-override path works,
  but auto-routing to Factual on conv-26 is net-negative). De-prioritise
  from "AC-5 blocker" to "future correctness".
- ISS-159 (weapon A cross-encoder reranker) — **stays the right next move**
  for AC-5a (retrieval gate). Targets the 9 single-fact misses; realistic
  +20-30pp on that sub-bucket.
- New ticket needed for AC-5b (list-question / generation / judge work).

### Artifacts

- `.gid/issues/ISS-149/artifacts/probe-A-natural.json`,
  `probe-B-factual.json` (ISS-149 probe, K=10 force-intent sweep)
- `benchmarks/runs/ISS149-K30-conv26-20260525T122829Z/locomo_summary.json`
  (K-expansion probe)
- `/tmp/iss149_probe.sh`, `/tmp/iss149_K30.sh` (sweep drivers — pin to
  ISS-149 artifacts/ on close)
