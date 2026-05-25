#!/usr/bin/env bash
# ISS-149 probe / ISS-148 AC-5 weapon selection path 3:
# Does forcing Intent::Factual lift conv-26 single-hop?
#
# Background:
#   - ISS-157 weapon B (embedder swap) failed: 3 embedders, Factual plan
#     selected 0/152 across all arms. Bottleneck confirmed to be
#     ISS-149 (NullEntityLookup → entity-anchor=0 → Factual never wins).
#   - Before investing 2-4 days fixing ISS-149 OR 3-5 days building
#     weapon A (cross-encoder), this probe answers: IF the classifier
#     were fixed, would single-hop actually reach AC-5 (≥0.40)?
#
# Method:
#   ENGRAM_BENCH_FORCE_INTENT=factual short-circuits Stage-1 classify
#   via GraphQuery::with_intent(Intent::Factual) — the same caller-
#   override path that exists for legitimate API callers (dispatch.rs:213).
#   Two arms, identical config, only the intent differs.
#
# Expected outcomes & decisions:
#   - Forced single-hop ≥ 0.35  →  ISS-149 is the lever. Fix it (2-4 days).
#   - Forced single-hop 0.25-0.35 → mixed signal, weapon A still better.
#   - Forced single-hop ≤ 0.25  →  BM25 path is the ceiling, even with
#                                   Factual selected. Skip ISS-149, go
#                                   straight to weapon A.
#
# Caveat: forcing Factual will hurt other categories (multi-hop / temporal
# / abstract) since those questions don't want a Factual plan. We only
# read the single-hop bucket from arm B; other categories are noise here.

set -u
ROOT=/Users/potato/clawd/projects/engram-bench
BIN="$ROOT/target/release/engram-bench"

cd "$ROOT"

export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=10
export ENGRAM_BENCH_MMR_LAMBDA=0.7
export ENGRAM_BENCH_HYDE=per_category

# Anthropic auth — required for HyDE Haiku + judge Sonnet.
if [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  TOK=$(security find-generic-password -s anthropic_oauth -w 2>/dev/null || true)
  if [ -n "$TOK" ]; then
    export ANTHROPIC_AUTH_TOKEN="$TOK"
  fi
fi

STAMP=$(date -u +%Y%m%dT%H%M%SZ)

run_arm() {
  local label="$1"
  local intent="$2"

  echo "=== ARM ${label}: force_intent=${intent:-<natural>} ==="
  local log=/tmp/iss149-probe-${label}-${STAMP}.log

  if [ -z "$intent" ]; then
    unset ENGRAM_BENCH_FORCE_INTENT
  else
    export ENGRAM_BENCH_FORCE_INTENT="$intent"
  fi

  "$BIN" locomo > "$log" 2>&1
  local rc=$?
  echo "  exit=$rc  log=$log"

  local newest
  newest=$(ls -dt "$ROOT/benchmarks/runs/"2026-*_locomo 2>/dev/null | head -1)
  local target=""
  if [ -n "$newest" ] && [ -d "$newest" ]; then
    target="$ROOT/benchmarks/runs/ISS149-probe-${label}-conv26-${STAMP}"
    mv "$newest" "$target"
    echo "  dir=${target}"
  fi
  if [ -n "$target" ] && [ -f "$target/locomo_summary.json" ]; then
    python3 -c "
import json
d = json.load(open('$target/locomo_summary.json'))
by = d.get('by_category', {})
print(f\"  overall={d.get('overall',0):.4f} single={by.get('single-hop',0):.4f} multi={by.get('multi-hop',0):.4f} open={by.get('open-domain',0):.4f} temporal={by.get('temporal',0):.4f}\")
"
  else
    echo "  WARN: no locomo_summary.json"
  fi
  echo
}

echo "ISS-149 probe starting at $STAMP"
echo "config: conv-26, K=10, MMR=0.7, HyDE=per_category"
echo

run_arm "A-natural"  ""
run_arm "B-factual"  "factual"

echo "============================================================="
echo "ISS-149 probe summary"
echo "============================================================="
python3 - <<PY
import json, os
root = "$ROOT/benchmarks/runs"
stamp = "$STAMP"
arms = [("A-natural", "Stage-1 classifier (control)"),
        ("B-factual", "force Intent::Factual")]
results = {}
for label, _ in arms:
    p = f"{root}/ISS149-probe-{label}-conv26-{stamp}/locomo_summary.json"
    results[label] = json.load(open(p)) if os.path.exists(p) else None

print()
print(f"{'arm':14s}  {'overall':>8s}  {'multi-hop':>10s}  {'single-hop':>11s}  {'open':>7s}  {'temporal':>9s}")
for label, desc in arms:
    d = results.get(label)
    if d is None:
        print(f"{label:14s}  (missing summary)")
        continue
    by = d.get("by_category", {})
    print(f"{label:14s}  "
          f"{d.get('overall', 0):>8.4f}  "
          f"{by.get('multi-hop', 0):>10.4f}  "
          f"{by.get('single-hop', 0):>11.4f}  "
          f"{by.get('open-domain', 0):>7.4f}  "
          f"{by.get('temporal', 0):>9.4f}")

print()
a = results.get("A-natural") or {}
b = results.get("B-factual") or {}
a_single = a.get("by_category", {}).get("single-hop", 0.0)
b_single = b.get("by_category", {}).get("single-hop", 0.0)
delta = (b_single - a_single) * 100
print(f"single-hop delta (forced − natural): {delta:+.2f}pp")
print(f"absolute forced single-hop:           {b_single:.4f}")
print()
print("=== Decision ===")
if b_single >= 0.40:
    print(f"  PASS AC-5 already (Factual {b_single:.4f} ≥ 0.40). Fix ISS-149.")
elif b_single >= 0.35:
    print(f"  STRONG signal ({b_single:.4f} ≥ 0.35). ISS-149 is the lever.")
elif b_single >= 0.25:
    print(f"  MIXED ({b_single:.4f}). Weapon A likely still needed.")
else:
    print(f"  WEAK ({b_single:.4f} < 0.25). BM25 path is ceiling. Skip to weapon A.")
PY
