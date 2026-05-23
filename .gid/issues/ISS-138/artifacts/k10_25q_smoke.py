#!/usr/bin/env python3
"""
ISS-138 AC #3 best-effort: K=10 25q smoke envelope check.

We don't have a cogmembench K=10 reference to diff against, so the
literal "verdict-mismatch ≤ 1/25 vs cogmembench" check is unverifiable
today. The closest signal we can produce is K=10-run-vs-run verdict
stability on the same 25-question slice: if K=10 is internally stable
at the smoke scale, it's plausibly within the ISS-100 envelope.
"""
import json
import pathlib
import sys

BASE = pathlib.Path("/Users/potato/clawd/projects/engram-bench/benchmarks/runs")
RUNS = {
    "K10_r1": BASE / "ISS069-k10-temp0-20260523T122707Z" / "2026-05-23T12-39-54Z_locomo" / "locomo_per_query.jsonl",
    "K10_r2": BASE / "ISS069-k10-temp0-run2-20260523T124006Z",
    "K10_r3": BASE / "ISS069-k10-temp0-run3-20260523T125250Z",
}

def find_jsonl(p: pathlib.Path) -> pathlib.Path:
    if p.is_file():
        return p
    for child in p.rglob("locomo_per_query.jsonl"):
        return child
    raise FileNotFoundError(p)

def load(p):
    rows = []
    with open(find_jsonl(p)) as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows

runs = {name: load(path) for name, path in RUNS.items()}
n = min(len(r) for r in runs.values())
print(f"Loaded runs: {{ {', '.join(f'{k}={len(v)}q' for k,v in runs.items())} }}, n={n}")

# Sort each by id to align
for name in runs:
    runs[name] = sorted(runs[name], key=lambda r: r["id"])

# Take first 25 by id-order = smoke envelope slice
SMOKE = 25
slices = {name: rows[:SMOKE] for name, rows in runs.items()}
ids = [r["id"] for r in slices["K10_r1"]]
print(f"\n25q smoke slice (sorted by id), first 5 ids: {ids[:5]}")
print(f"Categories: {sorted(set(r['category'] for r in slices['K10_r1']))}")

# Verdict stability: pairwise score-disagree count
def score_mismatch(a_rows, b_rows):
    diffs = []
    for a, b in zip(a_rows, b_rows):
        assert a["id"] == b["id"]
        if a["score"] != b["score"]:
            diffs.append({
                "id": a["id"],
                "category": a["category"],
                "score_a": a["score"],
                "score_b": b["score"],
                "verdict_a": a["verdict_raw"][:60] if a.get("verdict_raw") else "",
                "verdict_b": b["verdict_raw"][:60] if b.get("verdict_raw") else "",
            })
    return diffs

print("\n=== Pairwise K=10 verdict stability on 25q slice ===")
for a_name, b_name in [("K10_r1", "K10_r2"), ("K10_r1", "K10_r3"), ("K10_r2", "K10_r3")]:
    diffs = score_mismatch(slices[a_name], slices[b_name])
    print(f"{a_name} vs {b_name}: {len(diffs)}/{SMOKE} score flips" + (" ✅ ≤1/25" if len(diffs) <= 1 else " ❌ >1/25"))
    for d in diffs:
        print(f"   {d['id']:>10} [{d['category']}] {d['score_a']} vs {d['score_b']}  ({d['verdict_a']!r} | {d['verdict_b']!r})")

# Aggregate score over smoke slice
print("\n=== Aggregate score on 25q smoke ===")
for name, rows in slices.items():
    mean = sum(r["score"] for r in rows) / len(rows)
    by_cat = {}
    for r in rows:
        by_cat.setdefault(r["category"], []).append(r["score"])
    cat_str = " | ".join(f"{c}: {sum(v)/len(v):.3f} (n={len(v)})" for c, v in sorted(by_cat.items()))
    print(f"  {name}: mean={mean:.4f}  [{cat_str}]")

# Honesty note
print("\n--- Note ---")
print("This is NOT the literal ISS-100 AC #3 check. That requires a cogmembench")
print("K=10 reference run; we don't have one. This is a K=10-run-vs-run stability")
print("proxy on the first 25 questions of conv-26 (sorted by id).")
