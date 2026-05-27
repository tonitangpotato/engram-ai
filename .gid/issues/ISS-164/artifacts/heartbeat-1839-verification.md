# ISS-164 1835 verdict — independent verification (RustClaw2 heartbeat 18:39 EDT)

**TL;DR: Confirmed.** Re-ran the per-query diff from raw `locomo_per_query.jsonl` files. Every number in `heartbeat-1835-phase2-verdict.md` matches exactly. Trader's verdict stands: entity channel is NET NEGATIVE (-3.3pp overall, -4 multi-hop, -3 temporal), and zero of the 9 stubborn single-fact questions flipped.

Note on authorship: the 1835 verdict file is signed "RustClaw2" but was written 18:37 by what's almost certainly Trader (only Trader had context on K=10/HYDE=off bench + the n=27 classification). This is a cross-instance identity slip, not malicious — but I'm filing this verification under my own name to keep the audit trail clean.

## Verification: ran python diff on raw jsonl files

Both arms `locomo_per_query.jsonl` (152 rows each) loaded and joined by `id`.

| Bucket | n | A-correct | B-correct | Δ | Trader's claim | match? |
|---|---|---|---|---|---|---|
| single-hop | 32 | 6.0 | 8.0 | +2 | +2 | ✅ |
| multi-hop | 37 | 17.0 | 13.0 | -4 | -4 | ✅ |
| temporal | 70 | 33.0 | 30.0 | -3 | -3 | ✅ |
| open-domain | 13 | 4.0 | 4.0 | 0 | 0 | ✅ |
| overall | 152 | 60 | 55 | -5 | -3.3pp | ✅ |

## Single-hop flip attribution

Only 2 questions flipped, both UP, both LIST-style golds:

- **q32** gold="Pride parade, school speech, support group" — B retrieved the activity list
- **q39** gold="Joining activist group, going to pride parades, participating in an art show, mentoring program" — B retrieved a 4-item activist participation list

Both flipped 0→1 score. **Zero flips down** in single-hop.

## 9 stubborn single-fact questions

ALL still 0 in both arms (n=27 single-fact bucket unchanged by entity channel):

```
q3  [single-hop] "Adoption agencies"     A=0 B=0
q7  [single-hop] "Single"                A=0 B=0
q11 [single-hop] "Sweden"                A=0 B=0
q37 [single-hop] "sunset"                A=0 B=0
q40 [single-hop] "2"                     A=0 B=0
q43 [single-hop] "abstract art"          A=0 B=0
q71 [single-hop] "Becoming Nicole"       A=0 B=0
q75 [single-hop] "3"                     A=0 B=0
q76 [single-hop] "19 October 2023"       A=0 B=0
```

## Implication

Per the sweep script's own decision rule (`/tmp/iss164_bench_sweep.sh` header):

> B sf - A sf < 0 → channel actively hurts; revert Phase 1 commits 77ef3f3 + ebc9adf; file degradation root-cause; reassess weapon plan.

The actual single-fact (n=27) delta is **0** (not negative — same 0/27 in stubborn questions, no detectable change among the other 18 either since single-hop changes were entirely LIST). But the overall multi-hop -4 / temporal -3 regression is decisive: the entity channel is making the retrieval pool worse for hop questions.

**My read** (this is paper-only — not asking for action): the Phase 1 implementation is doing more damage in multi-hop than the morning sweep showed, and the +2 single-hop "wins" cannot be attributed to entity channel because they're in the noisy LIST bucket where ±60% drift is normal (per heartbeat-1438-baseline-drift-caveat.md).

Verdict for potato: **revert + investigate root cause before next iteration**, exactly as Trader concluded. Do not flip `FusionConfig::locked` default.

## Files inspected

- /Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS164-A-conv26-20260526T213218Z/locomo_per_query.jsonl
- /Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS164-B-conv26-20260526T213218Z/locomo_per_query.jsonl
- /tmp/iss164-bench/master.log
- /Users/potato/clawd/projects/engram/.gid/issues/ISS-164/artifacts/heartbeat-1835-phase2-verdict.md (the Trader verdict)
