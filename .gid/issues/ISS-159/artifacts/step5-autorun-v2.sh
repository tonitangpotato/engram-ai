#!/bin/bash
# ISS-159 Step 5 v2 auto-finisher.
# - STAMP read from /tmp/iss159-step5-v2/STAMP (written by sweep at start)
# - Output target dirs: ISS159v2-{A,B,C}-conv26-${STAMP}
# - Polls every 60s for sweep completion, then emits comparative analysis.
# - Idempotent via mkdir lockdir; 3h hard timeout.
set -u

LOG_DIR=/tmp/iss159-step5-v2
RESULT=$LOG_DIR/AUTORUN-RESULT.md
ANALYSER=/tmp/iss159_step5_analyse_v2.py
RUNS=/Users/potato/clawd/projects/engram-bench/benchmarks/runs

mkdir -p "$LOG_DIR"

# mkdir lock (POSIX atomic, macOS-safe)
LOCK=$LOG_DIR/autorun.lockdir
if ! mkdir "$LOCK" 2>/dev/null; then
    AGE=$(($(date +%s) - $(stat -f %m "$LOCK" 2>/dev/null || echo 0)))
    if [ $AGE -gt 14400 ]; then
        rm -rf "$LOCK"
        mkdir "$LOCK" 2>/dev/null || { echo "[autorun-v2] cannot acquire lock" >&2; exit 0; }
    else
        echo "[autorun-v2] another instance already running (lock age ${AGE}s); exiting" >&2
        exit 0
    fi
fi
trap 'rmdir "$LOCK" 2>/dev/null' EXIT

START=$(date +%s)
MAX=$((4 * 60 * 60))   # 4h cap (3 arms could be slow)

# Wait for STAMP file (sweep writes it almost immediately).
while [ ! -f "$LOG_DIR/STAMP" ]; do
    if [ $(($(date +%s) - START)) -gt 600 ]; then
        echo "# autorun-v2 ABORT: STAMP file never appeared" > "$RESULT"
        exit 1
    fi
    sleep 5
done
STAMP=$(cat "$LOG_DIR/STAMP")
echo "[autorun-v2] STAMP=$STAMP" > "$LOG_DIR/autorun.progress"

# Poll for sweep completion (no engram-bench AND no sweep_v2 script).
while true; do
    if [ $(($(date +%s) - START)) -gt $MAX ]; then
        echo "[autorun-v2] TIMEOUT after 4h" >> "$LOG_DIR/autorun.progress"
        break
    fi
    BENCH=$(pgrep -f "target/release/engram-bench" | wc -l | tr -d ' ')
    SWEEP=$(pgrep -f "iss159_step5_sweep_v2.sh" | wc -l | tr -d ' ')
    if [ "$BENCH" = "0" ] && [ "$SWEEP" = "0" ]; then
        break
    fi
    sleep 60
done

# Build result document.
{
    echo "# ISS-159 Step 5 v2 Autorun Result"
    echo ""
    echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "STAMP: $STAMP"
    echo ""
    echo "## Artifact check"
    echo ""
    ARMS_FOUND=()
    for arm in A B C; do
        f=$RUNS/ISS159v2-${arm}-conv26-${STAMP}/locomo_summary.json
        if [ -f "$f" ]; then
            echo "- Arm $arm OK: $f"
            ARMS_FOUND+=("$arm")
        else
            echo "- Arm $arm MISSING: $f"
        fi
    done
    echo ""
    echo "## Per-arm summaries"
    echo ""
    for arm in A B C; do
        f=$RUNS/ISS159v2-${arm}-conv26-${STAMP}/locomo_summary.json
        if [ -f "$f" ]; then
            echo "### Arm $arm"
            echo '```json'
            cat "$f"
            echo '```'
            echo ""
        fi
    done

    echo ""
    echo "## Comparative table"
    echo ""
    echo '```'
    python3 - "$STAMP" "$RUNS" <<'PYEOF'
import json, sys, os
stamp, runs_root = sys.argv[1], sys.argv[2]
rows = []
for arm in ["A", "B", "C"]:
    f = os.path.join(runs_root, f"ISS159v2-{arm}-conv26-{stamp}", "locomo_summary.json")
    if not os.path.exists(f):
        rows.append((arm, None))
        continue
    d = json.load(open(f))
    rows.append((arm, d))
# Print header
print(f"{'Arm':<6}{'overall':<12}{'single-hop':<14}{'multi-hop':<13}{'open':<10}{'temporal':<10}")
print("-" * 65)
ref = {"label":"ISS-157-A", "overall":0.4605, "single-hop":0.2188, "multi-hop":0.5405, "open-domain":0.5385, "temporal":0.5143}
print(f"{'ref':<6}{ref['overall']:<12.4f}{ref['single-hop']:<14.4f}{ref['multi-hop']:<13.4f}{ref['open-domain']:<10.4f}{ref['temporal']:<10.4f}  (ISS-157-A baseline)")
for arm, d in rows:
    if d is None:
        print(f"{arm:<6}MISSING")
        continue
    bc = d.get("by_category", {})
    print(f"{arm:<6}{d['overall']:<12.4f}{bc.get('single-hop',0):<14.4f}{bc.get('multi-hop',0):<13.4f}{bc.get('open-domain',0):<10.4f}{bc.get('temporal',0):<10.4f}")
print()
# AC-5a verdict
best_sh = max((d['by_category']['single-hop'] for _, d in rows if d), default=0.0)
best_arm = max(((a, d['by_category']['single-hop']) for a, d in rows if d), key=lambda x: x[1], default=("none", 0.0))
print(f"Best single-hop: Arm {best_arm[0]} @ {best_sh:.4f}")
target = 0.60
gap = target - best_sh
if best_sh >= target:
    print(f"✅ AC-5a PASS ({best_sh:.4f} ≥ {target})")
else:
    print(f"❌ AC-5a FAIL (gap = {gap:.4f}, best = {best_sh:.4f})")
PYEOF
    echo '```'

    echo ""
    echo "## Caveat: stochastic baseline drift"
    echo ""
    echo "Arm A is **not** expected to bit-reproduce ISS-157-A despite identical"
    echo "retrieval config — Anthropic Haiku extractor is non-deterministic even"
    echo "at temp=0 (known ISS-155-class issue). Internal A/B/C comparison within"
    echo "this sweep is the valid signal; cross-sweep baseline comparison is noisy."
    echo "Empirical drift observed in v1 run (2026-05-26T03:56:56Z): overall"
    echo "0.4605 → 0.3618 (-9.9pp) under nominally identical config."
    echo ""
    echo "Decision rule: judge CE by **Arm B − Arm A** delta on single-hop within"
    echo "this sweep, not by absolute Arm B vs historical 0.2188."

} > "$RESULT.tmp"
mv "$RESULT.tmp" "$RESULT"

echo "[autorun-v2] DONE — see $RESULT" >> "$LOG_DIR/autorun.progress"
