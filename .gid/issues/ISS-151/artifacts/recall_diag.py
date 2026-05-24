#!/usr/bin/env python3
"""ISS-151 recall diagnostic: categorise single-hop FAILS on conv-26.

Reads:
  - LoCoMo fixture (for gold + evidence)
  - Mode-B dump (for retrieved_candidates)

Outputs the 14/9/2 split:
  A. recall miss   — gold kw absent from top-K
  B. partial-list  — gold kw partially present (multi-item gold)
  C. wrong-fact    — gold kw fully present, generator failed

Usage:
  python3 recall_diag.py <run_dir>

Run record used in ISS-151:
  benchmarks/runs/ISS150-modeB-dump-conv26-20260524T042707Z/
"""
import json, re, sys, os, glob


FIXTURE = "/Users/potato/clawd/projects/engram-bench/benchmarks/fixtures/locomo/39e7df4ea492e8bc7a483b2cfc8e18620054beb05fed267f5cc098bd65fd5f4d/conversations.jsonl"


def load_fixture():
    with open(FIXTURE) as f:
        for line in f:
            c = json.loads(line)
            if c["conversation_id"] == "conv-26":
                return {q["id"]: q for q in c["questions"]}
    raise RuntimeError("conv-26 not in fixture")


def extract_keywords(gold: str):
    """Naive: alphanumeric tokens of len >= 4."""
    return [w.lower() for w in re.findall(r"\b[A-Za-z]{4,}\b", gold)]


def main(run_dir):
    candidates_jsonl = glob.glob(os.path.join(run_dir, "*", "locomo_per_query.jsonl"))
    if not candidates_jsonl:
        candidates_jsonl = glob.glob(os.path.join(run_dir, "locomo_per_query.jsonl"))
    if not candidates_jsonl:
        print(f"!! no per_query.jsonl under {run_dir}", file=sys.stderr)
        sys.exit(2)
    rp = candidates_jsonl[0]

    qmap = load_fixture()

    recall_miss = []
    partial_or_wrong = []
    with open(rp) as f:
        for line in f:
            r = json.loads(line)
            if not (r["id"].startswith("conv-26")
                    and r.get("category") == "single-hop"
                    and r["score"] == 0):
                continue
            q = qmap.get(r["id"], {})
            gold = q.get("gold", "")
            kws = extract_keywords(gold)
            if not kws:
                continue
            cands_text = " ".join(c["text"].lower() for c in r.get("retrieved_candidates", []))
            hits = [k for k in kws if k in cands_text]
            if not hits:
                recall_miss.append((r["id"], gold, kws))
            else:
                partial_or_wrong.append((r["id"], gold, hits, kws))

    print(f"## single-hop FAILS breakdown ({len(recall_miss) + len(partial_or_wrong)} total)\n")
    print(f"A. Retrieval recall miss        : {len(recall_miss):>2d}")
    print(f"B+C. Pool has evidence, fail    : {len(partial_or_wrong):>2d}")
    print()

    print("### A. Recall miss — gold kw absent from top-K\n")
    for qid, gold, kws in recall_miss:
        print(f"  {qid:15s} gold={gold[:50]!r:55s} kws={kws[:5]}")
    print()

    print("### B+C. Pool has at least one kw — partial-list or gen fail\n")
    for qid, gold, hits, kws in partial_or_wrong:
        kind = "partial-list" if len(hits) < len(kws) else "wrong-fact"
        print(f"  {qid:15s} [{kind:12s}] hit={hits} (of {len(kws)} kws) gold={gold[:50]!r}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <run_dir>", file=sys.stderr)
        sys.exit(1)
    main(sys.argv[1])
