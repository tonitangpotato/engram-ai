#!/usr/bin/env bash
# RUN-0008 post-fix retrieve: measure hit@k with ISS-076 dangling-UUID
# fix applied (Phase A only — ISS-075/ISS-077 NOT yet applied).
#
# Compare to RUN-0007 baseline log:
#   .gid/eval-runs/RUN-0007-substrate/RUN-0007-baseline-pre-fix.log
set -euo pipefail

ROOT="/Users/potato/clawd/projects/engram"
SUB="$ROOT/.gid/eval-runs/RUN-0008-substrate"
DB="$SUB/locomo-conv26-iss076.db"
GDB="$SUB/locomo-conv26-iss076.graph.db"
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
  --ns locomo-conv26-iss076 \
  2>&1 | tee "$SUB/RUN-0008-post-fix.log"
