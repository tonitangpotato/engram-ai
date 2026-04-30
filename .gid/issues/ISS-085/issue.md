---
id: ISS-085
title: Wire engram retriever as cogmembench adapter for conv-26 (LLM-as-judge J score)
status: open
priority: P0
severity: high
tags: [retrieval, evaluation, cogmembench, locomo, j-score]
created: 2026-04-30
relates_to: [ISS-084, ISS-083]
blocks: [ISS-084]
---

# Wire engram as cogmembench adapter → J score on conv-26

## Why P0

We currently have **no number that's comparable to any other memory system.** RUN-0009's 52.7% is recall@5 of the gold dialog; published LoCoMo numbers (Mem0, MemGPT, Letta, Graphiti) are all **J score** (LLM-judged answer correctness). They are not the same metric. Until we have J:

- We can't claim engram is competitive with anything.
- We can't intelligently pick between Path A / B / C in ISS-084 (per-failure analysis needs J + answer + judge to know whether each miss is recall-bound or reasoning-bound).
- We can't tell whether ISS-083 / Episodic / rerank fixes actually move end-to-end quality, only whether they move recall@5.

Cost: ~3-4 days. Cost of *not* doing it: we keep arguing about strategy on a metric (recall@5) that doesn't disambiguate the strategies.

## Background

`cogmembench` (separate repo) already implements the LoCoMo evaluation pipeline:
- ingest → memory backend → retrieve top-k → answer LLM → judge LLM → J score
- supports multiple backends via adapter trait (Mem0, MemGPT, …)
- judge prompt and metric definition match the LoCoMo paper

We need to plug `engramai::retrieval::RetrievalEngine` in as another backend. Then run the same evaluation that produced RUN-0009's recall@5 numbers, but with answer + judge stages added.

## Concrete tasks

### 1. Adapter trait alignment

- [ ] Read cogmembench's existing adapter trait (likely `MemoryBackend` or `MemoryAdapter`) — find what methods are required (`ingest(messages)`, `query(question, k) -> candidates`, `reset()`, possibly `cleanup()`)
- [ ] Verify trait async/sync, return types, error model
- [ ] If trait is in cogmembench repo only and engram can't import it directly → either (a) duplicate the trait in engram with `cogmembench-adapter` feature flag, or (b) add engram as cogmembench dependency. Pick whichever has fewer cycles.

### 2. Implement engram adapter

- [ ] New crate or feature: `engram-cogmembench-adapter` (location TBD — probably in cogmembench repo, depending on engramai)
- [ ] `ingest`: build namespace, run pipeline that produces locomo-conv26-full DB equivalent (or take pre-built namespace as fixture, faster for iteration)
- [ ] `query`: call `RetrievalEngine::retrieve(query, ns, k=5)` → return candidates in cogmembench's expected format (text, source_id, score)
- [ ] `reset` / `cleanup`: optional namespace teardown

### 3. Wire judge

- [ ] Confirm cogmembench has a working judge — read `judge.rs` or equivalent
- [ ] Use the same answer model that comparable papers use (gpt-4o-mini probably, match Mem0 paper)
- [ ] Use the LoCoMo paper judge prompt; do NOT roll our own
- [ ] Sanity check: run on 5 hand-picked QAs first, verify judge output is reasonable (true/false on obvious cases)

### 4. Run RUN-0010 = RUN-0009 + judge

- [ ] Same substrate (locomo-conv26-full DB), same 199 QAs, same k=5
- [ ] Add answer-generation stage: top-5 candidates + question → answer model → answer string
- [ ] Add judge stage: question + answer + gold → judge model → bool
- [ ] Compute: J overall, J by category (cat=1..5), J among recall@5 hits vs misses
- [ ] Cost estimate: 199 × 2 LLM calls × $0.0001 ≈ $0.04 per run with gpt-4o-mini. Negligible.

### 5. Per-failure analysis (feeds ISS-084 decision)

- [ ] For each of the 23 cat=1 (multi-hop) misses, manually classify:
  - (i) gold not in top-20 candidates (substrate / recall ceiling)
  - (ii) gold in top-20 but below 5 (rerank would fix)
  - (iii) gold needs 2+ hops AND predicate filter (only SA fixes)
  - (iv) classification picked wrong plan (router fix)
- [ ] Tabulate distribution. Report in a small markdown doc under `.gid/eval-runs/RUN-0010-substrate-judge/per-failure-cat1.md`.
- [ ] This distribution is the **decision input** for ISS-084.

### 6. Report

- [ ] Write `.gid/eval-runs/RUN-0010-substrate-judge/RUN-0010-report.md`
- [ ] Sections: setup, J overall, J by category, J vs recall@5 (correlation), per-failure analysis, comparison table vs published Mem0/MemGPT/Letta numbers (with caveats — different conv subsets etc.)
- [ ] **Honest framing**: report the comparison but flag every place where our setup differs from published — different judge, different k, different conv subset, different ingestion pipeline, etc.

## Acceptance criteria

- [ ] cogmembench can run `engram` as a backend on the same locomo-conv26-full substrate as RUN-0009
- [ ] J score is computed for all 199 QAs
- [ ] Report exists with J overall, J by category, recall@5 vs J correlation
- [ ] Per-failure analysis on 23 cat=1 misses with (i)/(ii)/(iii)/(iv) distribution
- [ ] One side-by-side comparison table with at least one published baseline (Mem0 if numbers available; otherwise note "no comparable public number")

## Out of scope

- Building a *new* benchmark — we use cogmembench's existing one
- Tuning engram retrieval to game J — RUN-0010 must use the same retrieval config as RUN-0009
- Any of Path A / B / C from ISS-084 — they happen *after* RUN-0010 informs the decision

## Open questions

- Where does the adapter live: engram crate (with feature flag) or cogmembench repo? → **propose: cogmembench repo, depends on `engramai`**, keeps engram crate clean
- Which answer model: gpt-4o-mini (cheap, fast, matches Mem0 paper) or gpt-4o (better answers, $$)? → **propose: gpt-4o-mini for RUN-0010, gpt-4o for any final publishable run**
- Should we run on more than conv-26? → **no, scope this issue to conv-26 to match RUN-0009; expand in a follow-up**

## References

- ISS-084 (decision: SA path A vs B vs C — depends on this)
- ISS-083 (Hybrid downgrade — orthogonal but improves both recall@5 and J before RUN-0010 if landed first)
- RUN-0009 report: `.gid/eval-runs/RUN-0009-substrate/RUN-0009-full-conv-report.md`
- LoCoMo paper: judge prompt and J definition
