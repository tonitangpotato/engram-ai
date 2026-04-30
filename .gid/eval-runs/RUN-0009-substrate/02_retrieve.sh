#!/usr/bin/env bash
# RUN-0009 retrieve: full conv-26 e2e — all 19 sessions, all 199 QAs.
# Uses post-fix binary (ISS-075 + ISS-076) and the fresh-ingested substrate
# from 01_ingest.py (namespace `locomo-conv26-full`).
#
# Compare to:
#   - RUN-0007 baseline (sessions 1-3, 25 QAs, pre-fix): hit@5 = 10/20 = 50%
#   - RUN-0008 post-fix (sessions 1-3, 25 QAs, post-fix): hit@5 = 10/20 = 50% (FLAT)
#   - RUN-0009 (this) is the answer to "do the fixes move hit@k on the FULL
#     conv-26, where the larger graph + more entity overlap is where dedup
#     would actually matter?"
set -euo pipefail

ROOT="/Users/potato/clawd/projects/engram"
SUB="$ROOT/.gid/eval-runs/RUN-0009-substrate"
DB="$SUB/locomo-conv26-full.db"
GDB="$SUB/locomo-conv26-full.graph.db"
DATASET="/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json"

if [ ! -f "$DB" ]; then
  echo "[!] DB not found: $DB" >&2
  echo "    Run 01_ingest.py first." >&2
  exit 1
fi

cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
cargo run --quiet --release \
  --example locomo_conv26_retrieval \
  -- \
  --db "$DB" \
  --graph-db "$GDB" \
  --dataset "$DATASET" \
  --max-session 19 \
  --limit 5 \
  --ns locomo-conv26-full \
  2>&1 | tee "$SUB/RUN-0009-full-conv26.log"
