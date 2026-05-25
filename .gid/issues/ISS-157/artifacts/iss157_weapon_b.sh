#!/usr/bin/env bash
# ISS-157 weapon B: embedder swap on conv-26 K=10 MMR 0.7.
#
# Three arms:
#   A. nomic-embed-text  (768d)  — baseline, current default
#   B. bge-large          (1024d) — MTEB +1.8 vs nomic
#   C. mxbai-embed-large  (1024d) — MTEB +2.3 vs nomic, MTEB leader open weights
#
# Reference baselines (from ISS-156 PerCategory, identical retrieval config
# but nomic-embed-text):
#   overall=0.4737  multi=0.5946  open=0.4615  single=0.2188  temporal=0.5286
#
# Target for weapon B (per ISS-157 design):
#   single-hop ≥ 0.28 (+6pp over 0.2188) without regression on other
#   categories ≥ 2pp. If hit, proceed to weapon-B production wiring;
#   if miss, kill this weapon and move to weapon A (cross-encoder).
#
# HyDE policy held at per_category (the ISS-156 winner) to keep the
# experiment apples-to-apples — we're isolating the embedder variable.

set -u
ROOT=/Users/potato/clawd/projects/engram-bench
BIN="$ROOT/target/release/engram-bench"

cd "$ROOT"

export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=10
export ENGRAM_BENCH_MMR_LAMBDA=0.7
export ENGRAM_BENCH_HYDE=per_category
# Anthropic auth — required for HyDE Haiku call + judge Sonnet call.
# Honor existing env or fall back to keychain. (Same pattern as
# /tmp/iss156_empirical.sh.)
if [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  TOK=$(security find-generic-password -s anthropic_oauth -w 2>/dev/null || true)
  if [ -n "$TOK" ]; then
    export ANTHROPIC_AUTH_TOKEN="$TOK"
  fi
fi

run_arm() {
  local label="$1"
  local model="$2"
  local dim="$3"
  local stamp="$4"

  echo "=== ARM ${label}: model=${model} dim=${dim} ==="
  local log=/tmp/iss157-${label}-${stamp}.log

  if [ -z "$model" ]; then
    unset ENGRAM_BENCH_EMBED_MODEL ENGRAM_BENCH_EMBED_DIM
  else
    export ENGRAM_BENCH_EMBED_MODEL="$model"
    export ENGRAM_BENCH_EMBED_DIM="$dim"
  fi

  "$BIN" locomo \
    > "$log" 2>&1

  local rc=$?
  echo "  rc=${rc}, log=${log}"

  local newest
  newest=$(ls -dt "$ROOT/benchmarks/runs/"2026-*_locomo 2>/dev/null | head -1)
  if [ -n "$newest" ] && [ -d "$newest" ]; then
    local target="$ROOT/benchmarks/runs/ISS157-${label}-conv26-${stamp}"
    mv "$newest" "$target"
    echo "  dir=${target}"
    if [ -f "$target/locomo_summary.json" ]; then
      python3 -c "
import json
d = json.load(open('$target/locomo_summary.json'))
print('  overall =', round(d['overall'], 4))
for c in sorted(d.get('by_category', {})):
    print(f'  {c:12s} =', round(d['by_category'][c], 4))
"
    fi
  fi
  echo
}

STAMP=$(date -u +%Y%m%dT%H%M%SZ)

run_arm "A-nomic"      ""                "" "$STAMP"
run_arm "B-bge"        "bge-large"        1024 "$STAMP"
run_arm "C-mxbai"      "mxbai-embed-large" 1024 "$STAMP"

# Cross-arm comparison
python3 << PY
import json, os
root = "$ROOT/benchmarks/runs"
stamp = "$STAMP"
arms = [
    ("A-nomic",  "nomic-embed-text 768d (baseline)"),
    ("B-bge",    "bge-large 1024d"),
    ("C-mxbai",  "mxbai-embed-large 1024d"),
]
results = {}
for label, _desc in arms:
    p = f"{root}/ISS157-{label}-conv26-{stamp}/locomo_summary.json"
    if os.path.exists(p):
        results[label] = json.load(open(p))
    else:
        results[label] = None

print()
print("=" * 78)
print("ISS-157 weapon B — embedder swap, conv-26 K=10 MMR 0.7 HyDE=per_category")
print("=" * 78)
print()
print(f"{'arm':12s}  {'overall':>8s}  {'multi-hop':>10s}  {'single-hop':>11s}  {'open':>7s}  {'temporal':>9s}")
for label, desc in arms:
    d = results.get(label)
    if d is None:
        print(f"{label:12s}  (missing summary)")
        continue
    by = d.get("by_category", {})
    print(f"{label:12s}  "
          f"{d.get('overall', 0):>8.4f}  "
          f"{by.get('multi-hop', 0):>10.4f}  "
          f"{by.get('single-hop', 0):>11.4f}  "
          f"{by.get('open-domain', 0):>7.4f}  "
          f"{by.get('temporal', 0):>9.4f}")

print()
print("ISS-156 PerCategory baseline (nomic, separate run):")
print(f"{'(ref)':12s}    0.4737      0.5946       0.2188   0.4615    0.5286")

print()
print("=== ISS-157 weapon B gates ===")
ref_single = 0.2188
ref_overall = 0.4737
ref_multi = 0.5946

for label, _ in [("B-bge", None), ("C-mxbai", None)]:
    d = results.get(label)
    if d is None:
        continue
    s = d["by_category"].get("single-hop", 0.0)
    o = d.get("overall", 0.0)
    m = d["by_category"].get("multi-hop", 0.0)
    print()
    print(f"--- {label} ---")
    pass_single = s >= 0.28
    pass_overall = o >= ref_overall - 0.02
    pass_multi = m >= ref_multi - 0.02
    print(f"  single-hop ≥ 0.28:               {'PASS' if pass_single else 'FAIL'}  ({s:.4f}, Δ {(s-ref_single)*100:+.2f}pp vs A-baseline)")
    print(f"  overall ≥ {ref_overall:.4f}-2pp ({ref_overall-0.02:.4f}):  {'PASS' if pass_overall else 'FAIL'}  ({o:.4f})")
    print(f"  multi-hop ≥ {ref_multi:.4f}-2pp ({ref_multi-0.02:.4f}): {'PASS' if pass_multi else 'FAIL'}  ({m:.4f})")
PY
