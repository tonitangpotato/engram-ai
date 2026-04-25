# Design Review r1 — v03-retrieval

**Target:** `.gid/features/v03-retrieval/design.md` (627 lines)
**Requirements:** `.gid/features/v03-retrieval/requirements.md` + master `.gid/docs/requirements-v03.md`
**Reviewer:** rustclaw (main-agent manual review)
**Date:** 2026-04-24
**Status:** findings only, not yet applied

## Summary

- Critical: 0
- Important: 7
- Minor: 10
- Total: 17

The design is structurally sound and thorough on plan machinery (§4) and fusion (§5). The largest issues are **internal contradictions** around L5 on-read synthesis, a **GUARD mis-attribution** (§4.4 cites GUARD-6 where it means GUARD-2), **terminology drift** (Hybrid vs Mixed; Associative as intent vs plan), **missing GUARD citations** (2, 8), and **several referenced types never defined** (TimeWindow, SignalWeightMatrix, TieBreakOrder, RetrievalError variants, PlanDetail and friends).

---

## FINDING-1 🔴 Important — §4.4 mis-attributes GUARD-6 where the argument is GUARD-2 ✅ Applied

**Section:** §4.4 Abstract plan — "Why not return empty when L5 is unpopulated?"

**Problem:** The paragraph justifies the fallback by saying *"Returning nothing when L5 is empty would violate GOAL-3.6's 'no silent degrade' posture — GUARD-6 says cognitive state never gates results."* GUARD-6 is about **cognitive state never gating** retrieval (i.e., Affective must not hide results based on affect). The argument here is actually about **GUARD-2 — never silent degrade** (substrate empty → downgrade with reason, not return nothing).

**Suggested fix:** Replace "GUARD-6 says cognitive state never gates results" with "GUARD-2 says the system must never silently degrade — substrate-empty is a legitimate reason, and the outcome is surfaced via `RetrievalOutcome::DowngradedFromAbstract`." Also add GUARD-2 to §11 traceability table for Abstract plan.

**Rationale:** Wrong GUARD citation confuses readers and implementers; tests derived from this section will test the wrong invariant.

**Applied:** §4.4 L5-empty rationale now cites GUARD-2 (never silent degrade) and `RetrievalOutcome::DowngradedFromAbstract`; GUARD-2 row added to §11 traceability.

---

## FINDING-2 🔴 Important — Internal contradiction: L5-on-read synthesis ✅ Applied

**Section:** §4.4 vs §6.3 (tracing) vs §8.1 (metrics)

**Problem:** §4.4 explicitly states *"On-demand synthesis from the read path is **not** attempted in v0.3."* But §6.3 says *"`l5_llm_calls`: non-zero only if Abstract plan triggered on-demand synthesis — rare"*, and §8.1 lists `retrieval_l5_synthesis_total` as a retrieval metric. If on-demand synthesis never fires on the read path in v0.3, the counter/metric are dead code on the retrieval side.

**Suggested fix:** Pick one:
- (a) **If L5 is read-only in v0.3:** remove `l5_llm_calls` from `PlanTrace` and `retrieval_l5_synthesis_total` from §8.1; note that L5 synthesis cost is exposed by the compiler (v03-resolution), not retrieval. Add a one-line note in §4.4 stating "L5 is strictly read-only in v0.3; synthesis cost lives on the compiler's counters."
- (b) **If L5 can fire on read for some Abstract queries:** correct §4.4 to describe when, and add the cost budget into §7.3.

Recommend (a) to match the "rare" framing of GOAL-3.13 and keep the retrieval surface deterministic in token budget.

**Rationale:** Engineering integrity (skill phase 6, check 23) — contradictory framing becomes implementation-time debt.

**Applied:** Chose option (a). Removed `l5_llm_calls` from `PlanTrace` (§6.3) and `retrieval_l5_synthesis_total` from §8.1. §4.4 now states L5 is strictly read-only in v0.3; §6.3 GOAL-3.13 note rewritten to point at compiler counters and classifier-side LLM metering.

---

## FINDING-3 🔴 Important — Intent taxonomy inconsistent: `Associative` is plan but referenced as intent ✅ Applied

**Section:** §3.1, §3.2, §3.4, §6.2

**Problem:** §3.1 lists **5** intents: `Factual, Episodic, Abstract, Affective, Hybrid`. But §3.2 Stage 1 rules assign intent `Associative` ("if 0 strong signals → Associative"); §3.4 uses Associative as a downgrade target plan; §4 lists Associative as a plan-kind. The enum `Intent` is never defined in §6.2. Is `Associative` a 6th intent, a default plan when classification produces nothing, or a plan-only concept that's reached via Factual downgrade?

**Suggested fix:** Choose one model and apply consistently:
- **Model A (recommended):** Intents are **5** (as §3.1 states). `Associative` is a **plan-kind only**, reached when the classifier emits no strong signals — this is represented as `Intent::Factual` with `signals: []` routed by the plan builder to the Associative plan. Update §3.2 Stage 1 to say "if 0 strong signals → Intent::Factual with `downgrade_hint = Associative`, plan builder materializes as Associative plan".
- **Model B:** Intents are **6** — add `Associative` to §3.1. Then `Intent::Associative` is the "nothing strong matched" label. This is cleaner but changes GOAL-3.1 phrasing ("classify into exactly 5 intents").

Also: define `enum Intent` explicitly in §6.2 with variants listed.

**Rationale:** Ambiguity will cause test drift ("did the classifier emit the right intent?" is untestable if the taxonomy is fuzzy).

**Applied:** Chose Model A. §3.2 Stage 1 `|strong_signals| == 0` now emits `Intent::Factual` with `downgrade_hint = Associative`; §3.2 note clarifies 5 intents with Associative as plan-only. `enum Intent` defined in new §6.2a Types.

---

## FINDING-4 🔴 Important — Terminology drift: Hybrid vs Mixed ✅ Applied

**Section:** §4.7 (Hybrid), §5.4 (Mixed), §7.1 (Mixed)

**Problem:** §3.1 and §4.7 use **Hybrid** as the plan name. §5.4 says *"Parallel sub-plans in Mixed (§4.4) must fuse deterministically"* — but §4.4 is Abstract, not Mixed, and the feature is named Hybrid. §7.1 also reads *"Mixed total = max(sub_plan_a, sub_plan_b) + rrf_fusion"*. This is plain name drift between sections.

**Suggested fix:** Rename all occurrences of "Mixed" to "Hybrid" (§5.4 paragraph 2, §7.1 cost table row); fix the section reference in §5.4 from §4.4 to §4.7.

**Rationale:** Consistency; otherwise reviewers/implementers will think there are two distinct plan kinds.

**Applied:** Replaced all "Mixed" occurrences with "Hybrid" in §5.4, §7.1 cost table, `FusionConfig.rrf_k` comment, and §5.1 signal-source table. §5.4 cross-ref corrected from §4.4 to §4.7.

---

## FINDING-5 🔴 Important — Missing GUARD citations (GUARD-2, GUARD-8) in §11 traceability ✅ Applied

**Section:** §11 Traceability table

**Problem:** Feature requirements list GUARD-1, 2, 3, 6, 8 as relevant. The §11 table only lists **GUARD-3** and **GUARD-6**. GUARD-2 (never silent degrade) is *behaviorally* satisfied by §4.4 / §6.4 (typed outcomes); GUARD-8 (affect_snapshot immutable, cosine-only) is behaviorally satisfied by §4.5 ("Affect at query time is read from the latest cognitive_state; affect on candidate memories is read from their immutable write-time snapshot"). Both satisfactions are unsurfaced.

**Suggested fix:** Add two rows to §11:
- `GUARD-2 | never silent degrade | §4.4 downgrade with reason, §6.4 RetrievalOutcome variants`
- `GUARD-8 | affect_snapshot immutable, cosine-only | §4.5 reads write-time snapshot, no recomputation; cosine used in affect_similarity fusion term`

**Rationale:** Traceability is the primary defense against requirement drift; silent satisfaction doesn't hold up under review.

**Applied:** §11 traceability table now includes GUARD-2 (downgrades + `RetrievalOutcome` + hybrid truncation) and GUARD-8 (write-time affect snapshot, cosine). Footer updated to "14 GOALs + 4 GUARDs".

---

## FINDING-6 🔴 Important — Hybrid plan silently drops 3rd+ strong signals ✅ Applied

**Section:** §4.7 + §3.2 Stage 1

**Problem:** Classifier Stage 1 rule routes to Hybrid when *"`|strong_signals| ≥ 2` and each ≥ τ_high"*. §4.7 says *"Max 2 sub-plans — 3+ explodes cost"*. What happens when classifier emits 3 or 4 strong signals? The top 2 become sub-plans; the rest are **silently dropped** with no trace. This is a latent silent-degrade (brushes against GUARD-2).

**Suggested fix:** In §4.7 add:
- When `|strong_signals| > 2`, keep the top-2 by signal score, but emit a `PlanTrace.hybrid_truncated: Vec<DroppedSignal>` for observability.
- Add a metric `retrieval_hybrid_truncation_total{dropped_kind=...}` in §8.1.
- Or: widen the cap to 3 (cost analysis — 3 sub-plans at k=6 each with text/graph fusion is ~3x the fusion cost, still within the 100ms p95 budget on current benchmarks).

Recommend the "keep 2 + trace" option; it preserves GUARD-2 without adding cost.

**Rationale:** Classifier output and plan execution should be observationally consistent; dropped intent signals must be surfaceable.

**Applied:** §4.7 step 1 records dropped strong signals in `PlanTrace.hybrid_truncated: Vec<DroppedSignal>`; `retrieval_hybrid_truncation_total{dropped_kind=...}` metric added in §8.1; `DroppedSignal` defined in §6.2a.

---

## FINDING-7 🔴 Important — Classifier LLM cost not separately metered ✅ Applied

**Section:** §8.1, §6.3, GOAL-3.13

**Problem:** §3.2 Stage 2 LLM fallback consumes LLM calls (retrieval-side cost). §8.1 has `retrieval_classifier_method_total{method=llm}` (call count) but no cost/latency breakdown; `l5_llm_calls` is L5-only. GOAL-3.13 says "L5 synthesis cost observable separately" — implying the retrieval-side LLM cost should also be separable.

**Suggested fix:** Add to §8.1:
- `retrieval_classifier_llm_calls_total` (counter)
- `retrieval_classifier_llm_tokens_total{direction=prompt|completion}` (counter)
- `retrieval_classifier_llm_duration_seconds` (histogram)

And surface these in `PlanTrace` as `classifier.llm_cost: Option<LlmCost>` (None when rule-only).

**Rationale:** Without this, a cost regression in classifier fallback is invisible to ops; the "LLM call budget" is underspecified.

**Applied:** §8.1 adds `retrieval_classifier_llm_calls_total`, `retrieval_classifier_llm_tokens_total{direction}`, `retrieval_classifier_llm_duration_seconds`. §6.3 notes `ClassifierTrace.llm_cost: Option<LlmCost>`; `LlmCost` defined in §6.2a.

---

## FINDING-8 🟡 Important — GUARD-6 benchmark phrasing is wrong ✅ Applied

**Section:** §9 "GUARD-6 — affect modulates, never gates"

**Problem:** §9 defines the GUARD-6 test as *"`Affective` plan never returns a strict subset of `Associative`-plan results for the same query"*. This is the wrong invariant. Affective uses its own candidate pool (`hybrid_recall(query, k=K_seed_affective)`), not the full Associative pool, so it legitimately may not include some Associative results — without violating GUARD-6.

**Suggested fix:** Rephrase:
> **GUARD-6 test:** For Affective plan on query `Q`, construct two self-states `S1` and `S2` with differing valence. The **union of result IDs** across `S1` and `S2` must equal the affect-plan's own candidate pool (i.e., no candidate is *removed* purely because of affect). Affect may reorder or downweight, but never filter out. Formally: `result(Q, S1) ∪ result(Q, S2) == candidate_pool(Q)` (up to k-truncation at the tail).

**Rationale:** A wrong invariant test either over-accepts (passes when it shouldn't) or over-rejects (fails legitimate designs). Phrasing it in terms of the candidate pool matches what GUARD-6 actually mandates.

**Applied:** §9 GUARD-6 property test rephrased: invariant is `result_ids(Q, S1) ∪ result_ids(Q, S2) == candidate_pool(Q)`, where candidate pool is the Affective plan's own seed set (§4.5 step 2).

---

## FINDING-9 🟡 Important — Episodic plan unhandled case: no time expression found ✅ Applied

**Section:** §4.2 step 1

**Problem:** Step 1 says *"Parse time expression from query ... relative ('last week') resolves against query_time"*. What happens if the Episodic-routed query contains **no recognizable time expression** (classifier mis-routed, or expression too fuzzy)? Does the plan:
- Fall back to `TimeWindow::AllTime`?
- Downgrade to Associative via `RetrievalOutcome::DowngradedFromEpisodic`?
- Error?

Unspecified.

**Suggested fix:** In §4.2 step 1 add:
> If no time expression can be parsed with confidence ≥ 0.5, the Episodic plan downgrades to Associative, emitting `RetrievalOutcome::DowngradedFromEpisodic { reason: "no_time_expression" }`. The graph filter step is dropped; fusion proceeds with Associative weights.

Update §6.4 outcomes to include `DowngradedFromEpisodic`.

**Rationale:** Explicit typed outcome keeps GUARD-2 satisfied and makes the downgrade observable.

**Applied:** §4.2 step 1 downgrades to Associative with `RetrievalOutcome::DowngradedFromEpisodic { reason: "no_time_expression" }` when parse confidence < 0.5. `DowngradedFromEpisodic` and `DowngradedFromAbstract` both added to §6.4.

---

## FINDING-10 🟡 Important — L5 sample rate for affect-divergence telemetry unspecified ✅ Applied

**Section:** §4.5 step 5 + §8.1 `retrieval_affect_rank_divergence`

**Problem:** §4.5 says Kendall-tau rank-divergence "runs only when `GraphQuery.explain = true` or a sample-rate flag fires." No default sample rate is specified. If sample rate is 0 in production, GOAL-3.8 ("affect-weighting produces observable, metric-defined difference") is observable only via explicit `explain=true` — which most production callers won't set.

**Suggested fix:** Specify a default: `default_affect_divergence_sample_rate: 0.01` (1% of Affective plan calls compute the second ranking). Document this in §7.3 as a retrieval config knob. Note that setting it to 0 makes GOAL-3.8 observability degraded; tests should pin it to ≥ 0.01.

**Rationale:** Default behavior should satisfy the GOAL in production, not only when the caller opts in.

**Applied:** §4.5 step 5 specifies default `affect_divergence_sample_rate = 0.01`; knob listed in §7.3; §9 affect-divergence test pins the rate ≥ 0.01.

---

## FINDING-11 🟡 Minor — Fusion with missing graph signal: redistribution unspecified ✅ Applied

**Section:** §5.2 Episodic, §4.1 Factual step 6

**Problem:** Episodic fusion is `0.55*text + 0.30*graph_score (if present) + 0.15*recency`. If graph step 3 didn't produce a graph score (e.g., graph store unavailable, but GUARD-6 says we proceed), the 0.30 weight is unallocated. Options:
- Redistribute to text (normalize remaining weights to sum to 1.0).
- Set graph_score to 0 (zero contribution, but total weight now 0.70, scores no longer in [0,1]).
- Drop weight entirely (same as above).

Unspecified.

**Suggested fix:** Add to §5.2 a paragraph:
> **Missing signal normalization:** When a fusion component is absent (e.g., no graph score because the graph expansion step was skipped or returned empty), the remaining weights are renormalized to sum to 1.0 by proportional scaling. This preserves score ranges in [0, 1] and keeps fusion scores comparable across calls.

**Rationale:** Unspecified fusion behavior is a test flake magnet and a silent-degrade risk.

**Applied:** §5.2 now documents **Missing signal normalization** — remaining weights proportionally renormalized to sum to 1.0, recorded in `FusionTrace`.

---

## FINDING-12 🟡 Minor — `FusionConfig::locked()` not documented at API boundary ✅ Applied

**Section:** §5.4 + §6.2

**Problem:** `FusionConfig::locked()` is introduced in §5.4 as the deterministic-mode constructor (fusion rules, tie-break order, RNG disabled) but §6.2 `Memory::recall*` APIs never expose a way to request locked-mode fusion. A benchmark or test caller wanting determinism has no opt-in surface.

**Suggested fix:** Either:
- Add `Memory::recall_locked(intent, query, k) -> ...` as a deterministic-mode variant, OR
- Add a `FusionConfig` parameter to `recall*` APIs (with `FusionConfig::default()` and `FusionConfig::locked()`).

Document in §6.2 which APIs accept fusion overrides.

**Rationale:** Otherwise `locked()` is un-reachable from public API — benchmarks can't use it without private hooks.

**Applied:** §6.2 adds `Memory::graph_query_locked` deterministic-mode API pinned to `FusionConfig::locked()`; design notes clarify the `graph_query` vs `graph_query_locked` split.

---

## FINDING-13 🟡 Minor — Undefined types referenced throughout ✅ Applied

**Section:** §6.2, §6.3, §5.4

**Problem:** Types referenced but never defined: `TimeWindow`, `SignalWeightMatrix`, `TieBreakOrder`, `RetrievalError` (variants), `PlanDetail`, `ClassifierTrace`, `Downgrade`, `FusionTrace`, `BiTemporalTrace`, `AffectTrace`, `SubScores`, `AffectVector`, `Intent` (enum definition).

**Suggested fix:** Add a §6.2a "Types" subsection with at minimum a one-line definition + variant list for each:
- `TimeWindow { None, At(DateTime<Utc>), Range { from, to }, Relative(Duration) }`
- `TieBreakOrder { ByMemoryIdAsc }` (fixed in v0.3; enum exists for future-proofing — or delete the field)
- `RetrievalError { Timeout, StoreUnavailable, ConfigError(String), ... }`
- Traced structs (`PlanDetail`, `ClassifierTrace`, etc.) can be one-line "struct with fields ... see impl" but the section should enumerate the list.

**Rationale:** Implementers will either (a) stop and ask, or (b) guess, introducing inconsistencies across plans.

**Applied:** Added §6.2a Types with one-line definitions for `Intent`, `TimeWindow`, `SignalWeightMatrix`, `RetrievalError`, `AffectVector`, `SubScores`, `DroppedSignal`, `LlmCost`, and the trace structs.

---

## FINDING-14 🟡 Minor — `TieBreakOrder` field appears unnecessary ✅ Applied

**Section:** §5.4 + §6.2 `FusionConfig`

**Problem:** `TieBreakOrder` is a field on `FusionConfig`, but §5.4 says *"Ties in fusion score broken by `(memory_id ascending)`"* — a single fixed rule. A field with one legal value is dead config surface.

**Suggested fix:** Either:
- Remove the `tie_break` field; document the rule as a hard-coded invariant in §5.4.
- OR justify the enum by listing ≥ 2 variants now (e.g., `ByMemoryIdAsc`, `ByRecencyDesc`, `ByWriteOrder`) with notes on when each is useful.

**Rationale:** Dead config is the cheapest kind of debt, but it's still debt.

**Applied:** Removed `tie_break: TieBreakOrder` field from `FusionConfig`; §5.4 documents `(memory_id ascending)` as a hard-coded invariant with a note on future-version extensibility. `TieBreakOrder` correspondingly not included in §6.2a Types.

---

## FINDING-15 🟡 Minor — Episodic graph-filter threshold (0.3) undocumented ✅ Applied

**Section:** §4.2 step 3

**Problem:** Step 3 says *"If any entity signal had score ≥ 0.3 even though not dominant"*. The 0.3 threshold is not traced to §3.2 (which uses τ_high, τ_low) nor to a config knob in §7.3. Where does 0.3 come from?

**Suggested fix:** Either (a) define `τ_graph_filter = 0.3` in §3.2 alongside τ_high / τ_low and reference it in §4.2 step 3, or (b) add it to §7.3 retrieval config knobs with a sentence of justification ("chosen so episodic queries with weak entity hints still get a 1-hop expansion; tuned against the routing benchmark").

**Rationale:** Magic numbers that don't appear in the config section are invisible to operators.

**Applied:** §4.2 step 3 now references `τ_graph_filter` (default `0.3`) and forwards to §7.3 for the knob + justification.

---

## FINDING-16 🟡 Minor — `K_seed_affective` undefined ✅ Applied

**Section:** §4.5 step 2 + §7.3

**Problem:** Step 2 of Affective plan uses `hybrid_recall(query, k=K_seed_affective)`. `K_seed_affective` is never defined; §7.3 cost caps list `k`, `max_hops`, and a few others but not this.

**Suggested fix:** Add to §7.3: `K_seed_affective = 3 * requested_k` (default), with a cap at 60 to bound cost. Cite in §4.5 step 2.

**Rationale:** Implementation will either hard-code this or ask; either outcome is a diff away from design intent.

**Applied:** §4.5 step 2 now states `K_seed_affective = 3 * requested_k` (capped at 60); added to §7.3 cost caps.

---

## FINDING-17 🟡 Minor — GUARD-8 benchmark inflexibility on Kendall-tau threshold ✅ Applied

**Section:** §9 affect-divergence test

**Problem:** GOAL-3.8 requirement explicitly says *"The exact metric and threshold may be tuned during implementation"*. §9 hard-codes "Kendall-tau < 0.9 on ≥ 20 queries". No mechanism to retune without editing the design.

**Suggested fix:** In §9 add:
> Metric/threshold are tuning parameters in `benchmark_config.toml` (keys `affect_divergence.metric = "kendall_tau"`, `affect_divergence.threshold = 0.9`). Re-tuning during implementation updates the config, not this design doc.

**Rationale:** Requirement explicitly allows tunability; design should reflect that mechanically.

**Applied:** §9 affect-divergence test now notes metric/threshold/sample-rate live in `benchmark_config.toml` (keys `affect_divergence.metric`, `affect_divergence.threshold`, `affect_divergence.sample_rate`); retuning updates config, not design.

---

## Applied Status

All findings applied below (except where noted).

## Applied

### FINDING-1 ✅
- §4.4 L5-empty rationale now cites GUARD-2 (never silent degrade) and `RetrievalOutcome::DowngradedFromAbstract`; GUARD-2 added to §11 traceability.

### FINDING-2 ✅
- Chose option (a): removed `l5_llm_calls` from `PlanTrace` and `retrieval_l5_llm_calls_total` from §8.1. §4.4 now states L5 is read-only in v0.3; §6.3 GOAL-3.13 note rewritten to point at compiler counters and classifier-side LLM metering.

### FINDING-3 ✅
- Chose Model A: §3.2 Stage 1 "|strong_signals| == 0" now emits `Intent::Factual` with `downgrade_hint = Associative`; added a note clarifying 5 intents and Associative as plan-only. `enum Intent` defined in new §6.2a Types.

### FINDING-4 ✅
- Replaced "Mixed" with "Hybrid" in §5.4 (determinism paragraph), §7.1 cost table, `FusionConfig.rrf_k` comment, and §5.1 signal-source table. Fixed §5.4 cross-ref from §4.4 to §4.7.

### FINDING-5 ✅
- §11 traceability table now includes GUARD-2 (downgrades + RetrievalOutcome + hybrid truncation) and GUARD-8 (write-time affect snapshot, cosine). Footer updated to "14 GOALs + 4 GUARDs".

### FINDING-6 ✅
- §4.7 step 1 now records dropped strong signals in `PlanTrace.hybrid_truncated: Vec<DroppedSignal>`; metric `retrieval_hybrid_truncation_total` added in §8.1; `DroppedSignal` defined in §6.2a.

### FINDING-7 ✅
- Added classifier LLM metrics in §8.1 (`retrieval_classifier_llm_calls_total`, `_tokens_total{direction}`, `_duration_seconds`). `ClassifierTrace.llm_cost: Option<LlmCost>` noted in §6.3 and `LlmCost` defined in §6.2a.

### FINDING-8 ✅
- §9 GUARD-6 property-test rephrased: invariant is `result_ids(Q, S1) ∪ result_ids(Q, S2) == candidate_pool(Q)`, where the candidate pool is the Affective plan's own seed set (§4.5 step 2), not the Associative result set.

### FINDING-9 ✅
- §4.2 step 1 now downgrades to Associative with `RetrievalOutcome::DowngradedFromEpisodic { reason: "no_time_expression" }` when no time expression parses with confidence ≥ 0.5. `DowngradedFromEpisodic` and `DowngradedFromAbstract` added to §6.4 `RetrievalOutcome`.

### FINDING-10 ✅
- §4.5 step 5 now specifies default `affect_divergence_sample_rate = 0.01`; knob added to §7.3; §9 test pins rate ≥ 0.01.

### FINDING-11 ✅
- §5.2 now documents **Missing signal normalization** (proportional renormalization of remaining weights to sum to 1.0), recorded in `FusionTrace`.

### FINDING-12 ✅
- §6.2 added `graph_query_locked` deterministic-mode API using `FusionConfig::locked()`; design notes clarify `graph_query` vs `graph_query_locked`.

### FINDING-13 ✅
- Added §6.2a Types with one-line definitions for `Intent`, `TimeWindow`, `SignalWeightMatrix`, `RetrievalError`, `AffectVector`, `SubScores`, `DroppedSignal`, `LlmCost`, and the trace structs.

### FINDING-14 ✅
- Removed `tie_break: TieBreakOrder` field from `FusionConfig`; §5.4 documents `(memory_id ascending)` as a hard-coded invariant with a note on future extensibility. `TieBreakOrder` consequently not in §6.2a Types.

### FINDING-15 ✅
- §4.2 step 3 now references `τ_graph_filter` (default `0.3`); knob added to §7.3 with justification.

### FINDING-16 ✅
- §4.5 step 2 now states `K_seed_affective = 3 * requested_k` (capped at 60); added to §7.3.

### FINDING-17 ✅
- §9 affect-divergence test now notes metric/threshold live in `benchmark_config.toml` (keys `affect_divergence.metric`, `affect_divergence.threshold`, `affect_divergence.sample_rate`); re-tuning updates config, not design.

### Summary
- Applied: 17/17
- Skipped: 0/17
