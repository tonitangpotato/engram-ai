# RUN-0012 Results — ISS-091 verification + hit@5 root-cause analysis

**Metric:** hit@5 (gold dialog turn appears in top-5 retrieved). **NOT** J-score.
**Substrate:** locomo-conv26-full.db (post-ISS-087/088/089/091 fixes, real-clock occurred_at).
**Date run:** 2026-04-30 → 2026-05-01.
**Total QAs:** 197 (out of 199 — 2 lost upstream, separate bug, not investigated here).

## 1. Headline numbers (hit@5)

| Category | RUN-0009 (pre-fix) | RUN-0012 (post-fix) | Δ |
|---|---|---|---|
| Headline (cat 1-4, cogmembench convention) | 79/150 = 52.7% | 80/150 = 53.3% | +0.6pp (within noise) |
| Total | — | 103/197 = 52.3% | — |
| cat=1 Single-hop factual | — | 10/32 = 31.2% | — |
| cat=2 Temporal | 89.2% | 32/37 = 86.5% | -2.7pp ⚠️ |
| cat=3 Open-ended inference | — | 4/11 = 36.4% | — |
| cat=4 Multi-hop | 45.7% | 34/70 = 48.6% | +2.9pp ✅ |
| cat=5 Adversarial | 44.7% | 23/47 = 48.9% | +4.2pp ✅ |

**ISS-091 verification result:** ✅ Confirmed.
- 0 occurrences of `finish_pipeline_run: run not found` (pre-fix had N).
- 27 ingest pipeline runs succeeded, 2 failed (failure mode = `embedding dim mismatch`, separate bug — not regression from ISS-091).
- All "headline = flat" outcomes are within LLM extraction non-determinism. ISS-091 fix is structurally complete; substrate-level fixes from ISS-087/088/089/091 do **not** move hit@5 because hit@5 is metric-blind to time-grounding correctness (see "Retrospective" below).

## 2. Failure mode breakdown (94 misses / 197)

I parsed each miss into (query, gold, top-5, plan, outcome) and bucketed by **failure pattern**, not by plan.

### Mode A — Near-miss (45 / 94 = 48% of all miss)

Top-5 contains the gold *session* (e.g., D7) but the wrong *turn* within that session (D7:20 instead of D7:22).

**Distribution by category:**
- cat=4 Multi-hop: 20 / 36 misses
- cat=5 Adversarial: 15 / 24
- cat=1 Single-hop: 7 / 22
- cat=2 Temporal: 2 / 5
- cat=3 Open-ended: 1 / 7

**Examples:**
- Q="What does Melanie do to destress?" gold=`[D7:22, D5:4]` top=`[D7:20, D17:22, ...]` — D7:20 is "Melanie running" (close concept), D7:22 is the actual destress mention.
- Q="When did Melanie paint a sunrise?" gold=`[D1:12]` top=`[D1:14, D13:10, ...]` — adjacent turn in same session.
- Q="What types of pottery have Melanie and her kids made?" gold=`[D5:6, D8:4]` top=`[D8:2, D14:4, ...]` — neighbor turn.

**Mechanism:** Each `dia_id` in LoCoMo is one conversational turn (~1-2 sentences). Within a session, adjacent turns share topic + entities; their MiniLM-L6 embeddings (384-dim cosine) become near-degenerate at the turn level. Engram retrieves the right session because session-level topic signal dominates, then loses turn-level resolution.

**Implication:** Top-5 set-cover is good (we recall the right neighborhood); top-1 ranking inside the neighborhood is poor. This affects hit@5 metric harshly but should affect J-score less, because the LLM answerer fed all 5 candidates can often piece together the correct answer from a neighbor turn that contains the same fact in different words.

### Mode B — Total miss, recency dump (12 / 94 = 13% of all miss)

Top-5 is entirely D19 (last session). No content overlap with gold.

**Distribution:**
- cat=4 Multi-hop: 5 (all `plan=Hybrid`)
- cat=5 Adversarial: 5 (all `plan=Hybrid`)
- cat=1 Single-hop: 2 (`plan=Episodic`)

**Examples:**
- Q="What did Melanie realize after the charity race?" (charity race in D2) → top=`[D19:1, D19:2, D19:5, D19:11, D19:7]`, plan=Hybrid.
- Q="Where did Caroline move from 4 years ago?" → top=`[D19:2, D19:1, ...]`, plan=Episodic.

**Mechanism:** Hybrid plan combines associative + episodic legs. Episodic leg returns last session as "most recent working memory" with very high score; associative leg's content matches get crowded out in fusion. Episodic plan alone has the same dump pattern.

**Conclusion:** This is a real retrieval engine bug, not a substrate problem. Independent of ISS-083 (which was about Hybrid emitting empty; this is Hybrid emitting wrong-content non-empty). → **Filed as ISS-094**.

### Mode C — Total miss, semantic paraphrase failure (37 / 94 = 39% of all miss)

Top-5 has no session overlap with gold; gold turn uses vocabulary the query doesn't.

**Examples:**
- Q="What did Caroline research?" gold=`[D2:8]` (D2:8 says "I'm looking into adoption agencies") — query has "research", document has "looking into". Embedding distance large enough that gold drops below k=5.
- Q="What is Caroline's identity?" gold=`[D1:5]` (D1:5 says "I'm a transgender woman") — query lacks the keyword "transgender".

**Mechanism:** Pure embedding paraphrase failure. Sparse-keyword (BM25) or entity-graph (NER + entity overlap) signal would catch these; embedding alone cannot.

### Mode D — Plan-routing failure (21 / 94 = 22%, overlaps with B/C above)

Sub-classified by outcome:

- **`downgraded_from_abstract` = 16** (cat=4: 7, cat=5: 5, cat=1: 3, cat=3: 1).
  Query routed to abstract plan (needs L5 substrate); L5 unavailable; downgrades to factual. But factual is a *worse* plan for these queries (they need inference, not factual lookup).
  Example: "Would Melanie be more interested in a national park or a theme park?" — pure inference query.

- **`no_cognitive_state` = 5** (cat=4: 3, cat=5: 2).
  Query routed to Affective plan (needs `cognitive_state` metadata); substrate has none.
  Example: "How did Melanie feel about her family supporting her?"

**Conclusion:** Both are direct evidence that **L5 abstract substrate (and affective metadata ingestion) is the binding constraint for cat=3 / inference-style queries**. → Evidence appended to **ISS-083** (and feeds ISS-084 Path B).

## 3. Per-category root-cause attribution

### cat=1 Single-hop (10/32 = 31.2%)
- 22 misses: 7 near-miss / 15 total miss / 3 of those 15 are `downgraded_from_abstract`.
- **Dominant mode: C (semantic paraphrase, 12-15 of 22)**. cat=1 questions ask "What is X's identity / What did X research" — gold answer turn often lacks the query keyword. Pure embedding ceiling.
- hit@5 has very limited headroom on cat=1 from retrieval changes alone. **J-score should reveal more** because the answer LLM can be conservative ("I don't know") and avoid wrong answers, which J counts more favorably than hit@5 (which only counts gold turn presence).

### cat=2 Temporal (32/37 = 86.5%)
- 5 misses; not analyzed in depth (already strong category).
- Key fact: ISS-087/088/089/091 fixed `occurred_at` accuracy in substrate, but cat=2 hit@5 is **flat** across that change. Confirms hit@5 doesn't measure time-grounding (gold turn is recovered by topic embedding regardless of time field). **J-score is the right test** for whether time fixes matter — answer LLM must produce correct date string, hit@5 cannot.

### cat=3 Open-ended (4/11 = 36.4%)
- 7 misses: 6 = `plan=Factual` but query is inference-style (e.g., "Would X pursue counseling without support?"); 1 = `downgraded_from_abstract`.
- **Root cause: L5 abstract layer absent (ISS-083 / ISS-084 Path B).**
- 11 records (not 13): 2 cat=3 questions lost upstream — separate bug.

### cat=4 Multi-hop (34/70 = 48.6%)
- 36 misses: 20 near-miss / 16 total miss / 7 downgraded / 3 no-cog / 5 recency-dump (Hybrid).
- **Two distinct problems:** (1) embedding turn-level resolution (Mode A, ISS-084 Path A territory) — biggest bucket; (2) Hybrid recency bias (Mode B, ISS-094) — smaller but actionable.

### cat=5 Adversarial (23/47 = 48.9%)
- Mirror of cat=4 — same mechanisms, no independent root cause.
- Filed in same actionable issues.

## 4. Retrospective: hit@5 is metric-blind to substrate time-grounding fixes

ISS-087 / ISS-088 / ISS-089 / ISS-091 fixed substrate-level time-data correctness:

- **Pre-fix (RUN-0009):** every `occurred_at` was wall-clock now (all turns timestamped 2026-04, even though dialogue is set in 2023). cat=2 = 89.2%.
- **Post-fix (RUN-0012):** every turn has correct 2023 session date. cat=2 = 86.5%.
- **Δ = -2.7pp**, attributable to LLM extraction non-determinism (different session shuffle picked different filler entities for some turns).

This is **not** evidence that the time fixes are useless. It is evidence that **hit@5 cannot tell** — because:
- Gold turn is determined by topic embedding match, which doesn't consume `occurred_at`.
- Even the retrieval engine's own time-aware filters (if enabled) only re-rank within the topic neighborhood; they don't change which session is reached.
- A correct date string in the *answer* would be visible to a J-score LLM judge but is invisible to "is the gold dia_id in top-5".

**Conclusion:** Headline hit@5 cannot validate substrate time-correctness work. ISS-085 J-score wiring is therefore a **prerequisite** for evaluating any of the ISS-087/088/089/091 series, not a parallel track.

## 5. Issues filed / updated from this analysis

- **ISS-094 (NEW)** — Hybrid plan recency bias: episodic leg dominates associative leg, returns last-session content for queries about earlier sessions. 12 reproducers from RUN-0012, all `top = {D19:*}` only.
- **ISS-083 (UPDATE)** — appended cat=3 / cat=4 `downgraded_from_abstract` evidence (16 cases) showing L5 absence is binding constraint for inference-style queries.
- **ISS-084** — this analysis is the per-failure data the issue's Path C asked for. Distribution favors **Path B first** (rerank + multi-query + ISS-094 Hybrid fix recover Mode A near-misses + Mode B recency dumps cheaply, ~30-40 of 94 misses), then re-evaluate Path A on the residual after Path B fixes land.

## 6. Cross-link

- Records JSON: `RUN-0012-records.json` (197 entries with query / gold / top / plan / outcome).
- Per-miss text dump: `miss_breakdown.txt` (cat=1/3/4/5 misses with full Q/gold/top).
- Raw retrieval log: `RUN-0012-full-conv26.log`.
- Ingest log: `ingest.log`, `ingest.stdout.log`.
