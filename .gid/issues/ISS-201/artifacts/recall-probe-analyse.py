#!/usr/bin/env python3
"""Pure bi-encoder recall@K analysis for iss186 probe dumps.

For each query, find where the gold answer's evidence sits in the
top-200 bi-encoder ranking. Bucket:
  A = gold in top-10
  B = gold in top-11..50
  C = gold in top-51..200
  D = gold not in top-200 (or not ingested)

Gold matching is fuzzy + date-normalized, because LoCoMo gold is the
ANSWER string (e.g. "7 May 2023") while memory text carries the fact
in different surface form (e.g. "[2023-05-08] ... (2023-05-07)").
"""
import json, sys, re, calendar
from collections import defaultdict

MONTHS = {m.lower(): i for i, m in enumerate(calendar.month_name) if m}
MONTHS.update({m.lower(): i for i, m in enumerate(calendar.month_abbr) if m})

def norm_dates(s):
    """Extract a set of normalized YYYY-MM tokens + bare years from text."""
    s = s.lower()
    toks = set()
    # ISO dates 2023-05-08
    for y, mo in re.findall(r'(\d{4})-(\d{2})', s):
        toks.add(f"{y}-{mo}")
        toks.add(y)
    # "7 may 2023" / "may 2023" / "may 7 2023"
    for mname, mnum in MONTHS.items():
        for m in re.finditer(rf'\b{mname}\b\s*(\d{{4}})?', s):
            y = m.group(1)
            if y:
                toks.add(f"{y}-{mnum:02d}")
                toks.add(y)
    # bare 4-digit years
    for y in re.findall(r'\b(19|20)\d{2}\b', s):
        pass
    for y in re.findall(r'\b((?:19|20)\d{2})\b', s):
        toks.add(y)
    return toks

def tokens(s):
    return set(re.findall(r'[a-z0-9]+', s.lower()))

def gold_hits(gold, text):
    """True if `text` plausibly contains the gold answer's evidence."""
    g = str(gold).strip().lower()
    t = text.lower()
    if not g:
        return False
    # 1) direct substring
    if g in t:
        return True
    # 2) date-normalized overlap (for temporal golds)
    gd = norm_dates(g)
    if gd:
        td = norm_dates(text)
        if gd & td:
            return True
    # 3) content-word overlap: all gold non-stopword tokens present
    STOP = {"the","a","an","of","to","in","on","at","and","or","is","was",
            "were","for","with","her","his","their","they","she","he","it",
            "about","that","this","once","times","time","year","years"}
    gt = tokens(g) - STOP
    if gt and gt <= tokens(text):
        return True
    return False

def bucket(rank):
    if rank is None: return "D"
    if rank <= 10:   return "A"
    if rank <= 50:   return "B"
    return "C"

def main(path):
    by_cat = defaultdict(lambda: defaultdict(int))
    total = defaultdict(int)
    rows = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line: continue
            d = json.loads(line)
            cat = d.get("category","?")
            gold = d.get("gold","")
            evidence = d.get("evidence")
            top = d.get("top", d.get("top200", d.get("top_200", [])))
            # match against evidence episodes if present (more precise),
            # else fall back to gold answer string
            ev_texts = []
            if isinstance(evidence, list):
                for e in evidence:
                    ev_texts.append(e if isinstance(e,str) else json.dumps(e,ensure_ascii=False))
            elif isinstance(evidence, str):
                ev_texts = [evidence]
            hit_rank = None
            for c in top:
                ct = c.get("text","")
                if gold_hits(gold, ct) or any(gold_hits(ev, ct) for ev in ev_texts):
                    hit_rank = c.get("rank")
                    break
            b = bucket(hit_rank)
            by_cat[cat][b] += 1
            by_cat["ALL"][b] += 1
            total[cat] += 1
            total["ALL"] += 1
            rows.append((d.get("qid"), cat, b, hit_rank, gold))
    print("="*64)
    print("PURE BI-ENCODER RECALL (top-200), gold-evidence bucketed")
    print("  A=top-10  B=11-50  C=51-200  D=missed/not-ingested")
    print("="*64)
    for cat in ["single-hop","multi-hop","open-domain","temporal","ALL"]:
        if cat not in total: continue
        n = total[cat]
        a,bb,c,dd = (by_cat[cat][x] for x in "ABCD")
        r10  = a/n if n else 0
        r50  = (a+bb)/n if n else 0
        r200 = (a+bb+c)/n if n else 0
        print(f"\n{cat}  (n={n})")
        print(f"  recall@10  = {r10:.3f}   (A={a})")
        print(f"  recall@50  = {r50:.3f}   (+B={bb})")
        print(f"  recall@200 = {r200:.3f}   (+C={c})")
        print(f"  MISSED (D) = {dd}  ({dd/n:.3f})")
    # D-bucket detail = write-side / extraction failures
    print("\n" + "="*64)
    print("D-bucket (gold NOT in top-200 = write/extraction failure):")
    for qid,cat,b,hr,gold in rows:
        if b=="D":
            print(f"  {qid} [{cat}] gold={gold!r}")

if __name__ == "__main__":
    main(sys.argv[1])
