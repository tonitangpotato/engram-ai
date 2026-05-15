---
id: ISS-076
title: All graph_edges endpoint UUIDs are dangling — edges have no resolvable subject or object entity
status: done
priority: P0
labels:
- resolution
- edges
- root-cause
- v0.3
- locomo
relates_to:
- ISS-075
- ISS-072
- ISS-068
---

# ISS-076: graph_edges endpoint UUIDs do not match any graph_entities row

## TL;DR

In RUN-0007 substrate (and likely all prior v0.3 substrates), every edge in `graph_edges` references `subject_id` and `object_entity_id` UUIDs that **do not exist in `graph_entities`**. 125 live edges, 88 distinct subject UUIDs, 113 distinct object UUIDs — zero of them join back to an entity row.

This is independent of (and worse than) ISS-075 (dedup failure). Even after ISS-075 is fixed, this bug means edges still can't be traversed from any entity.

## Evidence

```
Edge subject IDs found in entities table:
  distinct_subjects = 88
  subjects_in_entities_table = 0   ← all dangling

Edge object_entity IDs found in entities table:
  distinct_objects = 113
  objects_in_entities_table = 0    ← all dangling
```

Source: `.gid/eval-runs/RUN-0007-substrate/locomo-conv26-iss072.graph.db`, queried with `LEFT JOIN graph_entities ON e.subject_id = ent.id`.

Sample edge rows show predicate_label populated correctly (`leads_to`, `related_to`, `part_of`, `uses`) but BLOB UUID columns rendered as garbage when JOINed — confirming the IDs are real UUIDs, just pointing at entities that don't exist (or exist under different IDs).

## Hypothesis on root cause

The resolution pipeline has two stages that allocate entity IDs:

1. **`stage_extract`** parses LLM triples into `(subject_mention, predicate, object_mention)`. Each mention gets an internal ID (or text key) for downstream stages.
2. **`stage_persist`** writes to both `graph_entities` and `graph_edges`.

The dangling-UUIDs pattern strongly suggests **the IDs assigned to mentions during extract/resolve are not the same IDs that get inserted into `graph_entities` at persist time**, but **are** the IDs written into `graph_edges.subject_id` / `object_entity_id`. Likely candidates:

- Two parallel ID allocation paths (one for the entity row, one for the edge endpoint), drifting under some condition.
- Edge persist runs before entity persist commits, and falls back to "transient" mention IDs that never get reconciled.
- The CreateNew short-circuit (ISS-075) generates a fresh `Entity::new()` UUID at persist time, but `resolve_edges` already cached the *pre-resolution mention's* UUID — so subject_id ≠ entity.id.

The third hypothesis is the most likely given ISS-075 — when every entity is `CreateNew`, the "pre-resolution mention UUID" the edge stage saw is never the "freshly allocated entity UUID" the persist stage wrote.

## Verification (need to confirm before fixing)

- Read `pipeline.rs::resolve_edges` and `stage_persist.rs` together. Identify where edge subject/object IDs come from and where entity IDs come from.
- Print pipeline trace for a single conv-26 turn and check whether `EntityResolution::new_id` matches the `Edge::subject_id` for triples whose subject is that entity.

## Why this matters for benchmarks

- Spreading activation walks from anchor entities along edges. If `entity.id` ≠ any `edge.subject_id`, walks terminate immediately at the anchor — explaining why retrieval has been treating Caroline as a leaf node despite 27 mention copies and many "X-related-to-Caroline" triples.
- Edge-based retrieval signals (predicate frequency, relation-typed traversal) are unusable until this is fixed.
- Hebbian co-activation links between entities can't be derived from edges either.

## Acceptance criteria

On a fresh ingest of LoCoMo conv-26:

- [AC-1] `SELECT COUNT(*) FROM graph_edges e WHERE NOT EXISTS (SELECT 1 FROM graph_entities ent WHERE ent.id = e.subject_id) AND e.invalidated_at IS NULL` = 0 (no dangling subjects)
- [AC-2] Same query for `object_entity_id` (where `object_kind = 'entity'`) = 0 (no dangling objects)
- [AC-3] At least one Caroline entity has `>0` outgoing edges (after ISS-075 dedup fix; before that, at least one *of the 27* should).

## Out of scope

- ISS-075 (pipeline never writes alias/embedding) — separate root cause; both must be fixed.
- ISS-074 (entity enrichment fields default) — orthogonal.
- Spreading activation algorithm itself — runs fine *if* the graph is consistent; this issue is about graph consistency.

## Verification command (current state)

```bash
sqlite3 .gid/eval-runs/RUN-0007-substrate/locomo-conv26-iss072.graph.db \
  "SELECT
     (SELECT COUNT(*) FROM graph_edges e LEFT JOIN graph_entities ent ON e.subject_id=ent.id 
        WHERE ent.id IS NULL AND e.invalidated_at IS NULL) AS dangling_subjects,
     (SELECT COUNT(*) FROM graph_edges e LEFT JOIN graph_entities ent ON e.object_entity_id=ent.id 
        WHERE ent.id IS NULL AND e.invalidated_at IS NULL AND e.object_kind='entity') AS dangling_objects;"
# Expected (current): 125 | 113
# Expected (fixed):   0   | 0
```

---

## Phase A retrieval impact (RUN-0008, 2026-04-30)

After applying the dangling-UUID fix (commit `f95480b`) and re-ingesting LoCoMo conv-26, ran the same 25-query retrieval suite as RUN-0007 baseline. Compared head-to-head:

**Plumbing (AC-1/2/3): all green**
- Dangling subject edges: 125 → **0**
- Dangling object edges: 113 → **0**
- Caroline entity copies: 27 → **1** (deduped via canonical alias)
- Caroline outgoing edges: 0 (on any single copy) → **>0** (on the deduped entity)

**Retrieval hit@5: essentially flat**

| Metric | RUN-0007 baseline | RUN-0008 post-fix | Δ |
|---|---|---|---|
| Total hit@5 | 12/25 (48.0%) | 13/25 (52.0%) | +1 |
| Headline hit@5 (cat 1–4) | 10/20 (50.0%) | 10/20 (50.0%) | 0 |
| Empty result sets | 2 | 2 | 0 |

The single +1 hit moved on Q22 (cat=5 Adversarial / Abstract plan, downgraded) — a boundary case, not a structural improvement. Per-category and per-plan breakdowns are otherwise identical to baseline.

**What this falsifies**

The hypothesis that *entity dedup is a retrieval bottleneck* on this dataset. Headline hit@5 is dominated by the Factual plan (17/25 queries), which retrieves chunks via text/embedding similarity directly — entity-level edges don't sit on that path. Multiplying Caroline 27× was a real defect (and a genuine substrate bug), but it wasn't the thing holding hit@k back.

**What's still broken (separate root causes, not blocked on ISS-076)**

- **Hybrid plan: 0/2, both `empty_result_set`** (`DowngradedFromEpisodic` → `DowngradedL5Unavailable`). Plan dispatch falls off a cliff before any retrieval substrate is consulted.
- **Multi-hop (cat=1): 0/3.** No graph traversal ever fires; queries are answered by Factual plan with chunk similarity only.
- **Affective: 0/2, no_cognitive_state.** Substrate not wired.

These are the actual hit@k levers. Tracked separately:

- ISS-075 Phase B (sync embedding wiring on entities) — necessary but probably *not* sufficient on its own; will help Hybrid only after Hybrid stops downgrading.
- Hybrid/Multi-hop plan executor wiring — likely the bigger lever; needs separate diagnostic before scoping.

**Conclusion**

ISS-076 is **substantively complete** — the bug it described is fixed and verified end-to-end. Its retrieval impact is null/marginal on this benchmark, which is a useful negative result: it tells us where the next lever is *not*.

Closing with status = fixed-validated; downstream work tracked under ISS-075 Phase B and a (yet-to-be-filed) Hybrid-plan-downgrade investigation.

**Artifacts**
- Substrate: `.gid/eval-runs/RUN-0008-substrate/locomo-conv26-iss076.{db,graph.db}`
- Ingestion log: `.gid/eval-runs/RUN-0008-substrate/ingest.log`
- Retrieval log: `.gid/eval-runs/RUN-0008-substrate/RUN-0008-post-fix.log`

---

## Status-drift note (2026-05-15, post-v0.4 substrate migration)

Attempted to verify the "fixed-validated" claim before flipping `status: open → done`. Re-ran the original verification queries on `.gid/eval-runs/RUN-0008-substrate/locomo-conv26-iss076.db`:

```
dangling_subjects = 0   (matches "Expected (fixed)")
dangling_objects  = 0   (matches "Expected (fixed)")
graph_entities total rows = 1   (NOT 'Caroline 1', it's an unrelated 'go' entity)
graph_entities WHERE canonical_name='Caroline' = 0
graph_entity_aliases total = 0
graph_edges total = 0
```

The "0 dangling" numbers are now **vacuously true** because the substrate's `graph_edges` table is empty (0/0 = 0% dangling). The Caroline → 1 result from the original Phase A report cannot be reproduced from this substrate — schema has drifted under v0.4 work or the data was rebuilt.

**Implication**: Status flip needs a fresh post-fix substrate to re-verify ACs. Filing this note so we don't accidentally mark `done` on stale evidence. The fix commit `f95480b` is still in `git log`; what we lack is a current substrate to re-verify on. Pair with ISS-075 (same situation).

**Recommendation when potato is back**: Either (a) accept `f95480b` + code review as sufficient and close, or (b) trigger a fresh ingest to regenerate AC verification.

---

## Resolution (2026-05-15)

**Closed via code-review-close (Option a).** Ratified with potato in current session.

**Rationale:**
- Fix commit `f95480b` is in `git log` and was verified end-to-end against `RUN-0008-substrate` immediately after landing (see ISS-076 Phase A retrieval impact section — `Caroline 27 → 1`, alias rows >0, dangling endpoints 0/0).
- The original ACs target a v0.3 substrate shape (`graph_entities` / `graph_entity_aliases`). The substrate has since drifted to v0.4 (unified nodes/edges substrate, Phase B/C/D work in `crates/engramai/src/substrate/`), making the original verification SQL vacuously-true rather than meaningfully-true.
- A fresh ingest just to re-verify pre-drift ACs would cost LoCoMo API spend with no engineering signal — the fix is already in code and the relevant retrieval-quality follow-ups are tracked in separate live issues (ISS-075 Phase B was Hybrid-plan downgrade investigation; that has since been superseded by v0.4 retrieval adapter work).

**Evidence trail (kept in this file):**
- Phase A retrieval impact section above documents the end-to-end verification on `RUN-0008` (the only substrate that ever existed with the fix applied and v0.3 schema intact).
- Status-drift note documents why the original SQL ACs can't be re-verified on current substrate.
- `git log f95480b` is the canonical fix evidence.

**Acceptance criteria:** closed via code review at `f95480b` + Phase A retrieval-impact verification on `RUN-0008`. Vacuous re-verification on drifted v0.4 substrate explicitly skipped per Option (a).
