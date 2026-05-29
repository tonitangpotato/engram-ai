#!/bin/bash
# ISS-188 AC-3 Arm C re-run — original sweep died on OAuth 401 mid-run
# (token expired after A+B ~55min). Re-run Arm C alone with a fresh
# token, SAME STAMP so /tmp/iss188_analyse.py pairs all three arms.
#
# Arm C: POPULATE_EMBEDDINGS=on  MMR_LAMBDA=0.5  (stronger diversity push)
# Envelope: ISS-161 Arm A — conv-26 K=10 temp=0 HyDE=off entity_channel=off
#           FACTUAL_REWEIGHT=off pipeline_pool=1.

set -uo pipefail
cd /Users/potato/clawd/projects/engram-bench

# Fresh OAuth from Claude Code keychain.
TOK_JSON=$(security find-generic-password -s "Claude Code-credentials" -w 2>/dev/null || true)
TOK=$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['claudeAiOauth']['accessToken'])" "$TOK_JSON" 2>/dev/null || true)
if [ -z "$TOK" ]; then echo "ERROR: no token" >&2; exit 1; fi
export ANTHROPIC_AUTH_TOKEN="$TOK"

BIN=/Users/potato/clawd/projects/engram-bench/target/release/engram-bench
STAMP=20260529T041125Z          # MATCH original sweep so analysis pairs arms
LOG_DIR=/tmp/iss188-bench
echo "$STAMP" > "$LOG_DIR/STAMP"  # restore STAMP for find -newer

# ISS-161 Arm A envelope.
unset ENGRAM_BENCH_HYDE ENGRAM_BENCH_K_SEED ENGRAM_BENCH_BM25_POOL
unset ENGRAM_BENCH_FORCE_INTENT ENGRAM_BENCH_CROSS_ENCODER
unset ENGRAM_BENCH_GEN_PROMPT ENGRAM_EXTRACTOR_PROMPT
unset ENGRAM_BENCH_ENTITY_CHANNEL ENGRAM_BENCH_PREV_TURN_CONTEXT
export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=10
export ENGRAM_BENCH_PIPELINE_POOL=1
export ENGRAM_BENCH_FACTUAL_REWEIGHT=off
export ENGRAM_BENCH_POPULATE_EMBEDDINGS=on
export ENGRAM_BENCH_MMR_LAMBDA=0.5

out="benchmarks/runs/ISS188-C-conv26-${STAMP}"
mkdir -p "$out"
log="$LOG_DIR/iss188-C.log"

echo "=== ARM C RERUN  POPULATE=on  MMR_LAMBDA=0.5  → $out ===" | tee -a "$LOG_DIR/master.log"
echo "    start: $(date -u +%Y-%m-%dT%H:%M:%SZ)" | tee -a "$LOG_DIR/master.log"

"$BIN" locomo --output-dir "$PWD/benchmarks/runs" >"$log" 2>&1
echo "    bench exit code: $? (2 = release-gate BLOCK, non-fatal)" | tee -a "$LOG_DIR/master.log"

newest=$(find benchmarks/runs -maxdepth 1 -type d -name "2026-*Z_locomo" -newer "$LOG_DIR/STAMP" 2>/dev/null | head -1)
if [ -n "$newest" ] && [ -d "$newest" ]; then
    mv "$newest"/* "$out"/ 2>/dev/null || true
    rmdir "$newest" 2>/dev/null || true
    echo "    moved $newest → $out" | tee -a "$LOG_DIR/master.log"
else
    echo "    WARN: no new run dir — arm aborted" | tee -a "$LOG_DIR/master.log"
fi

if [ -f "$out/locomo_summary.json" ]; then
    python3 -c "import json;s=json.load(open('$out/locomo_summary.json'));print('    overall:',s['overall']);[print(f'    {k}: {v}') for k,v in s.get('by_category',{}).items()]" | tee -a "$LOG_DIR/master.log"
fi
echo "    end: $(date -u +%Y-%m-%dT%H:%M:%SZ)" | tee -a "$LOG_DIR/master.log"
echo "=== ARM C RERUN DONE — run: python3 /tmp/iss188_analyse.py $STAMP ===" | tee -a "$LOG_DIR/master.log"
