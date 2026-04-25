# A2 Analysis: Predicate Schema — DESIGN §3.5 vs Implementation

**Context**: Working through DESIGN-v0.3-r1.md findings, starting with A2 (Predicate schema drift).
**Date**: 2026-04-24
**Status**: Analysis complete — recommendation ready for decision

---

## r1's Claim

DESIGN §3.5 says predicates are "emergent, not predeclared":
- Free-form strings at write time
- Background `schema_inducer` job clusters and canonicalizes
- Positioned as a key Graphiti differentiator

Code implements the opposite:
- Closed 9-variant enum in `Predicate`
- `from_str_lossy` falls back to `RelatedTo` for unknowns
- Extraction prompt hard-constrains LLM to the 9 allowed values
- No `schema_inducer` module exists

r1 recommended: hybrid `Predicate::Seeded(SeededPred) | Proposed(String)` + real schema_inducer, added ~1 week to Phase 2 or 4.

---

## Evidence Verification

**All of r1's evidence is accurate.** Verified:

- `crates/engramai/src/triple.rs` — closed enum with 9 variants (IsA, PartOf, Uses, DependsOn, CausedBy, LeadsTo, Implements, Contradicts, RelatedTo). `from_str_lossy` returns `RelatedTo` for anything unknown.
- `crates/engramai/src/triple_extractor.rs` (lines 18–40) — prompt literally says:
  > *"Allowed predicates: is_a, part_of, uses, depends_on, caused_by, leads_to, implements, contradicts, related_to"*
  Not few-shot bias — **explicit LLM-level constraint**.
- Zero `schema_inducer` / `SchemaInducer` references anywhere in the codebase (grep turned up one unrelated comment).

---

## What r1 Missed: DESIGN Is Internally Contradictory

r1 framed this as "DESIGN vs code." The deeper finding:

**§3.5 and §10 Q2 contradict each other inside the same DESIGN doc.**

- **§3.5**: Markets emergent schema as a differentiator.
  > *"Schema — emergent, not predeclared. Graphiti forces you to predeclare edge_type_map. Engram doesn't."*
- **§10 Q2** (open questions): Author admits doubt.
  > *"Predicate schema — fully emergent, or seeded? Fully emergent is clean but cold-starts badly... **I lean seeded-with-override**; happy to go fully emergent if you prefer."*

The code reflects §10 Q2's instinct, not §3.5's marketing — but takes it **further** (closed rather than seeded-with-override).

**Implication**: r1's hybrid recommendation isn't pushing the author into new territory. It's **the exact design §10 Q2 already leaned toward**. Low resistance expected.

---

## Effort Estimate — Calibrated

r1 said "~1 week to Phase 2 or 4." Code footprint suggests it's closer to **1.5–2 weeks**, split as:

**Files touched:**

| File | Predicate:: refs | Type of change |
|---|---|---|
| `crates/engramai/src/triple.rs` | 38 | Enum refactor + `from_str_lossy` new logic |
| `crates/engramai/src/triple_extractor.rs` | 4 | Prompt rewrite + allow "OTHER: xxx" parse path |
| `crates/engramai/src/storage.rs` | 1 | Serialize new variant (trivial) |
| `crates/engramai/tests/triple_integration.rs` | 31 | Existing tests mostly survive; add Proposed(String) coverage |

**Breakdown:**

- **Enum + prompt + parse** (hybrid variant): **2–3 days** — most of the 38 refs in `triple.rs` are enum definition/Display/FromStr, not deep logic.
- **`schema_inducer` module** (clustering embeddings → LLM naming → canonicalization rewrite pass): **~1 week**. This is genuinely new code. Complexity depends on clustering strategy (HDBSCAN vs simpler cosine threshold) and whether it runs online or as a batch job.
- **Edge rewrite migration** (rename Proposed("causes") → Seeded(CausedBy) when cluster maps): **2–3 days** — transactional, idempotent, needs care but not conceptually hard.

**Total: ~1.5–2 weeks**, not 1.

---

## Why Hybrid Is Correct (Not Full Emergent)

Reasons to NOT go fully emergent `Predicate(String)`:

1. **Cold-start problem** is real — fully emergent starts with chaos; 9 seeded predicates cover 80%+ of common cases.
2. **Downstream code** benefits from stable handles — `CausedBy` and `LeadsTo` have symmetric graph semantics that emergent strings can't guarantee without re-canonicalizing on every query.
3. **Honest marketing** — "seeded-with-emergent-override" differentiates from Graphiti *and* is defensible. "Fully emergent" would require the schema_inducer to be production-grade before v0.3 ships.

Reasons to NOT stay with closed enum (current code):

1. Kills the §3.5 differentiator entirely.
2. `RelatedTo` fallback silently discards real signal — "A **causes** B" and "A **is next to** B" both collapse to RelatedTo.
3. Prompt-level hard constraint means the LLM's knowledge of richer predicates (precedes, enables, prevents, ...) is actively suppressed.

Hybrid threads the needle.

---

## Recommendation

### Decision

**Accept r1's hybrid** (`Seeded | Proposed(String)` + schema_inducer).

### DESIGN changes required

1. **Rewrite §3.5**: change framing from "emergent, not predeclared" to **"seeded with emergent override"**. Describe:
   - 9 seeded predicates (bootstrap, covers common cases, stable handles for downstream symmetric-relation logic)
   - LLM may propose new predicates as `Proposed(String)` when no seeded variant fits
   - Background `schema_inducer` clusters proposals, names stable ones, rewrites edges when cluster → canonical mapping emerges
2. **Close §10 Q2**: move from "open question" to "decided: seeded-with-override." Keep the rationale visible.
3. **§9 roadmap**: add schema_inducer as a Phase 2 or Phase 4 work item, **~1.5–2 weeks**. Recommend Phase 4 — it's a quality enhancement, not a v0.3 MVP blocker (seeded-only still works).

### Code changes (when implementation starts — not now)

- `triple.rs`: `Predicate::Seeded(SeededPred)` + `Predicate::Proposed(String)`. `from_str_lossy` tries seeded first, falls through to `Proposed`.
- `triple_extractor.rs`: prompt becomes "Prefer these 9 predicates; if none fit, propose a concise lowercase verb phrase" + parse loosens.
- New module `schema_inducer.rs`: batch job, embeddings on Proposed strings, cluster, LLM-name top clusters, rewrite pass.
- Migration SQL: none initially (variant is additive); later migration only if promoting Proposed → new Seeded variant.

---

## Open Sub-Questions (for potato)

- **Phase 2 or Phase 4?** Phase 4 preferred (enhancement, not blocker). Your call.
- **Promotion policy** for Proposed → new Seeded variant: automatic (if cluster size > N) or requires human review? I'd lean human review early, automatic later once schema_inducer is trusted.
- **Display for Proposed**: render as-is, or prefix (e.g., `~causes` to signal "emergent, not canonical")? Affects UI clarity.

---

## Status

- [x] Evidence verified — r1 accurate
- [x] DESIGN internal contradiction surfaced (§3.5 vs §10 Q2)
- [x] Effort re-estimated (1.5–2 weeks, not 1)
- [x] Recommendation formed
- [ ] potato decision on Phase 2 vs Phase 4
- [ ] potato decision on promotion policy + Display format
- [ ] DESIGN §3.5 rewrite (separate pass, after all r1 findings decided)
- [ ] §10 Q2 closure (same pass)

Ready to move to **A3 (confidence-as-stored vs meta-cognitive)** when you say go.
