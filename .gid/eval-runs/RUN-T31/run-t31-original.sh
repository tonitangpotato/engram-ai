#!/bin/bash
# T31 LoCoMo unified-vs-legacy parity campaign.
# Runs both arms back-to-back; writes runs into engram-bench/benchmarks/runs/
# under T31-{legacy,unified}-<TS> directories.

set -euo pipefail

cd /Users/potato/clawd/projects/engram-bench

export PATH="$HOME/.cargo/bin:$PATH"
# Keep stdout/stderr in one file per arm; status visible via tail.

TS=$(date -u +%Y%m%dT%H%M%SZ)
LEGACY_OUT="benchmarks/runs/T31-legacy-${TS}"
UNIFIED_OUT="benchmarks/runs/T31-unified-${TS}"
mkdir -p "$LEGACY_OUT" "$UNIFIED_OUT"

LOG="/tmp/t31-driver.log"
echo "T31 LoCoMo parity campaign — $(date -u +%FT%TZ)" | tee "$LOG"
echo "legacy out: $LEGACY_OUT" | tee -a "$LOG"
echo "unified out: $UNIFIED_OUT" | tee -a "$LOG"
echo | tee -a "$LOG"

# arm A: legacy (unified_substrate=false, default)
echo "=== arm A: legacy ===" | tee -a "$LOG"
unset ENGRAM_BENCH_UNIFIED_SUBSTRATE
./target/release/engram-bench --output-dir "$LEGACY_OUT" locomo --format json \
    >> "$LOG" 2>&1
echo "arm A done at $(date -u +%FT%TZ)" | tee -a "$LOG"

# arm B: unified
echo | tee -a "$LOG"
echo "=== arm B: unified ===" | tee -a "$LOG"
export ENGRAM_BENCH_UNIFIED_SUBSTRATE=1
./target/release/engram-bench --output-dir "$UNIFIED_OUT" locomo --format json \
    >> "$LOG" 2>&1
echo "arm B done at $(date -u +%FT%TZ)" | tee -a "$LOG"

echo | tee -a "$LOG"
echo "=== compare ===" | tee -a "$LOG"
for d in "$LEGACY_OUT"/* "$UNIFIED_OUT"/*; do
    if [ -f "$d/locomo_summary.json" ]; then
        echo "$d" | tee -a "$LOG"
        cat "$d/locomo_summary.json" | tee -a "$LOG"
        echo | tee -a "$LOG"
    fi
done

echo "T31 done $(date -u +%FT%TZ)" | tee -a "$LOG"
