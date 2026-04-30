#!/usr/bin/env bash
# RUN-0007 baseline retrieve: measure hit@k on the BROKEN graph
# (before ISS-076 + ISS-075 fixes land). This quantifies "how broken"
# so post-fix runs can claim a measurable improvement.
#
# Reads the LIVE RUN-0007 .db files (which we've already backed up to
# RUN-0007-substrate-pre-fix/ for archival).
set -euo pipefail

ROOT="/Users/potato/clawd/projects/engram"
SUB="$ROOT/.gid/eval-runs/RUN-0007-substrate"
DB="$SUB/locomo-conv26-iss072.db"
GDB="$SUB/locomo-conv26-iss072.graph.db"
DATASET="/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json"

cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
cargo run --quiet --release \
  --example locomo_conv26_retrieval \
  -- \
  --db "$DB" \
  --graph-db "$GDB" \
  --dataset "$DATASET" \
  --max-session 3 \
  --limit 5 \
  --ns locomo-conv26-iss072 \
  2>&1 | tee "$SUB/RUN-0007-baseline-pre-fix.log"
