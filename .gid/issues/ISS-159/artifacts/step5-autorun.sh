#!/bin/bash
# ISS-159 Step 5 auto-finisher.
# Runs in the background, polls every 60s for sweep completion,
# then emits analysis + AC-5a verdict to /tmp/iss159-step5/AUTORUN-RESULT.md.
# Idempotent: safe to invoke multiple times (writes to fixed path).
set -u

STAMP=20260526T032648Z
RUNS=/Users/potato/clawd/projects/engram-bench/benchmarks/runs
LOG_DIR=/tmp/iss159-step5
RESULT=$LOG_DIR/AUTORUN-RESULT.md
ANALYSER=/tmp/iss159_step5_analyse.py

# Single-flight guard (mkdir is atomic on POSIX; macOS has no flock).
LOCK=$LOG_DIR/autorun.lockdir
if ! mkdir "$LOCK" 2>/dev/null; then
    # If lock is stale (>4h old) clean it; else bail.
    if [ -d "$LOCK" ]; then
        AGE=$(($(date +%s) - $(stat -f %m "$LOCK" 2>/dev/null || echo 0)))
        if [ $AGE -gt 14400 ]; then
            rm -rf "$LOCK"
            mkdir "$LOCK" 2>/dev/null || { echo "[autorun] cannot acquire lock" >&2; exit 0; }
        else
            echo "[autorun] another instance already running (lock age ${AGE}s); exiting" >&2
            exit 0
        fi
    fi
fi
trap 'rmdir "$LOCK" 2>/dev/null' EXIT

echo "[autorun] started at $(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$RESULT.in_progress"

# Poll until sweep done or 3h hard timeout.
START=$(date +%s)
MAX=$((3 * 60 * 60))   # 3 hours hard cap

while true; do
    ELAPSED=$(($(date +%s) - START))
    if [ $ELAPSED -gt $MAX ]; then
        echo "[autorun] TIMEOUT after 3h, bailing" >> "$RESULT.in_progress"
        mv "$RESULT.in_progress" "$RESULT"
        exit 1
    fi

    # Done condition: no engram-bench process AND no sweep script process.
    BENCH_PROCS=$(pgrep -f "target/release/engram-bench" | wc -l | tr -d ' ')
    SWEEP_PROCS=$(pgrep -f "iss159_step5_sweep.sh" | wc -l | tr -d ' ')
    if [ "$BENCH_PROCS" = "0" ] && [ "$SWEEP_PROCS" = "0" ]; then
        break
    fi

    sleep 60
done

# Sweep finished. Verify all 3 summary files exist.
{
    echo "# ISS-159 Step 5 Autorun Result"
    echo ""
    echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "STAMP: $STAMP"
    echo ""
    echo "## Artifact check"
    echo ""
    for arm in A B C; do
        f=$RUNS/ISS159-${arm}-conv26-${STAMP}/locomo_summary.json
        if [ -f "$f" ]; then
            echo "- Arm $arm OK: $f"
        else
            echo "- Arm $arm MISSING: $f"
        fi
    done
    echo ""
    echo "## Per-arm summaries"
    echo ""
    for arm in A B C; do
        f=$RUNS/ISS159-${arm}-conv26-${STAMP}/locomo_summary.json
        if [ -f "$f" ]; then
            echo "### Arm $arm"
            echo '```json'
            cat "$f"
            echo '```'
            echo ""
        fi
    done
    echo ""
    echo "## Comparative analysis"
    echo ""
    echo '```'
    if [ -x "$(command -v python3)" ] && [ -f "$ANALYSER" ]; then
        python3 "$ANALYSER" "$STAMP" 2>&1 || echo "(analyser failed; raw json above is source of truth)"
    else
        echo "(analyser missing — read raw json blocks above)"
    fi
    echo '```'
    echo ""
    echo "## Decision tree status"
    echo ""
    # Extract single-hop from each summary, compute max
    BEST_SH=$(
        for arm in A B C; do
            f=$RUNS/ISS159-${arm}-conv26-${STAMP}/locomo_summary.json
            if [ -f "$f" ]; then
                python3 -c "import json; print(json.load(open('$f'))['by_category']['single-hop'])" 2>/dev/null
            fi
        done | sort -nr | head -1
    )
    echo "Best single-hop across A/B/C: $BEST_SH"
    if [ -n "$BEST_SH" ]; then
        PASS=$(python3 -c "print('1' if float('$BEST_SH') >= 0.60 else '0')" 2>/dev/null)
        if [ "$PASS" = "1" ]; then
            echo ""
            echo "✅ AC-5a PASS (single-hop ≥0.60)."
            echo "Next: pin winning summary.json to .gid/issues/ISS-159/artifacts/,"
            echo "      write ISS-159 acceptance note, kick conv-44 cross-check."
        else
            GAP=$(python3 -c "print(f'{0.60 - float(\"$BEST_SH\"):.4f}')" 2>/dev/null)
            echo ""
            echo "❌ AC-5a FAIL. Gap from 0.60: $GAP"
            echo ""
            BIG=$(python3 -c "print('1' if float('$BEST_SH') < 0.50 else '0')" 2>/dev/null)
            if [ "$BIG" = "1" ]; then
                echo "Gap >10pp — escalate path:"
                echo "  - Store falsification memory at importance=0.9"
                echo "  - D2 fallback: swap to bge-reranker-base"
                echo "  - Probe upstream: ISS-149 classifier, ISS-157 embedder"
            else
                echo "Gap <10pp — pool widening path:"
                echo "  - Step 6: ship fusion_pool_size_override knob"
                echo "  - Re-sweep with k_in=50, pool=100"
            fi
        fi
    fi
} > "$RESULT.in_progress"

mv "$RESULT.in_progress" "$RESULT"

# Best-effort Telegram-style breadcrumb in case I'm awake when this lands.
echo "[autorun] DONE — see $RESULT" >> "$LOG_DIR/master.log"
