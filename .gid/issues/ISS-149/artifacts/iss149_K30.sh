#!/usr/bin/env bash
# ISS-149 deep-read follow-up: K-expansion probe on conv-26 single-hop.
#
# Hypothesis (from per-query analysis of ISS-149 probe):
#   - 16/25 single-hop failures are "list questions" where gold is
#     a multi-item list (e.g. "pottery, camping, painting, swimming")
#     and K=10 doesn't surface all relevant episodes.
#   - 9/25 are true single-fact misses where the answer lives in
#     1-2 of 419 episodes and gets crowded out.
#
# This probe asks: does K=30 lift single-hop materially?
#   - PASS: single-hop ≥ 0.35 → K-expansion is a cheap, real lever.
#           Combine with weapon A (cross-encoder) to chase AC-5.
#   - FAIL: single-hop ≤ 0.25 → list questions need query rewrite,
#           not just more candidates. Weapon A alone is the path.
#
# Config matches ISS-149 probe except TOP_K=30 instead of 10.
# Force-intent stays OFF (we want natural classifier, just bigger pool).

set -u
ROOT=/Users/potato/clawd/projects/engram-bench
BIN="$ROOT/target/release/engram-bench"

cd "$ROOT"

export ENGRAM_BENCH_LOCOMO_CONVS=conv-26
export ENGRAM_BENCH_TOP_K=30
export ENGRAM_BENCH_MMR_LAMBDA=0.7
export ENGRAM_BENCH_HYDE=per_category
unset ENGRAM_BENCH_FORCE_INTENT

if [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  TOK=$(security find-generic-password -s anthropic_oauth -w 2>/dev/null || true)
  if [ -n "$TOK" ]; then
    export ANTHROPIC_AUTH_TOKEN="$TOK"
  fi
fi

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
LABEL="K30"

echo "=== K-expansion probe: K=30 on conv-26 ==="
echo "stamp=$STAMP"
echo

LOG=/tmp/iss149-K30-${STAMP}.log
"$BIN" locomo > "$LOG" 2>&1
RC=$?
echo "exit=$RC  log=$LOG"

NEWEST=$(ls -dt "$ROOT/benchmarks/runs/"2026-*_locomo 2>/dev/null | head -1)
TARGET=""
if [ -n "$NEWEST" ] && [ -d "$NEWEST" ]; then
  TARGET="$ROOT/benchmarks/runs/ISS149-${LABEL}-conv26-${STAMP}"
  mv "$NEWEST" "$TARGET"
  echo "dir=$TARGET"
fi

if [ -n "$TARGET" ] && [ -f "$TARGET/locomo_summary.json" ]; then
  python3 << PY
import json
d = json.load(open("$TARGET/locomo_summary.json"))
by = d.get("by_category", {})
print()
print("=" * 60)
print("K=30 probe result")
print("=" * 60)
print(f"overall:    {d.get('overall',0):.4f}")
print(f"single-hop: {by.get('single-hop',0):.4f}  (K=10 baseline=0.2188)")
print(f"multi-hop:  {by.get('multi-hop',0):.4f}  (K=10 baseline=0.5405)")
print(f"open:       {by.get('open-domain',0):.4f}  (K=10 baseline=0.5385)")
print(f"temporal:   {by.get('temporal',0):.4f}  (K=10 baseline=0.5286)")

s = by.get("single-hop", 0.0)
print()
print("=== Decision ===")
if s >= 0.40:
    print(f"  AC-5 HIT at K=30 alone (single={s:.4f}). Ship K change.")
elif s >= 0.35:
    print(f"  STRONG ({s:.4f}). K-expansion is a real lever.")
    print(f"  Combine with weapon A (cross-encoder) to chase AC-5.")
elif s >= 0.28:
    print(f"  MIXED ({s:.4f}, +{(s-0.2188)*100:.1f}pp). Partial lift.")
elif s >= 0.22:
    print(f"  NEGLIGIBLE ({s:.4f}). K alone doesn't fix list questions.")
    print(f"  → Need query rewrite OR cross-encoder weapon A.")
else:
    print(f"  REGRESSION ({s:.4f}). K=30 hurt — noise crowding signal.")

# Also load per-query and bucket list vs non-list
per_q = []
import json as _j
for line in open("$TARGET/locomo_per_query.jsonl"):
    per_q.append(_j.loads(line))

single = [r for r in per_q if r["category"] == "single-hop"]
def is_list(g):
    import re
    return ("," in g or " and " in g.lower()
            or len(re.findall(r'"[^"]+"', g)) >= 2)
list_qs = [r for r in single if is_list(r["gold"])]
non_list = [r for r in single if not is_list(r["gold"])]
list_pass = sum(1 for r in list_qs if r["score"] == 1.0)
non_pass = sum(1 for r in non_list if r["score"] == 1.0)
print()
print("=== Bucketed (this run) ===")
print(f"list questions:        {list_pass}/{len(list_qs)} = {list_pass/max(len(list_qs),1):.3f}")
print(f"single-fact questions: {non_pass}/{len(non_list)} = {non_pass/max(len(non_list),1):.3f}")
print()
print("K=10 baseline (from ISS-149 probe):")
print("  list questions:        4/20 = 0.200")
print("  single-fact questions: 3/12 = 0.250")
PY
else
  echo "WARN: no locomo_summary.json"
fi
