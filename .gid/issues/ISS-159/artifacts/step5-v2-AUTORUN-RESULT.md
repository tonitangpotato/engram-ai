# ISS-159 Step 5 v2 Autorun Result

Generated: 2026-05-26T05:09:51Z
STAMP: 20260526T040634Z

## Artifact check

- Arm A OK: /Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS159v2-A-conv26-20260526T040634Z/locomo_summary.json
- Arm B OK: /Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS159v2-B-conv26-20260526T040634Z/locomo_summary.json
- Arm C MISSING: /Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS159v2-C-conv26-20260526T040634Z/locomo_summary.json

## Per-arm summaries

### Arm A
```json
{
  "overall": 0.34868421052631576,
  "by_category": {
    "multi-hop": 0.32432432432432434,
    "open-domain": 0.15384615384615385,
    "single-hop": 0.1875,
    "temporal": 0.4714285714285714
  },
  "n_queries": 152
}
```

### Arm B
```json
{
  "overall": 0.3618421052631579,
  "by_category": {
    "multi-hop": 0.43243243243243246,
    "open-domain": 0.15384615384615385,
    "single-hop": 0.1875,
    "temporal": 0.44285714285714284
  },
  "n_queries": 152
}
```


## Comparative table

```
Arm   overall     single-hop    multi-hop    open      temporal  
-----------------------------------------------------------------
ref   0.4605      0.2188        0.5405       0.5385    0.5143      (ISS-157-A baseline)
A     0.3487      0.1875        0.3243       0.1538    0.4714    
B     0.3618      0.1875        0.4324       0.1538    0.4429    
C     MISSING

Best single-hop: Arm A @ 0.1875
❌ AC-5a FAIL (gap = 0.4125, best = 0.1875)
```

## Caveat: stochastic baseline drift

Arm A is **not** expected to bit-reproduce ISS-157-A despite identical
retrieval config — Anthropic Haiku extractor is non-deterministic even
at temp=0 (known ISS-155-class issue). Internal A/B/C comparison within
this sweep is the valid signal; cross-sweep baseline comparison is noisy.
Empirical drift observed in v1 run (2026-05-26T03:56:56Z): overall
0.4605 → 0.3618 (-9.9pp) under nominally identical config.

Decision rule: judge CE by **Arm B − Arm A** delta on single-hop within
this sweep, not by absolute Arm B vs historical 0.2188.
