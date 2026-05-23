#!/bin/bash
# T31 unified arm only — legacy arm already done in T31-legacy-20260523T010504Z.
# No `set -e` because engram-bench exits non-zero on gate FAIL (P0 LoCoMo
# overall ≥ 68.5%), which is the very thing we're measuring.

cd /Users/potato/clawd/projects/engram-bench
export PATH="$HOME/.cargo/bin:$PATH"

OUT="benchmarks/runs/T31-unified-20260523T010504Z"
LOG="/tmp/t31-unified.log"

mkdir -p "$OUT"
echo "T31 unified arm — $(date -u +%FT%TZ)" | tee "$LOG"
echo "out: $OUT" | tee -a "$LOG"

export ENGRAM_BENCH_UNIFIED_SUBSTRATE=1
./target/release/engram-bench --output-dir "$OUT" locomo --format json \
    >> "$LOG" 2>&1
echo "unified arm exit=$? at $(date -u +%FT%TZ)" | tee -a "$LOG"

# locate the latest summary
LATEST=$(ls -dt "$OUT"/*_locomo 2>/dev/null | head -1)
if [ -n "$LATEST" ] && [ -f "$LATEST/locomo_summary.json" ]; then
    echo "--- unified summary ---" | tee -a "$LOG"
    cat "$LATEST/locomo_summary.json" | tee -a "$LOG"
fi
