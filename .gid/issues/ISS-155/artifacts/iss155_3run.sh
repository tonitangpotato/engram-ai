#!/usr/bin/env bash
# ISS-155 Phase 2 empirical validation: 3x LoCoMo conv-26 K=5 temp=0 with
# extractor temp=0 fix (engram fae6bb7) applied.
#
# Baseline: ISS-137 3 runs at 0.4013 / 0.4079 / 0.3947, stdev 0.66pp.
# Question: does extractor wobble reduction lower inter-run stdev further?
# (Or do those 3 ISS-137 runs already represent the LLM-judge floor?)
#
# Why K=5: matches ISS-137 era (pre-ISS-138 DEFAULT_TOP_K change).

set -u
ROOT=/Users/potato/clawd/projects/engram-bench
BIN="$ROOT/target/release/engram-bench"

cd "$ROOT"

unset ENGRAM_BENCH_MMR_LAMBDA
export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=5

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
SUMMARY=/tmp/iss155-3run-summary-${STAMP}.txt
> "$SUMMARY"

for run in 1 2 3; do
  echo "========================================================"
  echo " ISS-155 Phase 2 empirical run ${run}/3  K=5 conv-26 temp=0"
  echo " started: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  echo "========================================================"
  LOG=/tmp/iss155-3run-${run}.log
  "$BIN" locomo > "$LOG" 2>&1
  rc=$?
  echo "[run ${run}] exit=${rc}  finished: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"

  # rename run dir
  newest=$(ls -dt "$ROOT/benchmarks/runs/"2026-*_locomo 2>/dev/null | head -1)
  if [ -n "$newest" ] && [ -d "$newest" ]; then
    target="$ROOT/benchmarks/runs/ISS155-Phase2-run${run}-k5-conv26-${STAMP}"
    mv "$newest" "$target"
    SCORE=$(python3 -c "import json; d=json.load(open('${target}/locomo_summary.json')); print(d['overall'])")
    echo "[run ${run}] dir=${target}  overall=${SCORE}"
    echo "run${run}  ${SCORE}  ${target}" >> "$SUMMARY"
  else
    echo "[run ${run}] WARNING: no fresh run dir found"
    echo "run${run}  FAIL  no_dir" >> "$SUMMARY"
  fi
done

echo
echo "==== SUMMARY ===="
cat "$SUMMARY"
echo
python3 << 'PY'
import json, statistics, glob
scores = []
for run in 1, 2, 3:
    fn = sorted(glob.glob(f'/Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS155-Phase2-run{run}-k5-conv26-*/locomo_summary.json'))
    if fn:
        d = json.load(open(fn[-1]))
        scores.append(d['overall'])
        print(f'run{run}: overall={d["overall"]:.4f}  by_cat={d["by_category"]}')
if len(scores) >= 2:
    mean = statistics.mean(scores)
    stdev = statistics.stdev(scores)
    print(f'\nmean={mean:.4f}  stdev={stdev:.4f} ({stdev*100:.2f}pp)')
    print(f'ISS-137 baseline (pre-extractor-fix): mean=0.4013  stdev=0.0066 (0.66pp)')
PY
