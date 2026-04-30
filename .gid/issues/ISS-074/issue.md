---
id: ISS-074
title: "v0.3 extractor doesn't emit summary / importance / attributes — entity enrichment fields stuck at defaults"
status: open
priority: P2
filed: 2026-04-30
filed_by: rustclaw
labels: [graph, ingestion, v0.3, extractor, enrichment]
relates_to: [ISS-072, ISS-070]
---

# v0.3 extractor doesn't emit summary / importance / attributes

## Summary

Following ISS-072 A-clean, `graph_entities.kind` is now correctly populated end-to-end (RUN-0007: real-kind coverage 85%, `unknown` ratio 14.97%). However, three sibling columns remain at defaults for 100% of rows on a fresh LoCoMo conv-26 ingest:

- `summary` — empty string for 187/187 rows
- `importance` — `0.0` for 187/187 rows
- `attributes` — populated but content is **provenance-only** (`{"kind_source":"TripleHint"|"Default"}`); no real entity attributes (e.g. for a `person`: pronouns, role, relation; for an `event`: when/where)

This is **not** a plumbing regression — it was never on the wire to begin with.

## Root cause

The LLM extractor in `crates/engramai/src/triple_extractor.rs` and the corresponding wire format in `crates/engramai/src/triple.rs` only define `subject / predicate / object / subject_kind_hint / object_kind_hint`. There is no schema for `summary`, `importance`, or per-kind structured `attributes`.

```
$ grep -nE "summary|importance" crates/engramai/src/triple_extractor.rs
(no matches)
```

ISS-072's "Out of scope for this PR (deferred to B / GOAL-2.b)" note already foresaw this; that deferred work is what this issue tracks.

## Evidence

- RUN-0007 verification: `.gid/eval-runs/RUN-0007.md` — Findings F2 and F3
- ISS-072 issue body — "Out of scope" section + "RUN-0007 verification" appendix

## Scope

This is a small surgical change in three layers:

1. **Extractor prompt** (`triple_extractor.rs`): extend the JSON schema sent to the LLM to include optional `subject_summary`, `object_summary`, `importance` (per-triple), and per-kind structured `attributes` (e.g. for `person`: pronouns, role; for `event`: when/where). Make all fields optional with `#[serde(default)]` so existing tests keep passing.
2. **Wire type** (`triple.rs`): mirror the extractor schema additions on `Triple` / endpoint structs.
3. **Plumbing** (`resolution/adapters.rs`, `resolution/stage_persist.rs`): copy the new fields into `Entity.summary` / `Entity.importance` / `Entity.attributes` (merge with the existing `kind_source` provenance breadcrumb — don't overwrite it).

## Acceptance

On a fresh LoCoMo conv-26 ingest (same corpus as RUN-0007):

- `summary` populated (non-empty) for ≥ 60% of `graph_entities` rows
- `importance` > 0 for ≥ 60% of rows
- `attributes` carries at least one non-`kind_source` key for ≥ 40% of rows (lower bar because not every entity has structured attrs)
- Existing GOAL-2 (`kind=other("unknown") ratio ≤ 30%`) still passes — no regression on what A-clean fixed

## Out of scope

- Backfill of pre-existing entities (RUN-0005/0006/0007) — they stay as-is unless a separate backfill pass is run
- Deciding on a controlled vocabulary for per-kind `attributes` schemas — start permissive (free-form JSON object), tighten later if needed

## Notes

- Importance semantics need a small spec: is it per-triple (LLM scores each fact) or per-entity (averaged across mentions)? Recommendation: per-triple, then `stage_persist` averages across all mentions of an entity within the resolution batch.
- This issue sits naturally in the "GOAL-2.b enrichment" branch ISS-072 reserved.

---

## Scoping notes (2026-04-30 04:55 -04:00)

After reading the code surface, this is **less trivial than the original issue body suggests**. Recording open questions here so we don't sleepwalk into a design that ships noisy data.

### Why "just add fields to the prompt" is wrong

`importance` and `summary` are **already consumed** by downstream systems:

- `confidence.rs:85` — `reliability = reliability * 0.95 + record.importance * 0.05` (exponential moving average)
- `promotion.rs:175` — cluster member scoring divides by `importance` sum
- `knowledge_compile/candidates.rs:67` — filters compile candidates by `importance >= compile_min_importance`
- `knowledge_compile/synthesis.rs:165, 233, 240` — entity `summary` becomes the embedded text for topic synthesis
- `memory.rs` (multiple sites) — importance feeds cluster averages, retrieval ranking, capping

If the LLM emits garbage importance values, every one of these gets noisier. **This is a contract change, not a feature add.**

### Open design questions (need answers before implementation)

**Q1. Where do `summary` and `importance` belong on the wire?**
A `Triple` is *predicate-level* (one subject + one object + one relation). But `summary`/`importance` are *entity-level*. If we put `subject_summary` and `object_summary` on every triple:
- Same entity appearing in 5 triples → 5 different summaries. Which wins?
- Wasted tokens on the LLM side.
- Conflict resolution becomes a `stage_persist` problem (currently it has no `summary` merge logic).

**Two architectures to choose between:**

- **(α) Side-band entity list.** LLM returns `{ "entities": [{name, kind, summary, importance, attributes}], "triples": [...] }`. One canonical record per entity per batch. Clean. But: bigger prompt change, and the parser/wire-format work is real.
- **(β) Two-pass extraction.** Triple extraction stays as-is; a second LLM pass enriches entities (input: list of canonical names from pass 1, output: `{name → {summary, importance, attributes}}`). Costs 2× LLM calls per memory ingest. Cleaner separation of concerns.

The original ISS-074 body assumes per-triple fields (`subject_summary`, `object_summary`) — that's the **third** option (γ) and IMO the worst, for the conflict-resolution reason above.

**Recommendation:** **(α) side-band entity list**. Minimal cost increase, single-pass, and matches how the `Entity` table actually wants to be written.

**Q2. What does `importance` mean?**
Two valid semantics:
- **Salience within source memory** — "Rust" is the topic of this paragraph, so importance=0.9. Stable across re-extractions of the same text.
- **Global importance** — "Rust" is generally important, so importance=0.9 always. Unstable, depends on training data.

Downstream code assumes the first (it's compared to a per-entity threshold and EMA'd over time). The LLM prompt needs to **explicitly say** which semantic, with an anchored scale (0=incidental mention, 0.5=relevant, 1.0=primary subject), or we get drift between memories.

**Q3. `attributes` schema — bounded vs freeform?**
Issue body says "free-form JSON object, tighten later". I agree with the start, but:
- Need a `kind_source` collision rule (don't let LLM-emitted `kind_source` overwrite our provenance breadcrumb).
- Need a size cap (LLM could emit a 10kb biography; downstream consumers expect a small map).
- Probably: reserve a `_provenance` namespace for system-set keys (`kind_source`, future audit fields), accept anything else from LLM under a top-level merge.

**Q4. Do we actually need a design doc?**
Given Q1–Q3 each have a real choice with downstream consequences, **yes**. Suggest:
- Quick design doc at `.gid/issues/ISS-074/design.md` (1–2 pages, not the full feature treatment)
- Decisions: A1=α, A2=salience-within-source-with-anchored-scale, A3=freeform with `_provenance` namespace + size cap
- Then implementation is mechanical and the regression test (mirror of `iss072_kind_plumbing_regression.rs`) is straightforward

### Suggested next action

Write the design doc (~30 min of work), then either:
- Implement directly if straightforward, or
- Spawn a coder for the (α) implementation with the design doc + the existing ISS-072 plumbing as reference

**Priority check:** issue is filed P2. The downstream impact (degraded `knowledge_compile` quality, noisier confidence/reliability) suggests P1 is more honest. Recommend bumping to P1 if we agree this matters for v0.3 quality.
