---
id: "ISS-031"
title: "Schema Inducer — Canonicalize Proposed Predicate Variants"
status: open
priority: P2
created: 2026-04-24
severity: medium
---
# ISS-031: Schema Inducer — Canonicalize Proposed Predicate Variants

**Status:** open (deferred to v0.4)
**Severity:** medium — data-layer debt, bounded and non-compounding
**Milestone:** v0.4
**Related:**
- v0.3 DESIGN review r1 finding A2 (predicate schema)
- `docs/v0.3-working-memory/07-predicate-schema-a2-analysis.md`
- DESIGN-v0.3.md §3.5 (hybrid schema — Seeded enum + Proposed(String))
**Filed:** 2026-04-24

## TL;DR

v0.3 ships **hybrid predicate schema**: 9 seeded enum variants (canonical) + `Proposed(String)` fallback for novel predicates discovered during extraction. This provides structure without blocking unknown relations.

**Deferred to v0.4**: the **schema inducer** that clusters Proposed variants and promotes high-confidence clusters into new canonical enum variants.

## Why Defer (Data-Driven Judgment, Not Scope Cut)

Schema inducer requires tuning multiple thresholds:
- Clustering similarity threshold (when are two Proposed strings "the same"?)
- Promotion confidence threshold (how many occurrences justify enum promotion?)
- Embedding model choice for predicate phrases
- Review/approval workflow for auto-promoted variants

**These thresholds cannot be tuned without real corpus.** Building the inducer pre-v0.3 means picking numbers out of the air; they'll almost certainly be wrong and require rework once v0.3 is in production.

**The right sequence**: ship v0.3 → collect real Proposed-string distribution over weeks/months → design inducer against that data → ship in v0.4.

## Is This Technical Debt?

Yes, but **bounded data-layer debt**, not structural debt. Key properties:

**Debt does accumulate**:
- Proposed strings accumulate duplicates ("precedes" / "precede" / "preceded by" / "happens before" all same semantics)
- No canonicalization → string-level exact match queries may under-recall

**Debt does NOT compound** (this is the critical property):
- Proposed variants are **lossy-preserved raw signal only** — no decision path consumes them as if they were Seeded
- Graph symmetric queries (e.g., `CausedBy` ↔ `cause-of`) operate on Seeded variants only; Proposed doesn't participate
- No downstream code gets written against specific Proposed strings (verified by architectural invariant in DESIGN §3.5)
- v0.4 inducer can do a **one-time rewrite migration** to canonicalize the accumulated strings

**Analogy**: ISS-030 (signal taxonomy) is foundation-crack debt — gets worse as more code is written assuming broken structure. ISS-031 (schema inducer) is storage-room-clutter debt — stuff piles up, doesn't rot, cleaned in one pass later.

## v0.3 Protections to Ensure Debt Stays Bounded

DESIGN-v0.3.md §3.5 must enforce:
1. **Proposed variants cannot participate in graph symmetric/inverse query logic** — only Seeded do
2. **No downstream code reads Proposed strings for decisions** — they exist for preservation + future canonicalization only
3. **Proposed is explicitly documented as "canonicalization pending, v0.4"** — so no one writes code that assumes these strings are stable

## v0.4 Inducer Scope

1. Periodic clustering of Proposed strings using embeddings + string similarity
2. Occurrence-count thresholds for promotion candidate identification
3. Human review UI / CLI for promotion approval (not auto-promotion initially)
4. One-time migration tool to rewrite approved clusters into new Seeded variants
5. Schema evolution mechanism (add enum variants without breaking existing graph data)

## Out of Scope for This Ticket

- Final inducer design (this ticket is the starter; real proposal in v0.4)
- Picking thresholds (must be data-driven)
- Auto-promotion policy (probably human-in-loop for v0.4, auto later if data supports)

## Success Criteria

v0.4 inducer considered complete when:
- Running against real v0.3 production data produces reasonable clusters
- Promotion workflow reviewable by human with clear provenance
- Migration tool safely rewrites accumulated Proposed strings
- No data loss; full rollback capability
