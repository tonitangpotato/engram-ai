---
id: ISS-084
title: "Investigation: should SA be engram's path to multi-hop, given Shodh/SA-RAG prior art?"
status: open
priority: P1
severity: high
tags: [retrieval, multi-hop, spreading-activation, strategy, investigation]
created: 2026-04-30
relates_to: [ISS-070, ISS-083]
depends_on: [ISS-085]
blocks: [ISS-070]
---

# Investigation: should SA be engram's path to multi-hop?

## Why this is open

Over 2026-04-30 we did three things:

1. **Prototyped SA** (`crates/engram-bench/examples/spreading_activation_prototype.rs`) — got 1/3 on the in-window multi-hop set. Failure mode = **question-blind anchor resolution**: only the entity token "Caroline" was anchored, three different intent words (research / identity / relationship) produced **identical activation traces**.
2. **Did prior art** (`PRIOR-ART-2026-04-30-spreading-activation.md`) and discovered we were wrong about the field being empty:
   - **SA-RAG** (NeurIPS 2025-12) — full SA implementation; +39% on MuSiQuE/2WikiMultiHop. Doc-QA only, not conversational.
   - **Shodh-memory** (crates.io v0.1.90, npm, pypi) — **structurally identical to engram** (Rust core, Hebbian, Cowan 3-tier, power-law decay, MCP server, "neuroscience-grounded"). Their public SA code looks ≈90% the same as our prototype. They do NOT publish LoCoMo numbers.
   - **Cognee, Graphiti/Zep, Mem0, Letta** — all ❌ no SA.
3. **Ran RUN-0009 full conv-26** without SA wired in. cat=1 (multi-hop) = 30.3% hit@5; that's our floor.

We then **paused** SA integration and said "redesign anchor resolution before continuing." That decision was made but never written down, and we never picked a redesign strategy. This issue is to do that explicitly.

## The actual question

**Is SA the right paradigm to bet engram's multi-hop story on, given that:**

- The paradigm is published twice already (SA-RAG, Shodh).
- Nobody has demonstrated SA works on conversational/temporal multi-hop (LoCoMo-style). SA-RAG only tested doc-QA. Shodh tested nothing publicly.
- Our 1/3 result is consistent with "question-blind anchors give you 33% by accident" — it's not evidence that the paradigm is wrong, but it's also not evidence it's right. We learned nothing about whether predicate-aware SA would clear the bar.

There are three coherent paths. Pick one.

---

## Path A — Predicate-Aware SA (differentiated SA bet)

**Hypothesis:** SA paradigm is right for multi-hop. The reason it stalls in the wild is that everyone's anchor + diffusion is question-blind. **Engram's edge in v0.3 is the typed predicate graph (`leads_to`, `uses`, `depends_on`, `is_a`, …) which neither SA-RAG nor Shodh exploits during diffusion.** If we condition edge conductance on question-extracted predicate hints, we get a mechanism literally nobody else has, and it directly addresses the question-blindness failure mode.

**Concrete spec sketch (for the redesign doc, not this issue):**
1. **Anchor resolution v2:** vector top-K seeds (k=3-5 from query embedding) ∪ entity NER seeds. Dedup. This alone breaks the "Caroline is the only seed" trap.
2. **Predicate hint extraction:** simple LLM-or-classifier step on the query → list of relevant predicate types with weights (`research?` → `uses`:0.8, `depends_on`:0.6, `leads_to`:0.3).
3. **Diffusion with conditional conductance:** `next_act = act * edge.strength * predicate_boost(edge.predicate, query_hints) * decay`. Falls back to uniform when no hints (so it's never *worse* than vanilla SA).
4. **Post-activation pruning by query cosine** (SA-RAG style) — final sanity filter.
5. K_HOP=3 default. Wire into `RetrievalEngine::MultiHopPlan` (ISS-070).

**Pros:**
- Genuinely novel — not a re-implementation of Shodh.
- Failure mode (question-blindness) is identified and addressed by design, not handwaved.
- Predicate graph already exists in v0.3 substrate; no schema work.
- If it lands, the engram pitch becomes "first cognitive memory with predicate-aware multi-hop" — defensible.

**Cons / risks:**
- We don't know if predicate hints actually transfer to better recall. The +39% in SA-RAG came from KG quality + better seeding, not predicate awareness — so we'd be betting on a mechanism with **zero published precedent**.
- Predicate hint extraction is itself a model call per query — adds latency, can be wrong.
- Designing anchor v2 + predicate boost properly is probably 2-3 weeks of design + impl + eval.
- Even if it works on conv-26, generalising to other LoCoMo conversations means more KG quality work — engram's KG is auto-extracted (LLM), so quality varies.

**Cost to find out:** ~3 weeks (design doc, impl, run RUN-0010 with SA on, compare cat=1 vs RUN-0009).

**What "success" looks like:** cat=1 (multi-hop) jumps from 30.3% → ≥50% on conv-26 with SA on. Anything less is not worth the differentiation argument.

---

## Path B — Skip SA, optimise the substrate we have

**Hypothesis:** SA is a distraction. The 30.3% cat=1 number probably has a lot of headroom from **non-SA** sources we haven't optimised:

- **Hybrid plan downgrade fix** (ISS-083) — 10/199 queries silently empty, mostly cat=4/5 paraphrased pairs. Free recovery.
- **Episodic plan** — currently selects 5 times in RUN-0009 and hits 0/5. The episodic substrate exists, the plan dispatch is broken. Free recovery.
- **Better re-ranker** — current dispatcher returns Factual top-K by embedding similarity. Adding a cross-encoder rerank (already in the v0.3 design) could move multi-hop directly because the gold *is* often in candidates 6-20, just below k=5 cutoff.
- **Multi-query expansion** — for cat=1, generate 2-3 sub-queries and union recalls. No graph traversal needed, dispatched as Factual×N, fuse with RRF.

**Pros:**
- Each step is small, scoped, and has clear AC.
- We get to publishable J score faster (LLM-judge wired up + the four small fixes).
- Engram's differentiator becomes **substrate quality + temporal/episodic mechanics** (path 6.1.a in PRIOR-ART), which is real and harder to copy than an algorithm.
- If after all this cat=1 is still bad, we have **clean evidence** that the substrate isn't enough and SA is needed — i.e., A becomes a *justified* bet rather than a speculative one.

**Cons:**
- "Engram is a memory system with rerank + multi-query" isn't a sexy pitch. Closer to Mem0 + better substrate.
- We give up the "predicate-aware SA" novelty narrative.
- Headroom is unknown — multi-query and rerank might also cap out at 40%, not 50%+.

**Cost to find out:** ~1.5 weeks (4 fixes + RUN-0010 with rerank/multi-query).

**What "success" looks like:** cat=1 climbs from 30.3% → ≥45% with no graph traversal whatsoever. If we get there, SA becomes optional optimisation, not core. If we don't, A becomes the next bet with solid evidence.

---

## Path C — Pure measurement first, decide later

**Hypothesis:** We can't actually pick A or B intelligently right now because the only measurement we have is recall@5. **We don't know what fraction of the 30.3% gap closes from any of these levers** because we haven't measured the upstream / downstream structure of the failures.

**Concrete next step:**
1. Wire engram retriever as a `cogmembench` adapter for conv-26 → get **J score** with LLM judge.
2. **Per-failure analysis** on the 23 cat=1 misses: for each miss, classify as
   - (i) gold not in top-20 candidates (substrate or recall ceiling)
   - (ii) gold in top-20 but below 5 (rerank would fix)
   - (iii) gold needs 2+ hops *and* a predicate filter (only SA fixes)
   - (iv) classification picked wrong plan (router fix)
3. Distribution of (i)/(ii)/(iii)/(iv) tells us which path has the most leverage.

**Pros:**
- Cheapest. ~3-4 days. Adapter + judge + manual classification of 23 questions.
- Prevents both "we built fancy SA when rerank would have been enough" and "we shipped Mem0-clone when SA would have been the moat".
- Gives us our first cross-system comparison number (the J score), which we need anyway.

**Cons:**
- Doesn't ship anything user-facing.
- If the distribution is mixed (likely), we still have to pick A or B at the end.

**Cost to find out:** 3-4 days.

---

## What I think (not the decision, just input)

C first, then A or B based on what C tells us. Reasons:

- We've already burned a day on prior art and a prototype. We do not have the data to pick between A and B intelligently. C is cheap and disambiguates.
- The J number from C is necessary regardless — without it we can't compare to anything published.
- If C shows distribution heavily in (iii) → A is justified. If heavily (i)/(ii)/(iv) → B is sufficient. If mixed → we know to start with B (cheap wins) and revisit A when the cheap wins are exhausted.
- Skipping C means we'll re-litigate A vs B in 2 weeks, this time with sunk cost on whichever we picked.

But: if you have a strong product/strategic preference (e.g., "predicate-aware SA is the moat, just go") that overrides this. The cheapest-information argument is correct in expectation, not in every world.

## Decision needed

Pick A, B, or C. If C, the next concrete tasks are:

- ISS-XXX: Wire engram retriever as cogmembench adapter for conv-26 (depends on cogmembench api — can be split).
- ISS-XXX: Per-question failure classification on RUN-0009 cat=1 misses (manual, ~half day).
- After both: re-open this issue to pick A or B.

## Out of scope

- Implementing any of A/B/C in this issue. This is the **decision** issue.
- Designing anchor resolution v2 (that's a design doc that A would spawn).
- Comparing engram to Shodh on Shodh's own benchmark (they don't publish one).

## References

- Prior art: `.gid/features/v03-retrieval/PRIOR-ART-2026-04-30-spreading-activation.md`
- SA prototype investigation: `.gid/features/v03-retrieval/INVESTIGATION-2026-04-30-spreading-activation-status.md`
- RUN-0009 report: `.gid/eval-runs/RUN-0009-substrate/RUN-0009-full-conv-report.md`
- Related: ISS-070 (multi-hop dispatcher), ISS-083 (Hybrid empty downgrade), ISS-076 (substrate fixes from morning)
