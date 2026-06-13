#!/usr/bin/env python3
"""ISS-201 single-hop autopsy v3 — cardinality-aware + per-item pool check.

Key insight from v2: most 'single-hop' misses are LIST/SET golds whose items
are scattered across episodes. So we must:
  1. split gold into items (comma / 'and' / quotes)
  2. for each item, check if ANY candidate contains it
  3. classify by (is_list, items_found / items_total, gen said IDK?)

Buckets:
  LIST-PARTIAL   gold is a set; some items in pool, gen gave partial/IDK
                 -> aggregation/extraction: items not co-located, gen undercounts
  LIST-MISS      gold is a set; <50% items anywhere in pool -> items not stored
  ATOMIC-INPOOL  single fact, item IS in a candidate, gen refused/wrong -> gen/surface
  ATOMIC-MISS    single fact, not in any candidate -> retrieval/extraction drop
  DATE-STRAND    gold is a date; event in pool, date not in text
"""
import json, re
from collections import Counter

RUNDIR = "/Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS201-GUIDANCE-AB-conv26-20260610T190847Z/2026-06-10T19-51-41Z_locomo"
FIX = "/Users/potato/clawd/projects/engram-bench/benchmarks/fixtures/locomo/39e7df4ea492e8bc7a483b2cfc8e18620054beb05fed267f5cc098bd65fd5f4d/conversations.jsonl"
conv = json.loads(open(FIX).readline())
questions = {q["id"]: q for q in conv["questions"]}
rows = {json.loads(l)["id"]: json.loads(l) for l in open(f"{RUNDIR}/locomo_per_query_arm_a_off.jsonl")}

MISS = ['conv-26-q3','conv-26-q4','conv-26-q7','conv-26-q11','conv-26-q13','conv-26-q15',
'conv-26-q18','conv-26-q19','conv-26-q23','conv-26-q24','conv-26-q32','conv-26-q34',
'conv-26-q38','conv-26-q39','conv-26-q40','conv-26-q43','conv-26-q47','conv-26-q48',
'conv-26-q51','conv-26-q52','conv-26-q56','conv-26-q60','conv-26-q61','conv-26-q65',
'conv-26-q66','conv-26-q70','conv-26-q71','conv-26-q75','conv-26-q76','conv-26-q78']

DATE_RE = re.compile(r"\b(19|20)\d{2}\b|january|february|march|april|may|june|july|august|september|october|november|december", re.I)

def split_items(gold):
    g = gold.strip()
    # quoted titles -> each a unit
    quoted = re.findall(r'"([^"]+)"', g)
    if quoted:
        return [q.strip() for q in quoted]
    parts = re.split(r",|\band\b|/|;", g)
    return [p.strip().strip('".') for p in parts if p.strip() and len(p.strip())>1]

def item_in_pool(item, cands):
    key = [w for w in re.findall(r"[a-z0-9]+", item.lower()) if len(w)>2]
    if not key: 
        key = [item.lower()]
    blob = " ".join(c.get("text","").lower() for c in cands)
    # an item is "in pool" if its most-distinctive word appears
    return any(w in blob for w in key)

def classify(qid):
    q = questions[qid]; r = rows.get(qid,{})
    gold = q["gold"]; pred = r.get("predicted",""); cands = r.get("retrieved_candidates",[])
    items = split_items(gold)
    is_list = len(items) >= 2
    found = [it for it in items if item_in_pool(it, cands)]
    frac = len(found)/len(items) if items else 0
    pred_idk = "don't know" in pred.lower() or "do not know" in pred.lower()
    g_date = bool(DATE_RE.search(gold)) and not is_list and len(re.findall(r"[a-z]+",gold.lower()))<=3
    if g_date:
        b="DATE-STRAND"
    elif is_list:
        b = "LIST-PARTIAL" if frac>=0.5 else "LIST-MISS"
    else:
        if frac>0:
            b = "ATOMIC-INPOOL"
        else:
            b = "ATOMIC-MISS"
    return b, gold, q["question"], pred, items, found, frac

buckets=Counter(); rowsout=[]
for qid in MISS:
    b,gold,quest,pred,items,found,frac=classify(qid)
    buckets[b]+=1
    rowsout.append((qid,b,gold,quest,pred,items,found,frac))

print("=== BUCKET TALLY (n=%d) ==="%len(MISS))
for k,v in buckets.most_common(): print(f"  {k:14} {v}")
print()
for qid,b,gold,quest,pred,items,found,frac in rowsout:
    print(f"{qid:13} [{b:13}] items={len(items)} found={len(found)} ({frac:.0%})")
    print(f"      Q: {quest[:72]}")
    print(f"      gold={gold!r}  missing={[i for i in items if i not in found]}")
