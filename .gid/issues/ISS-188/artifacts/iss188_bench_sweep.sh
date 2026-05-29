#!/bin/bash
# ISS-188 AC-3 λ-sweep — populate factual/episodic-plan candidate
# embeddings before the C.5 MMR hook so diversity reranking can fire
# on list-questions (root fix from ISS-187 drop_CD verdict).
#
# Three arms on conv-26, ISS-161 Arm A envelope (locked v0.3 baseline):
#   A: POPULATE_EMBEDDINGS=off                 (baseline — byte-identical
#                                               to locked v0.3, MMR no-op
#                                               on factual plan as today)
#   B: POPULATE_EMBEDDINGS=on  MMR_LAMBDA=0.7  (Carbonell-Goldstein default)
#   C: POPULATE_EMBEDDINGS=on  MMR_LAMBDA=0.5  (stronger diversity push)
#
# Envelope (ISS-161 Arm A canonical):
#   K=10, temp=0, HyDE=off, MMR off (λ via env per arm), cross-encoder off,
#   entity_channel off, FACTUAL_REWEIGHT=off (locked v0.3), pipeline_pool=1.
#   Single-axis sweep isolating embedding-population + λ.
#
# Analysis target (AC-3): the 10 LIST-type SF queries
#   q13 q15 q18 q19 q24 q32 q34 q38 q39 q47
# Regression guard (AC-4): single-value SF q4 q7 q43 + conv-26 overall.
#
# Decision rule (ISS-188 issue body):
#   list-SF coverage lift >= +3/10 AND no single-value regression
#       → ship populate + winning λ as default (pending conv-44 AC-5).
#   lift +1..+2/10 → opt-in only, keep default off.
#   lift <= 0      → falsified; problem is in JUDGE or GENERATION, not
#                    retrieval. Pivot to ISS-179.

set -uo pipefail

cd /Users/potato/clawd/projects/engram-bench

# Anthropic OAuth from Claude Code keychain (fallback to env).
if [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    TOK_JSON=$(security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null || true)
    if [ -n "$TOK_JSON" ]; then
        TOK=$(python3 -c "import json,sys; d=json.loads(sys.argv[1]); print(d['claudeAiOauth']['accessToken'])" "$TOK_JSON" 2>/dev/null || true)
        [ -n "$TOK" ] && export ANTHROPIC_AUTH_TOKEN="$TOK"
    fi
fi
if [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    echo "ERROR: no anthropic auth token in env or keychain" >&2
    exit 1
fi

BIN=/Users/potato/clawd/projects/engram-bench/target/release/engram-bench
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
LOG_DIR=/tmp/iss188-bench
mkdir -p "$LOG_DIR"
echo "$STAMP" > "$LOG_DIR/STAMP"

# Common env (ISS-161 Arm A canonical envelope).
unset ENGRAM_BENCH_HYDE
unset ENGRAM_BENCH_K_SEED
unset ENGRAM_BENCH_BM25_POOL
unset ENGRAM_BENCH_FORCE_INTENT
unset ENGRAM_BENCH_CROSS_ENCODER
unset ENGRAM_BENCH_GEN_PROMPT
unset ENGRAM_EXTRACTOR_PROMPT
unset ENGRAM_BENCH_ENTITY_CHANNEL        # entity channel OFF for all arms
unset ENGRAM_BENCH_PREV_TURN_CONTEXT
export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=10
export ENGRAM_BENCH_PIPELINE_POOL=1      # ISS-166: install v0.3 pipeline
export ENGRAM_BENCH_FACTUAL_REWEIGHT=off # ISS-161 Arm A locked v0.3 baseline

run_arm () {
    local arm="$1"; shift
    local populate="$1"; shift   # on | off
    local lambda="$1"; shift     # numeric | unset
    local out="benchmarks/runs/ISS188-${arm}-conv26-${STAMP}"
    mkdir -p "$out"

    local log="$LOG_DIR/iss188-${arm}.log"

    export ENGRAM_BENCH_POPULATE_EMBEDDINGS="$populate"
    if [ "$lambda" = "unset" ]; then
        unset ENGRAM_BENCH_MMR_LAMBDA
    else
        export ENGRAM_BENCH_MMR_LAMBDA="$lambda"
    fi

    echo "=== ARM ${arm}  POPULATE=${populate}  MMR_LAMBDA=${lambda}  K=10  HYDE=off  → $out ===" | tee -a "$LOG_DIR/master.log"
    echo "    log: $log" | tee -a "$LOG_DIR/master.log"
    echo "    start: $(date -u +%Y-%m-%dT%H:%M:%SZ)" | tee -a "$LOG_DIR/master.log"

    "$BIN" locomo --output-dir "$PWD/benchmarks/runs" >"$log" 2>&1
    local rc=$?
    echo "    bench exit code: $rc" | tee -a "$LOG_DIR/master.log"

    local newest
    newest=$(find benchmarks/runs -maxdepth 1 -type d -name "2026-*Z_locomo" -newer "$LOG_DIR/STAMP" 2>/dev/null | head -1)
    if [ -n "$newest" ] && [ -d "$newest" ]; then
        mv "$newest"/* "$out"/ 2>/dev/null || true
        rmdir "$newest" 2>/dev/null || true
        echo "    moved $newest → $out" | tee -a "$LOG_DIR/master.log"
    else
        echo "    WARN: no new run dir found newer than STAMP; arm likely aborted" | tee -a "$LOG_DIR/master.log"
    fi

    if [ -f "$out/locomo_summary.json" ]; then
        python3 -c "
import json
with open('$out/locomo_summary.json') as f:
    s = json.load(f)
print(f'    overall: {s[\"overall\"]}')
for k, v in s.get('by_category', {}).items():
    print(f'    {k}: {v}')
" | tee -a "$LOG_DIR/master.log"
    fi
    echo "    end: $(date -u +%Y-%m-%dT%H:%M:%SZ)" | tee -a "$LOG_DIR/master.log"
}

run_arm A off unset
run_arm B on  0.7
run_arm C on  0.5

echo "=== ALL DONE ===" | tee -a "$LOG_DIR/master.log"
echo "" | tee -a "$LOG_DIR/master.log"
echo "Next: python3 /tmp/iss188_analyse.py $STAMP" | tee -a "$LOG_DIR/master.log"
echo "Compare list-SF coverage (q13/15/18/19/24/32/34/38/39/47) per arm." | tee -a "$LOG_DIR/master.log"
