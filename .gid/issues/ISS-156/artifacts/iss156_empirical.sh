#!/usr/bin/env bash
# ISS-156 empirical: per_category HyDE gating on conv-26 K=10 MMR 0.7.
#
# Compare against the two arms from the ISS-153 post-fix retest:
#   Arm A (no HyDE):     overall=0.4605  multi=0.5405  open=0.3846  single=0.2188
#   Arm B (HyDE all):    overall=0.4539  multi=0.4324  open=0.5385  single=0.2500
#
# Target (AC #4):
#   - overall ≥ 0.4605 (no regression vs no-HyDE)
#   - multi-hop within 2pp of 0.5405
#   - open-domain ≥ 0.4846 (no-HyDE + 10pp)

set -u
ROOT=/Users/potato/clawd/projects/engram-bench
BIN="$ROOT/target/release/engram-bench"

cd "$ROOT"

export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=10
export ENGRAM_BENCH_MMR_LAMBDA=0.7
export ENGRAM_BENCH_HYDE=per_category

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
LOG=/tmp/iss156-pc-${STAMP}.log

echo "ISS-156 per_category HyDE empirical  K=10 MMR=0.7 conv-26"
echo "started: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
"$BIN" locomo > "$LOG" 2>&1
rc=$?
echo "exit=${rc}  finished: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"

newest=$(ls -dt "$ROOT/benchmarks/runs/"2026-*_locomo 2>/dev/null | head -1)
if [ -n "$newest" ] && [ -d "$newest" ]; then
  target="$ROOT/benchmarks/runs/ISS156-pc-conv26-${STAMP}"
  mv "$newest" "$target"
  echo "dir=${target}"
  python3 << PY
import json
d = json.load(open("$target/locomo_summary.json"))
print(json.dumps(d, indent=2))
print()
print("=== vs ISS-153 retest baselines ===")
print(f"           per_cat   Arm A (off)   Arm B (all)   Δ vs A")
print(f"overall    {d['overall']:.4f}    0.4605        0.4539        {(d['overall']-0.4605)*100:+.2f}pp")
for c, target_a, target_b in [("multi-hop", 0.5405, 0.4324), ("open-domain", 0.3846, 0.5385), ("single-hop", 0.2188, 0.2500), ("temporal", 0.5429, 0.5429)]:
    v = d['by_category'][c]
    print(f"{c:10s} {v:.4f}    {target_a:.4f}        {target_b:.4f}        {(v-target_a)*100:+.2f}pp")
print()
print("=== AC #4 gates ===")
print(f"AC #4a overall ≥ 0.4605:       {'PASS' if d['overall'] >= 0.4605 else 'FAIL'}   ({d['overall']:.4f})")
mh = d['by_category']['multi-hop']
print(f"AC #4b multi-hop ≥ 0.5205:     {'PASS' if mh >= 0.5205 else 'FAIL'}   ({mh:.4f}, target 0.5405±2pp)")
od = d['by_category']['open-domain']
print(f"AC #4c open-domain ≥ 0.4846:   {'PASS' if od >= 0.4846 else 'FAIL'}   ({od:.4f}, target 0.3846 + 10pp)")
PY
fi
