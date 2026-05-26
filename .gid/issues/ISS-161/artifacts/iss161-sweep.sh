#!/bin/bash
# ISS-161 HyDE PerCategoryV2 sweep.
# Three arms on conv-26, K=10 MMR=0.7 temp=0, candidate dump on.
#
#   A control — HYDE=per_category (current production)
#   B v2      — HYDE=per_category_v2 (open + single-hop)
#   C v2+K30  — HYDE=per_category_v2 + TOP_K=30 (stacking)
#
# Decision rule:
#   - B single-fact ≥ 5/12 AND aggregate ≥ A: ship policy change.
#   - C single-fact ≥ 8/12 (= AC-5a ≥0.60 met): AC-5a reachable via stack.
#   - Both B,C single-fact < 5/12: probe inconclusive → escalate.
#
# Single-fact = single-hop with non-list gold (12 of 32 on conv-26).
# Gate is the single-fact sub-bucket B−A delta, NOT aggregate.

set -uo pipefail

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
LOG_DIR=/tmp/iss161-sweep
mkdir -p "$LOG_DIR"
echo "$STAMP" > "$LOG_DIR/STAMP"

export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_DUMP_CANDIDATES=1
export RUST_LOG=${RUST_LOG:-warn}

RUNS_ROOT=/Users/potato/clawd/projects/engram-bench/benchmarks/runs

run_arm() {
    local name=$1
    local hyde=$2
    local top_k=$3
    local target_dir="$RUNS_ROOT/ISS161-${name}-conv26-${STAMP}"
    local log="$LOG_DIR/iss161-${name}.log"
    export ENGRAM_BENCH_HYDE="$hyde"
    export ENGRAM_BENCH_TOP_K="$top_k"
    echo "=== ARM $name  HYDE=$hyde  K=$top_k  → $target_dir ===" | tee -a "$LOG_DIR/master.log"
    echo "    log: $log" | tee -a "$LOG_DIR/master.log"

    local before_list
    before_list=$(mktemp)
    ls -1 "$RUNS_ROOT" > "$before_list" 2>/dev/null || true

    set +e
    "$BIN" --output-dir "$RUNS_ROOT" locomo > "$log" 2>&1
    local rc=$?
    set -e

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
    echo "    bench exit code: $rc" | tee -a "$LOG_DIR/master.log"
}

run_arm A per_category    10
run_arm B per_category_v2 10
run_arm C per_category_v2 30

echo "" | tee -a "$LOG_DIR/master.log"
echo "===========================================" | tee -a "$LOG_DIR/master.log"
echo "ISS-161 sweep complete — STAMP=$STAMP" | tee -a "$LOG_DIR/master.log"
echo "===========================================" | tee -a "$LOG_DIR/master.log"
