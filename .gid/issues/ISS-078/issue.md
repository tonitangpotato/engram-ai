---
title: L5 topic compiler not wired into ingestion pipeline finalize
status: open
priority: P1
filed: 2026-04-30
filed_by: rustclaw
labels:
- substrate
- knowledge-synthesis
- ingestion
- retrieval
---

# L5 topic compiler not wired into ingestion pipeline finalize

## Symptom

After full ingestion of LoCoMo conv-26 (RUN-0008 substrate), the `knowledge_topics` table contains **0 rows**. As a result:

- `Abstract` retrieval plan: `searcher.search()` returns empty → plan exits with `DowngradedL5Unavailable`, returning 0 items.
- `Hybrid` retrieval plan: its abstract sub-plan goes down the same path → contributes 0 items to RRF fusion.
- Net effect: Hybrid plan hit@5 = 0/2 in eval cat=4 (open-ended/summary) queries, half of which is attributable to this substrate gap (the other half is ISS-079).

## Evidence

- Direct table inspection during diagnostic session 2026-04-30: `SELECT COUNT(*) FROM knowledge_topics;` → 0
- RUN-0008 post-fix summary confirms Hybrid 0/2 unchanged after ISS-076 dangling-edge fix, ruling out edge corruption as the cause.
- L5 compiler code exists and runs correctly when invoked manually (verified previously); it is simply never invoked by the ingestion pipeline.

## Root cause

The ingestion pipeline's finalize stage does not call the knowledge compiler over the freshly ingested substrate. L5 topics are a derived layer that must be (re)built whenever new memories land; right now it's a manual offline step that nobody runs in the eval flow.

## Fix direction (not yet decided)

Two options, pick one:

**A. Wire compiler into ingestion finalize (preferred for correctness).**
- After ingestion commits, run knowledge compiler over the new namespace/conv.
- Pros: substrate is always coherent post-ingest, no drift between L1/L2/L5 layers.
- Cons: adds latency + memory pressure to ingestion path; need to think about partial-failure semantics (what if compiler fails after ingest commits?).

**B. Add an explicit "finalize substrate" step to eval scripts.**
- Eval `01_ingest.py` or equivalent runs `cargo run --bin knowledge-compile` after ingest.
- Pros: cheap, decouples concerns, no production-path changes.
- Cons: doesn't fix real users' ingestion flow — only papers over for benchmarks.

Default lean: **A**, because the symptom in eval is the same symptom production users would hit.

## Acceptance criteria

- After fresh ingestion of any namespace, `knowledge_topics` table is non-empty (assuming the namespace has ≥ N memories where N is whatever the compiler's minimum cluster threshold is).
- RUN-0008-style eval cat=4 (Abstract/Hybrid plan) hit@5 improves from current baseline (0/2 for Hybrid).
- No regression in ingestion latency beyond an agreed budget (TBD when picking option A vs B).

## Out of scope

- Multi-hop cat=1 0/3 — separately tracked under ISS-070 (executor has no MultiHop PlanKind) and pending diagnosis of query routing.
- Episodic plan downgrade behavior — separately tracked under ISS-079.
- Tuning of L5 compiler clustering parameters — only wiring is in scope here.

## Related

- relates_to: ISS-070 (multi-hop dispatcher)
- relates_to: ISS-079 (episodic over-downgrade)
- substrate: RUN-0008 / locomo-conv26-iss076
