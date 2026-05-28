---
title: Cognitive feature E2E coverage matrix — audit which features are production-active vs wired-but-inert vs falsified
status: open
priority: P2
severity: feature
category: observability
created: 2026-05-28
relates: [ISS-148, ISS-159, ISS-164, ISS-175, ISS-177, ISS-180]
depends_on:
---

## Summary

Engram ships ~18 cognitive features per README (memory, ACT-R, Ebbinghaus,
Hebbian/STDP, dual-trace consolidation, emotional bus, interoceptive hub,
synthesis, dimensions, bitemporal, metacognition, etc.). Internal audit
(2026-05-28, potato session) revealed three populations:

1. **Production-active** — wired, tested, and verifiably affecting
   retrieval/generation on every LoCoMo run.
2. **Wired-but-inert** — code present + unit tests pass, but signal is
   read-only or never feeds back into retrieval ranking / generation
   gating. Effectively dead at the system level.
3. **Falsified / unverified at scale** — shipped behind feature flag or
   default-off after A/B sweep failed AC-5a or showed regression.

There is no single artifact tracking which feature is in which bucket.
This makes scope decisions (e.g. "is X the next lever?") prone to
guessing, and makes the README's cognitive-substrate claim non-auditable.

This issue ships a living matrix + decision rule for moving features
between buckets.

## Why now

- Today's session (ISS-175/177/180) burned 8 commits chasing ranker
  fusion variants. ISS-180 corrected mechanism (IDK-frequency at LLM
  generation) revealed entity_channel is recall/precision tradeoff,
  not category-conditional — invisible at score-level.
- ISS-179 census admits best-case lever stack tops at 10-13/27 vs
  target 17/27 on conv-26 SF. Several "missing" levers are *already
  shipped but inert*. Quantifying that bridges ISS-179.
- Four levers (ISS-159 CE / ISS-164 entity_channel / ISS-175 factual
  reweight / ISS-141 HyDE) have been falsified or marked opt-in.
  Without a matrix, falsification history is scattered across issue
  bodies.

## Initial audit (2026-05-28, evidence-cited)

### A. Production-active (default ON, exercised by every LoCoMo run)

| Feature | Code | Tests | E2E evidence |
|---|---|---|---|
| Memory storage + retrieval | `memory.rs`, `storage.rs` | 2011 lib + 79 integration (commit `ea2bf16`) | LoCoMo 152q ingest→query, daily |
| ACT-R activation | `signals.rs::actr_score`, `lifecycle.rs` | unit + fusion integration | Used in fusion every query |
| Ebbinghaus decay | `lifecycle.rs` | `iss103_decay_uses_wallclock_not_occurred_at` | Wallclock-grounded, verified post ISS-103 |
| Hebbian + STDP | `association/` | `iss117_canonical_hebbian` + association_integration_test | Dual-write to edges; spreading activation hits |
| Graph substrate (unified) | `graph/`, `substrate/` | `v04_phase_b_dual_write`, `v04_phase_c_*` (9 files) | T32 cutover (`887dc37`), default ON since 2026-05-23 |
| Embeddings (Ollama nomic) | `embeddings.rs` | `embedding_protocol_v2` | Every LoCoMo ingest + query |
| Entity extraction (LLM) | `extractor.rs`, `entities.rs` | `entity_integration_test`, ISS-176 retry/backoff | conv-26 produces 1300+ entities |
| Hybrid retrieval (8 plans) | `retrieval/plans/` | `v03_retrieval_acceptance_test` | Plan dispatch logged per query |
| Fusion (BM25 + graph + vector + recency + ACT-R) | `fusion/signals.rs`, `combiner.rs` | `multi_signal_integration` | 5 signals wired post ISS-147 / ISS-172 |
| Synthesis (insight clusters) | `synthesis/`, `compiler/` | `synthesis_integration_test`, `kc_integration_test` | knowledge_compile per conv |

### B. Wired-but-inert (signal exists, no feedback into retrieval/generation)

| Feature | Status | Inertness evidence |
|---|---|---|
| Emotional bus | code + `bus_test` + `iss_090_empathy_bus_compat` pass | LoCoMo runs never inject bus state; affective plan reads bus but bus is empty during bench. Not in any A/B sweep. |
| Interoceptive hub | `interoceptive/` + `interoceptive_test` pass; allostatic load computed | Affective plan reads `self_state` from hub but bench never populates (ISS-071). No retrieval / generation consumer. |
| Dimensions (factual/episodic/emotional/...) | `dimensions.rs` + `dimensional_integration_test` + ISS-158 dim threading | Only `abstract_l5` plan uses dimension signal. Other 7 plans ignore dimensions in scoring. |
| Bitemporal plan | `bitemporal.rs` + ISS-087/089 occurred_at round-trip | Plan dispatches but temporal category is consistently the weakest (0.30–0.50 across runs). No bitemporal-specific A/B isolates its lift. |
| Metacognition | `metacognition.rs` + `session_wm.rs` | Read/write paths exist. No e2e test shows a metacog signal triggering behavior change. Not in any LoCoMo loop. |

### C. Falsified or unverified at scale (shipped, default-OFF)

| Feature | Issue | Verdict |
|---|---|---|
| Cross-encoder reranker | ISS-159 | Falsified `b48ba46`. conv-26 single-hop B-A delta = 0 (literal). Multi-hop +10.8pp = its trained regime. Stage C.5 wired, default OFF. |
| Entity channel | ISS-164 | Shipped `77ef3f3`+`ebc9adf`, default OFF. ISS-180 stack-test corrected mechanism: recall/precision tradeoff at generation (IDK 0/7 → 6/7 on losses). Cannot default-ON. |
| Factual reweighting (combine_factual_v2) | ISS-175 / ISS-177 | Shipped `da11171`, default OFF. conv-26 SF NOT met ship gate (5/27 vs 7/27 target). conv-44 corpus-general WIN (+7.3pp overall). Eligible to default-ON pending AC-2 full-LoCoMo. |
| HyDE per_category | ISS-156 | Per-category gating revealed multi-hop -10.81pp on clean substrate. Default OFF. |
| Embedder swap (bge / mxbai) | ISS-157 | Falsified — single-hop unchanged across nomic/bge/mxbai. Stayed on nomic. |
| Extractor prompt v2 | ISS-161 | Falsified L7 (v2 prompt regression in K=10) and L3 (V2 example-driven marginal). Stayed on V1. |

### D. Designed but not implemented (per design v04)

| Feature | Design ref | Status |
|---|---|---|
| WriterSupervisor pattern | §6.2 / §6.9 / T66 | Design doc only, no code |
| WmState cold_start \| warm | §4.13 / §4.14 / §6.1 | Design only |
| Anomaly event persistence (tier 2) | §4.11 | Stub exists, no substrate writer |
| SOUL drive feedback closure | §4.12 | Bus has drive scoring read path; no SOUL.md write-back |
| Prev-turn ExtractionContext | ISS-178 (filed) | Not started |
| Mem0-style semantic UPDATE | ISS-163 (filed) | Not started |

## Decision rule for bucket transitions

A feature moves from B (wired-but-inert) → A (production-active) only
when:

1. There exists a LoCoMo A/B (or equivalent in-distribution bench)
   where the feature ON vs OFF produces a measurable, signed delta
   on at least one category, AND
2. The delta is not solely judge-wobble (validated by per-prediction
   text inspection per ISS-180 pattern), AND
3. The integration code is exercised by a default-ON pipeline path
   (not feature-flagged off by default).

A feature moves from C (falsified) back to candidate-pool only when a
new mechanism explanation is filed (issue) AND that mechanism is
testable AND prior falsification setup is shown to be inadequate. See
ISS-180 addendum on "ISS-159 retest in widened-recall setup" — that
reasoning is testable but NOT auto-approved by this rule; it requires
its own decision before re-running.

## Acceptance criteria

- [ ] AC-1: This matrix is committed to the issue body and linked from
  README "Cognitive substrate" section.
- [ ] AC-2: For each B-bucket feature, an issue exists describing what
  it would take to make the feature active (consumer wiring +
  bench harness change). Link from this issue.
- [ ] AC-3: For each C-bucket feature, the falsification commit is
  pinned in the matrix row (already done above).
- [ ] AC-4: A 1-line CI check (or a script under `.gid/scripts/`)
  that fails when README mentions a feature not in bucket A.
  Prevents drift between marketing copy and reality.
- [ ] AC-5: Matrix refreshed within 7 days after any production-code
  PR that adds or moves a cognitive feature. (Ownership convention
  not enforcement.)

## Scope this round

This issue is **planning-only** for now. The matrix above is the
deliverable. Promoting features B → A or removing C-bucket dead code
is *out of scope* for this issue — each transition gets its own issue.

## Out of scope

- Implementing emotional bus consumer in retrieval (separate issue)
- Wiring interoceptive hub into LoCoMo replay (ISS-071, already filed)
- Removing falsified-but-shipped code (separate cleanup pass)
- Adding any cross-encoder retest (would need its own decision)

## Evidence cited (cite-before-claim)

All entries above were verified 2026-05-28 against:

- `git log --oneline -50` on engram main
- `grep -rn "unified_substrate" crates/engramai/src/config.rs`
- `ls crates/engramai/src/` and `crates/engramai/src/retrieval/`
- `ls crates/engramai/tests/` (79 integration files enumerated)
- `gid_artifact_show` for ISS-149, ISS-159, ISS-164, ISS-175, ISS-177,
  ISS-180
- ISS-176 `2011 lib tests pass` line cited from issue body
  (commit `ea2bf16` baseline, 2026-05-28)

No claim above is from training-data reconstruction; each row has a
tool-verified anchor in the current session.
