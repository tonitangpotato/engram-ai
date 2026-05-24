#!/usr/bin/env bash
# ISS-153 HyDE re-test on POST-FIX substrate (ISS-155 fae6bb7 applied).
#
# Pre-fix comparison (already run):
#   ISS-152 Run A (no HyDE, K=10, MMR 0.7):  overall=0.3618  multi-hop=0.3243
#   ISS-153 (HyDE on, K=10, MMR 0.7):        overall=0.3947  multi-hop=0.2432
#   Δ overall: +3.29pp  Δ multi-hop: -8.1pp (REGRESSION)
#
# Question: is the -8.1pp multi-hop regression real signal, or did it live
# inside the ~0.66pp pre-fix wobble envelope + paraphrase-cluster noise?
#
# Now that ISS-155 is in (mean substrate -1.75pp, stdev 0.66pp → 0.38pp,
# 3 of 4 categories byte-identical across runs), we re-run BOTH arms
# back-to-back on the same post-fix binary and compare cleanly.

set -u
ROOT=/Users/potato/clawd/projects/engram-bench
BIN="$ROOT/target/release/engram-bench"

cd "$ROOT"

export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=10
export ENGRAM_BENCH_MMR_LAMBDA=0.7

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
SUMMARY=/tmp/iss153-retest-summary-${STAMP}.txt
> "$SUMMARY"

run_arm() {
  local label="$1"        # A or B
  local hyde_flag="$2"    # "" or "1"
  local LOG=/tmp/iss153-retest-${label}.log

  echo "========================================================"
  echo " ISS-153 HyDE re-test ARM ${label}  HyDE=${hyde_flag:-off}"
  echo "  K=10 MMR=0.7 conv-26 post-fix-substrate"
  echo "  started: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  echo "========================================================"

  if [ -n "$hyde_flag" ]; then
    ENGRAM_BENCH_HYDE="$hyde_flag" "$BIN" locomo > "$LOG" 2>&1
  else
    unset ENGRAM_BENCH_HYDE
    "$BIN" locomo > "$LOG" 2>&1
  fi
  local rc=$?
  echo "[arm ${label}] exit=${rc}  finished: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"

  local newest=$(ls -dt "$ROOT/benchmarks/runs/"2026-*_locomo 2>/dev/null | head -1)
  if [ -n "$newest" ] && [ -d "$newest" ]; then
    local target="$ROOT/benchmarks/runs/ISS153-retest-${label}-${hyde_flag:+hyde-}k10-conv26-${STAMP}"
    mv "$newest" "$target"
    local SCORE=$(python3 -c "import json; d=json.load(open('${target}/locomo_summary.json')); print(d['overall'])")
    echo "[arm ${label}] dir=${target}  overall=${SCORE}"
    echo "arm${label}  hyde=${hyde_flag:-off}  ${SCORE}  ${target}" >> "$SUMMARY"
  else
    echo "[arm ${label}] WARNING: no fresh run dir"
    echo "arm${label}  hyde=${hyde_flag:-off}  FAIL  no_dir" >> "$SUMMARY"
  fi
}

run_arm A ""        # baseline: K=10, MMR 0.7, no HyDE
run_arm B "1"       # HyDE on

echo
echo "==== SUMMARY ===="
cat "$SUMMARY"
echo
python3 << 'PY'
import json, glob
def load(label):
    fn = sorted(glob.glob(f'/Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS153-retest-{label}-*conv26-*/locomo_summary.json'))
    if fn:
        return json.load(open(fn[-1]))
    return None
a = load("A")
b = load("B")
if a and b:
    print(f'Arm A (no HyDE):  {json.dumps(a, indent=2)}')
    print(f'Arm B (HyDE on):  {json.dumps(b, indent=2)}')
    print('\n=== Δ (HyDE on - HyDE off, POST-FIX substrate) ===')
    print(f'overall:     {b["overall"]:.4f} - {a["overall"]:.4f} = {(b["overall"]-a["overall"])*100:+.2f}pp')
    for c in sorted(a['by_category']):
        da = a['by_category'][c]
        db = b['by_category'][c]
        print(f'{c:14s} {db:.4f} - {da:.4f} = {(db-da)*100:+.2f}pp')
    print('\n=== Compare against pre-fix ===')
    print(f'pre-fix  A=0.3618 B=0.3947  Δoverall=+3.29pp  Δmulti-hop=-8.11pp')
    print(f'post-fix A={a["overall"]:.4f} B={b["overall"]:.4f}  Δoverall={(b["overall"]-a["overall"])*100:+.2f}pp  Δmulti-hop={(b["by_category"]["multi-hop"]-a["by_category"]["multi-hop"])*100:+.2f}pp')
PY
