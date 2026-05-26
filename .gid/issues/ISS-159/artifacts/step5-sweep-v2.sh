#!/bin/bash
# ISS-159 Step 5 sweep v2.
# Fixes from v1:
#   1. bench --output-dir is a ROOT, not arm-specific dir. We pass a per-arm root
#      then rename the auto-stamped subdir to ISS159-{ARM}-conv26-${STAMP}.
#   2. ship-gate FAIL no longer kills the sweep (set +e around bench, log result).
#   3. exit-code of bench is recorded but does NOT stop next arm.
#
# Arms (unchanged):
#   A control — no CE, MMR=0.7
#   B CE-only — CE on K=50, MMR=1.0 (off)
#   C CE+MMR  — CE on K=50, MMR=0.7

set -uo pipefail   # no -e: ship-gate fail should not abort sweep

cd /Users/potato/clawd/projects/engram-bench

# Keychain fallback for Anthropic OAuth.
if [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    TOK_JSON=$(security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null || true)
    if [ -n "$TOK_JSON" ]; then
        TOK=$(python3 -c "import json,sys; d=json.loads(sys.argv[1]); print(d['claudeAiOauth']['accessToken'])" "$TOK_JSON" 2>/dev/null || true)
        if [ -n "$TOK" ]; then
            export ANTHROPIC_AUTH_TOKEN="$TOK"
        fi
    fi
fi
if [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    echo "ERROR: no anthropic auth token in env or keychain" >&2
    exit 1
fi

BIN=/Users/potato/clawd/projects/engram-bench/target/release/engram-bench
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
LOG_DIR=/tmp/iss159-step5-v2
mkdir -p "$LOG_DIR"

# Write STAMP to a known location so autorun can find it.
echo "$STAMP" > "$LOG_DIR/STAMP"

export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=10
export ENGRAM_BENCH_HYDE=per_category
export RUST_LOG=${RUST_LOG:-warn}

RUNS_ROOT=/Users/potato/clawd/projects/engram-bench/benchmarks/runs

run_arm() {
    local name=$1
    local target_dir="$RUNS_ROOT/ISS159v2-${name}-conv26-${STAMP}"
    local log="$LOG_DIR/iss159v2-${name}.log"
    echo "=== ARM $name → $target_dir ===" | tee -a "$LOG_DIR/master.log"
    echo "    log: $log" | tee -a "$LOG_DIR/master.log"

    # Capture set of existing runs BEFORE so we can identify the new one AFTER.
    local before_list
    before_list=$(mktemp)
    ls -1 "$RUNS_ROOT" > "$before_list" 2>/dev/null || true

    # set +e around bench: ship-gate fail must NOT kill the sweep
    set +e
    "$BIN" --output-dir "$RUNS_ROOT" locomo > "$log" 2>&1
    local rc=$?
    set -e

    # Find the new subdir bench just created (the only dir not in before_list).
    local new_dir
    new_dir=$(comm -13 <(sort "$before_list") <(ls -1 "$RUNS_ROOT" | sort) | grep "_locomo$" | head -1)
    rm -f "$before_list"

    if [ -n "$new_dir" ] && [ -d "$RUNS_ROOT/$new_dir" ]; then
        mv "$RUNS_ROOT/$new_dir" "$target_dir"
        echo "    moved $new_dir → $(basename "$target_dir")" | tee -a "$LOG_DIR/master.log"
        if [ -f "$target_dir/locomo_summary.json" ]; then
            python3 -c "
import json
d = json.load(open('$target_dir/locomo_summary.json'))
print(f\"    overall: {d.get('overall')}\")
for k, v in sorted(d.get('by_category', {}).items()):
    print(f\"    {k}: {v}\")
" | tee -a "$LOG_DIR/master.log"
        fi
    else
        echo "    NO OUTPUT DIR CREATED (rc=$rc) — see $log" | tee -a "$LOG_DIR/master.log"
        tail -5 "$log" | tee -a "$LOG_DIR/master.log"
    fi
    echo "    bench exit code: $rc (non-zero is OK if just a ship-gate fail)" | tee -a "$LOG_DIR/master.log"
}

# ---- Arm A: control (no CE, MMR=0.7) ----
unset ENGRAM_BENCH_CROSS_ENCODER
unset ENGRAM_BENCH_CROSS_ENCODER_K_IN
unset ENGRAM_BENCH_MMR_LAMBDA
run_arm A

# ---- Arm B: CE-only (CE on, MMR off) ----
export ENGRAM_BENCH_CROSS_ENCODER=1
export ENGRAM_BENCH_CROSS_ENCODER_K_IN=50
export ENGRAM_BENCH_MMR_LAMBDA=1.0
run_arm B

# ---- Arm C: CE+MMR (composition test) ----
export ENGRAM_BENCH_CROSS_ENCODER=1
export ENGRAM_BENCH_CROSS_ENCODER_K_IN=50
unset ENGRAM_BENCH_MMR_LAMBDA  # default 0.7 from FusionConfig::locked
run_arm C

echo "" | tee -a "$LOG_DIR/master.log"
echo "===========================================" | tee -a "$LOG_DIR/master.log"
echo "ISS-159 Step 5 v2 sweep complete — STAMP=$STAMP" | tee -a "$LOG_DIR/master.log"
echo "===========================================" | tee -a "$LOG_DIR/master.log"
