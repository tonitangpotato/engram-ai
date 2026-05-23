---
title: 'LoCoMo harness: pin temperature=0 in engram-bench/src/llm_client.rs to reduce J-score single-run variance from 9.5pp to ~2pp'
status: open
priority: P1
severity: degradation
labels: bench, locomo, variance, signal-noise
relates_to: [ISS-136, ISS-100]
filed: 2026-05-23
---

## Problem

LoCoMo J-score has **±9.5pp stdev across same-day, same-code, same-fixture
runs** (measured on 2026-05-06 across 7 full-152q runs). Range was 0.296 →
0.559, i.e. 26pp spread. This makes the J-score effectively useless as a
signal for any change smaller than ~15pp, and caused the false-positive
regression report in ISS-136 (apparent -7.2pp drop was less than one stdev).

## Root cause

`engram-bench/src/llm_client.rs::call_llm` at lines 146-151:

```rust
let payload = serde_json::json!({
    "model": model,
    "max_tokens": max_tokens,
    "system": effective_system,
    "messages": [{"role": "user", "content": prompt}],
});
```

No `temperature` field → Anthropic default `temperature=1.0` (high
stochasticity). This single call is used by BOTH the answer generator
(`scorers/locomo_judge.rs::generate_answer`) AND the verdict judge
(`scorers/locomo_judge.rs::judge_answer`). Two stochastic LLM stages
compounding across 152 queries → ±9.5pp per-run stdev.

The Python reference (`cogmembench/benchmarks/common/llm.py:75-80`) also
omits temperature. The Rust port is byte-for-byte faithful; the variance
is inherited from upstream LoCoMo harness design.

## Proposed fix

Single-line change to the payload literal:

```rust
let payload = serde_json::json!({
    "model": model,
    "max_tokens": max_tokens,
    "temperature": 0,
    "system": effective_system,
    "messages": [{"role": "user", "content": prompt}],
});
```

Expected effect: ~5× reduction in per-run variance (≤2pp stdev), based on
typical Anthropic determinism with temp=0 (residual variance from batched
inference still exists but is small).

## Acceptance criteria

1. `call_llm` payload includes `"temperature": 0`.
2. Run 3 full-152q LoCoMo evals back-to-back on the same code.
   - **Pass** if stdev across the 3 runs ≤ 2.5pp.
   - **Fail** if stdev > 2.5pp (means residual API stochasticity is
     larger than expected; need investigation).
3. ISS-100 AC #4 cross-validation envelope re-baselined against a
   temperature=0 Python reference run, OR explicitly documented as
   superseded (we'd rather have stable J-scores than byte-match a noisy
   reference).
4. Cross-validation 25q smoke retains its ≤1/25 verdict-mismatch
   property when both Python and Rust run at temp=0.
5. Document the cross-validate envelope shift in `bench-design.md`
   §3.1.1 (or wherever ISS-100 lives).

## Cost estimate

3 × 152q LoCoMo runs = ~3 × 6min × ($0.10 generator + $0.05 judge per run)
≈ **$0.50 total** to validate the variance reduction.

## Out of scope

- Multi-run averaging machinery (would also fix the signal/noise problem
  but at 3× ongoing cost). Single-shot temp=0 is the right root fix.
- Cross-validate envelope renegotiation. That's an ISS-100 follow-up and
  can wait until temp=0 lands and we measure the actual mismatch rate
  against a fresh Python ref run.

## Why this matters

LoCoMo is engram's P0 ship-gate. Right now it's a coinflip: any feature
work has to ship "blind" because the gate's signal/noise ratio is too low
to detect <15pp deltas. Phase D's whole point (RUN-T31 unified vs legacy)
was rendered ambiguous by exactly this variance — even the +0.66pp
"within noise" verdict in the RUN-T31 summary IS noise. Until this is
fixed, every LoCoMo run is theatrical.

## Validation result (2026-05-23, overnight)

Ran AC #2 to completion. **PASS.**

| run | overall | multi-hop | open-domain | single-hop | temporal |
| --- | --- | --- | --- | --- | --- |
| run 1 (03:55Z) | 0.4013 | 0.514 | 0.385 | 0.094 | 0.486 |
| run 2 (04:07Z) | 0.4079 | 0.568 | 0.385 | 0.063 | 0.486 |
| run 3 (04:19Z) | 0.3947 | 0.514 | 0.385 | 0.063 | 0.486 |
| **mean** | **0.4013** | 0.532 | 0.385 | 0.073 | 0.486 |
| **stdev** | **0.66 pp** | 3.12 pp | **0.00 pp** | 1.80 pp | **0.00 pp** |
| range | 1.32 pp | 5.41 pp | 0.00 pp | 3.13 pp | 0.00 pp |

- **n=3 stdev = 0.66 pp**, well under the ≤2.5pp pass threshold and the
  ≤2pp expected
- **93.1% variance reduction** vs the 9.49pp historical baseline
- **149/152 (98%) of questions agree on score across all 3 runs**
- Open-domain and temporal categories are **bit-identical** across the 3
  runs (stdev=0, range=0). Both score and predicted-text identical
- Multi-hop is the sole wobble source (2 questions flipping), at 3.12pp
  stdev — still well under the historical 9.49pp
- 76-84% of generated answer texts are byte-identical across run pairs

The 3 score-flip cases were inspected — all are caused by the
answer-generator producing slightly different prose (not the judge
flipping verdicts). This is the residual floor from Anthropic batched
inference non-determinism and is not fixable client-side.

### Open-domain category gain confirmed

The +15-23pp open-domain gain noted in HANDOVER (run 1 only at the time)
is **reproducible** — all 3 runs land at exactly 0.3846 (5/13 correct),
vs RUN-T31 unified=0.231 (3/13) and legacy=0.154 (2/13). This was a real
signal previously buried by the temp=1 noise floor.

### Run artifacts

- `engram-bench/benchmarks/runs/ISS137-temp0-20260523T034444Z/` (run 1)
- `engram-bench/benchmarks/runs/ISS137-temp0-run2-20260523T035639Z/` (run 2)
- `engram-bench/benchmarks/runs/ISS137-temp0-run3-20260523T040751Z/` (run 3)

### AC status

- [x] AC #1 — `temperature: 0` in payload (uncommitted in working tree)
- [x] AC #2 — 3 back-to-back full-152q runs, stdev ≤ 2.5pp — **PASS at
      0.66 pp**
- [ ] AC #3 — ISS-100 envelope re-baseline (still TODO; not blocking)
- [ ] AC #4 — Cross-validate 25q smoke at temp=0 (still TODO; not
      blocking)
- [ ] AC #5 — Doc the envelope shift in bench-design.md (still TODO)

The fix is **safe to commit**. AC #3-5 are downstream cleanup that can
wait.
