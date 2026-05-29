#!/usr/bin/env python3
"""ISS-188 AC-3 λ-sweep analysis.

Compares list-SF coverage across arms A (populate off), B (on, λ=0.7),
C (on, λ=0.5). Slices the 10 LIST-type SF queries and 3 single-value SF
queries, applies the ISS-188 decision rule.

Usage: python3 iss188_analyse.py <STAMP>
"""
import json
import sys
from pathlib import Path

STAMP = sys.argv[1] if len(sys.argv) > 1 else open("/tmp/iss188-bench/STAMP").read().strip()
RUNS = Path("/Users/potato/clawd/projects/engram-bench/benchmarks/runs")

# ISS-188 AC-3 target: the 10 LIST-type single-fact queries.
LIST_SF = [f"conv-26-q{n}" for n in (13, 15, 18, 19, 24, 32, 34, 38, 39, 47)]
# ISS-188 AC-4 regression guard: single-value SF queries.
SINGLE_SF = [f"conv-26-q{n}" for n in (4, 7, 43)]

ARMS = {
    "A": f"ISS188-A-conv26-{STAMP}",
    "B": f"ISS188-B-conv26-{STAMP}",
    "C": f"ISS188-C-conv26-{STAMP}",
}


def load(arm_dir):
    p = RUNS / arm_dir / "locomo_per_query.jsonl"
    if not p.exists():
        return None
    rows = {}
    for line in p.read_text().splitlines():
        if not line.strip():
            continue
        r = json.loads(line)
        rows[r["id"]] = r
    return rows


def coverage(rows, qids):
    """Sum of scores over qids (partial credit) + count passing >= 0.5."""
    scores = [rows[q]["score"] for q in qids if q in rows]
    passing = sum(1 for s in scores if s >= 0.5)
    return sum(scores), passing, len(scores)


def overall(arm_dir):
    p = RUNS / arm_dir / "locomo_summary.json"
    if not p.exists():
        return None
    return json.load(open(p))


loaded = {a: load(d) for a, d in ARMS.items()}
missing = [a for a, r in loaded.items() if r is None]
if missing:
    print(f"MISSING arm data: {missing} — run not complete?")
    for a in missing:
        print(f"  expected: {RUNS / ARMS[a] / 'locomo_per_query.jsonl'}")
    if all(loaded[a] is None for a in ARMS):
        sys.exit(1)

print(f"=== ISS-188 AC-3 λ-sweep — STAMP {STAMP} ===\n")

# --- LIST-SF coverage (AC-3 primary) ---
print("LIST-type SF coverage (q13/15/18/19/24/32/34/38/39/47, n=10):")
print(f"  {'arm':<4} {'sum_score':>10} {'pass>=0.5':>10}")
cov = {}
for a in "ABC":
    if loaded[a] is None:
        print(f"  {a:<4} {'(missing)':>10}")
        continue
    s, p, n = coverage(loaded[a], LIST_SF)
    cov[a] = (s, p, n)
    print(f"  {a:<4} {s:>10.3f} {p:>4}/{n}")

# --- per-query LIST-SF breakdown ---
print("\n  per-query scores:")
hdr = "  " + "qid".ljust(14) + "".join(f"{a:>8}" for a in "ABC")
print(hdr)
for q in LIST_SF:
    cells = ""
    for a in "ABC":
        if loaded[a] and q in loaded[a]:
            cells += f"{loaded[a][q]['score']:>8.2f}"
        else:
            cells += f"{'--':>8}"
    print("  " + q.replace("conv-26-", "").ljust(14) + cells)

# --- single-value SF (AC-4 regression guard) ---
print("\nSingle-value SF coverage (q4/q7/q43, n=3) — AC-4 guard:")
sv = {}
for a in "ABC":
    if loaded[a] is None:
        continue
    s, p, n = coverage(loaded[a], SINGLE_SF)
    sv[a] = (s, p, n)
    print(f"  {a:<4} sum={s:.3f} pass={p}/{n}")

# --- conv-26 overall (AC-4 regression guard) ---
print("\nconv-26 overall (AC-4 guard):")
for a in "ABC":
    o = overall(ARMS[a])
    if o:
        print(f"  {a:<4} overall={o.get('overall')}")

# --- decision rule ---
print("\n=== Decision rule ===")
if "A" in cov and ("B" in cov or "C" in cov):
    base_pass = cov["A"][1]
    best_arm, best_pass = None, base_pass
    for a in ("B", "C"):
        if a in cov and cov[a][1] > best_pass:
            best_arm, best_pass = a, cov[a][1]
    lift = best_pass - base_pass
    print(f"  baseline (A) list-SF pass: {base_pass}/10")
    if best_arm:
        print(f"  best arm: {best_arm} list-SF pass: {best_pass}/10  (lift +{lift}/10)")
    else:
        print(f"  no arm beats baseline  (lift {lift}/10)")

    # single-value regression check
    sv_ok = True
    if best_arm and "A" in sv and best_arm in sv:
        if sv[best_arm][1] < sv["A"][1]:
            sv_ok = False
            print(f"  ⚠️  single-value SF regressed: A={sv['A'][1]}/3 → {best_arm}={sv[best_arm][1]}/3")

    if lift >= 3 and sv_ok:
        print(f"  → VERDICT: ship populate + arm {best_arm} λ as default (pending conv-44 AC-5)")
    elif 1 <= lift <= 2:
        print(f"  → VERDICT: opt-in only (lift {lift}/10), keep default off")
    else:
        print(f"  → VERDICT: FALSIFIED (lift {lift}/10) — problem is JUDGE or GENERATION, pivot to ISS-179")
