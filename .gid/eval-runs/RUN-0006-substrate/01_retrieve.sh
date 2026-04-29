#!/usr/bin/env bash
# RUN-0006: re-run retrieval over RUN-0005 substrate (50/50 admitted, post ISS-068)
# but with the new per-LoCoMo-category breakdown instrumentation in
# crates/engramai/examples/locomo_conv26_retrieval.rs (uncommitted at run time).
#
# DB is copied fresh from RUN-0005-substrate so RUN-0005 evidence is preserved.
# Graph DB is created fresh (retrieval-side scaffolding only — no re-ingest).
set -euo pipefail

ROOT="/Users/potato/clawd/projects/engram"
SUB="$ROOT/.gid/eval-runs/RUN-0006-substrate"
DB="$SUB/locomo-conv26-iss068.db"
GDB="$SUB/locomo-conv26-iss068.graph.db"
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
  --ns locomo-conv26-iss068 \
  2>&1 | tee "$SUB/RUN-0006.log"
