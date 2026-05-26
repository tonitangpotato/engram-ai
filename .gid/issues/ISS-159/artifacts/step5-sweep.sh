#!/bin/bash
# ISS-159 Step 5: 3-arm bench on conv-26 for weapon A (cross-encoder)
#
# Arms:
#   A control     — no CE, MMR=0.7, HyDE=per_category   (matches ISS-157-A baseline)
#   B CE-only     — CE on, MMR=1.0 (off), HyDE=per_category
#   C CE+MMR      — CE on, MMR=0.7, HyDE=per_category   (composition per D5)
#
# All arms: K=10 (final top-K, ISS-138 default), temp=0 (ISS-137),
#           ENGRAM_BENCH_LOCOMO_CONVS=conv-26.
#
# AC: AC-5a single-fact ≥ 0.60 on conv-26.
# Baseline (ISS-157-A nomic, this env): overall=?, single-hop=0.2188.
#
# Sequential — single Mac mini, one bench at a time to keep CPU isolated.

set -euo pipefail

cd /Users/potato/clawd/projects/engram-bench

# Honor existing env or fall back to keychain (Claude Code-credentials JSON blob).
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
LOG_DIR=/tmp/iss159-step5
mkdir -p "$LOG_DIR"

export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=10
export ENGRAM_BENCH_HYDE=per_category
export RUST_LOG=${RUST_LOG:-warn}

run_arm() {
    local name=$1
    local out_dir=benchmarks/runs/ISS159-${name}-conv26-${STAMP}
    local log=$LOG_DIR/iss159-${name}.log
    echo "=== ARM $name → $out_dir ==="
    echo "    log: $log"
    if "$BIN" --output-dir benchmarks/runs locomo > "$log" 2>&1; then
        # The bench writes to a timestamped dir under benchmarks/runs/.
        # Find the latest dir and rename for clarity.
        latest=$(ls -td benchmarks/runs/*locomo* 2>/dev/null | head -1)
        if [ -n "$latest" ]; then
            mv "$latest" "$out_dir"
            echo "    -> $out_dir"
        fi
        if [ -f "$out_dir/locomo_summary.json" ]; then
            python3 -c "
import json
d = json.load(open('$out_dir/locomo_summary.json'))
print(f\"    overall: {d.get('overall')}\")
for k, v in sorted(d.get('by_category', {}).items()):
    print(f\"    {k}: {v}\")
"
        fi
    else
        echo "    FAILED — see $log"
        tail -10 "$log"
        return 1
    fi
}

# ---- Arm A: control (no CE, MMR=0.7) ----
unset ENGRAM_BENCH_CROSS_ENCODER
# MMR override unset → falls through to FusionConfig::locked().mmr_lambda = 0.7
unset ENGRAM_BENCH_MMR_LAMBDA
run_arm A

# ---- Arm B: CE-only (CE on, MMR off) ----
export ENGRAM_BENCH_CROSS_ENCODER=1
export ENGRAM_BENCH_CROSS_ENCODER_K_IN=50
# Force MMR off via λ=1.0 (NullReranker equivalent)
export ENGRAM_BENCH_MMR_LAMBDA=1.0
run_arm B

# ---- Arm C: CE+MMR (composition test) ----
export ENGRAM_BENCH_CROSS_ENCODER=1
export ENGRAM_BENCH_CROSS_ENCODER_K_IN=50
unset ENGRAM_BENCH_MMR_LAMBDA  # default 0.7
run_arm C

echo ""
echo "=========================================="
echo "ISS-159 Step 5 sweep complete — STAMP=$STAMP"
echo "=========================================="
echo "Compare summary.json:"
echo "  jq '.overall_accuracy, .per_category' benchmarks/runs/ISS159-*-conv26-${STAMP}/locomo_summary.json"
echo ""
echo "AC-5a target: single-hop ≥ 0.60 on conv-26."
