#!/usr/bin/env bash
# ISS-160 AC-1: reproduce list/single-fact bucketing + K-invariance on a
# 2nd conversation. Target conv-44 because its single-hop ratio is
# INVERTED from conv-26 (13 list / 17 single-fact vs conv-26's 20/12) —
# strongest falsification test for "the bucketing pattern is universal."
#
# Hypothesis: list-bucket pass rate is roughly K-invariant on conv-44 too.
#             single-fact bucket lifts with K=10→K=30.
#
# Decision tree (on conv-44 K=30 vs K=10):
#   - list bucket delta ≤ +2pp  AND  single-fact bucket delta ≥ +8pp
#     → REPRODUCED. Lock ISS-160 design; proceed to ISS-159 weapon A.
#   - list bucket delta > +2pp
#     → NOT reproduced. List failures move with retrieval here.
#       conv-26 was an artefact. ISS-160 ACs need rework.
#   - single-fact delta < +5pp
#     → Weapon A's projected lift on conv-26 may be conv-specific.
#       Reconsider ISS-159 scope.

set -u
ROOT=/Users/potato/clawd/projects/engram-bench
BIN="$ROOT/target/release/engram-bench"

cd "$ROOT"

# Common config (matches ISS-149 probe envelope)
export ENGRAM_BENCH_LOCOMO_CONVS=conv-44
export ENGRAM_BENCH_MMR_LAMBDA=0.7
export ENGRAM_BENCH_HYDE=per_category
unset ENGRAM_BENCH_FORCE_INTENT

# OAuth token from keychain if not present in env
if [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  TOK=$(security find-generic-password -s anthropic_oauth -w 2>/dev/null || true)
  if [ -n "$TOK" ]; then
    export ANTHROPIC_AUTH_TOKEN="$TOK"
  fi
fi

STAMP=$(date -u +%Y%m%dT%H%M%SZ)

run_arm () {
  local K=$1
  local LABEL=$2
  echo
  echo "=== ARM: K=$K (label=$LABEL) on conv-44 ==="
  export ENGRAM_BENCH_TOP_K=$K
  LOG=/tmp/iss160-repro-${LABEL}-${STAMP}.log
  "$BIN" locomo > "$LOG" 2>&1
  local RC=$?
  echo "exit=$RC  log=$LOG"
  NEWEST=$(ls -dt "$ROOT/benchmarks/runs/"2026-*_locomo 2>/dev/null | head -1)
  if [ -n "$NEWEST" ] && [ -d "$NEWEST" ]; then
    TARGET="$ROOT/benchmarks/runs/ISS160-repro-${LABEL}-conv44-${STAMP}"
    mv "$NEWEST" "$TARGET"
    echo "dir=$TARGET"
    echo "$TARGET" > /tmp/iss160-repro-${LABEL}-${STAMP}.dir
  else
    echo "WARN: no run dir found after K=$K"
    return 1
  fi
}

run_arm 10 K10
run_arm 30 K30

# Comparative bucketing analysis
DIR_K10=$(cat /tmp/iss160-repro-K10-${STAMP}.dir 2>/dev/null || echo "")
DIR_K30=$(cat /tmp/iss160-repro-K30-${STAMP}.dir 2>/dev/null || echo "")

if [ -z "$DIR_K10" ] || [ -z "$DIR_K30" ]; then
  echo "ERROR: one or both arms missing run dirs. Bailing."
  exit 1
fi

python3 << PY
import json, re

def is_list(g):
    if not isinstance(g, str): return False
    if ',' in g: return True
    if ' and ' in g.lower(): return True
    if len(re.findall(r'"[^"]+"', g)) >= 2: return True
    return False

def load(d):
    summ = json.load(open(f"{d}/locomo_summary.json"))
    per_q = [json.loads(l) for l in open(f"{d}/locomo_per_query.jsonl")]
    single = [r for r in per_q if r["category"] == "single-hop"]
    list_qs = [r for r in single if is_list(r["gold"])]
    sf_qs = [r for r in single if not is_list(r["gold"])]
    list_pass = sum(1 for r in list_qs if r["score"] == 1.0)
    sf_pass = sum(1 for r in sf_qs if r["score"] == 1.0)
    return {
        "overall": summ.get("overall", 0.0),
        "by_cat": summ.get("by_category", {}),
        "n_single": len(single),
        "n_list": len(list_qs),
        "n_sf": len(sf_qs),
        "list_pass": list_pass,
        "sf_pass": sf_pass,
        "single_acc": (list_pass + sf_pass) / max(len(single), 1),
        "list_acc": list_pass / max(len(list_qs), 1),
        "sf_acc": sf_pass / max(len(sf_qs), 1),
    }

K10 = load("$DIR_K10")
K30 = load("$DIR_K30")

print()
print("=" * 70)
print("ISS-160 AC-1 reproduction probe — conv-44")
print("=" * 70)
print()
print(f"{'metric':<32} {'K=10':>10} {'K=30':>10} {'Δ pp':>10}")
print("-" * 64)
def row(label, a, b):
    d = (b - a) * 100
    print(f"{label:<32} {a:>10.4f} {b:>10.4f} {d:>+9.1f}")

row("overall",                   K10["overall"], K30["overall"])
row("by-cat: single-hop",        K10["by_cat"].get("single-hop", 0), K30["by_cat"].get("single-hop", 0))
row("by-cat: multi-hop",         K10["by_cat"].get("multi-hop", 0),  K30["by_cat"].get("multi-hop", 0))
row("by-cat: open-domain",       K10["by_cat"].get("open-domain", 0), K30["by_cat"].get("open-domain", 0))
row("by-cat: temporal",          K10["by_cat"].get("temporal", 0),   K30["by_cat"].get("temporal", 0))
print()
print(f"single-hop bucketed (n_list={K10['n_list']}, n_single-fact={K10['n_sf']}):")
row("  list questions",          K10["list_acc"], K30["list_acc"])
row("  single-fact questions",   K10["sf_acc"],   K30["sf_acc"])
print()
print("=" * 70)
print("Decision")
print("=" * 70)
list_delta_pp = (K30["list_acc"] - K10["list_acc"]) * 100
sf_delta_pp   = (K30["sf_acc"]   - K10["sf_acc"])   * 100
print(f"list bucket delta:       {list_delta_pp:+.1f}pp  (target: ≤ +2pp for K-invariance)")
print(f"single-fact bucket delta:{sf_delta_pp:+.1f}pp  (target: ≥ +8pp to match conv-26)")
print()
if list_delta_pp <= 2.0 and sf_delta_pp >= 8.0:
    print("✓ REPRODUCED. ISS-160 bucketing + K-invariance pattern holds.")
    print("  → Lock ISS-160 design. Proceed to ISS-159 weapon A.")
elif list_delta_pp > 2.0:
    print(f"✗ NOT REPRODUCED. List bucket moved {list_delta_pp:+.1f}pp with K.")
    print("  → conv-26 K-invariance may be a corpus artefact.")
    print("  → ISS-160 ACs need rework before weapon A.")
elif sf_delta_pp < 5.0:
    print(f"~ PARTIAL. List invariant held, but single-fact lifted only {sf_delta_pp:+.1f}pp.")
    print("  → Weapon A's projected lift may be conv-specific.")
    print("  → Re-scope ISS-159 before committing.")
else:
    print(f"~ INTERMEDIATE. list={list_delta_pp:+.1f}pp, sf={sf_delta_pp:+.1f}pp.")
    print("  → Inspect per-query JSONLs before deciding.")

print()
print("Artifacts:")
print(f"  K=10: $DIR_K10")
print(f"  K=30: $DIR_K30")
PY
