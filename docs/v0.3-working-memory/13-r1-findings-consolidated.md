# r1 Findings — Consolidated Resolution Plan

**Date**: 2026-04-24 (overnight autonomous pass)
**Source review**: `docs/reviews/DESIGN-v0.3-r1.md`
**Scope**: All findings A1–D6 analyzed; per-finding detail docs in `docs/working-memory/06–12`

**Read this first.** Per-finding docs dive deep; this doc lists the actionable decisions.

---

## TL;DR

- **3 findings resolved** on inspection (no decision needed): C3, C4, D5
- **11 findings require potato decision** before DESIGN rewrite starts
- **1 new finding surfaced by analysis** (not in r1): interoceptive signal taxonomy mixes resources + affect
- **Net roadmap impact**: +2.5 weeks (14 → 16.5 weeks) — honest Phase 1 carve-up compensates for gained realism; A4/A5 deletion *saves* 3–4 days

---

## Decisions Needed (in priority order)

### Block 1 — Architectural (must resolve before any DESIGN rewrite)

**D0. A1 working memory naming** — rename `SessionWorkingMemory` → `ActiveContext` (or `ConversationBuffer`)?
- Doc 06 proved true cost <1 hour (not r1's ~20 call sites — it was 5 real usages, rest were tree-sitter false positives at confidence 0.6)
- **Not yet executed** — doc 06 made the recommendation, rename itself pending
- My recommendation: **ActiveContext**. Cleaner; avoids the "working memory" overload that conflicts with v0.3's L2/L3 cognitive working memory
- Detail: `06-session-wm-rename-blast-radius.md`

**D1. A2 predicate schema** — accept hybrid `Seeded | Proposed(String)` + schema_inducer?
- My recommendation: **yes**, matches §10 Q2's "seeded-with-override" stance already held by author
- Open: schema_inducer in Phase 2 (partial) or Phase 4 (full)?
- Promotion policy Proposed → Seeded: human-reviewed early, automatic later?
- Detail: `08-confidence-a3-analysis.md` — wait, A2 is doc 07

**D2. A3 confidence field** — Alternative 1 (don't store) vs Alternative 3 (split reliability/salience)?
- My recommendation: **Alt 1 for v0.3.0** (simplest, no schema change, preserves relational semantics)
- Upgrade to Alt 3 only if a concrete SQL-filter use case appears
- Any planned v0.3 feature needs `WHERE confidence > 0.X` in SQL?
- Detail: `08-confidence-a3-analysis.md`

**D3. A4 interoceptive gating** — full r1 ("zero role on write path") vs my refined ("reactive error handling, not proactive gating")?
- My recommendation: **refined version** — keep reactive handling when LLM calls fail, delete proactive signal-based gating
- Delete §4.5 backlog mechanism entirely (A5, agreed with r1)
- Detail: `09-interoceptive-gating-a4-a5-analysis.md`

### Block 2 — Scope & timeline

**D4. B1 Phase 1 carve-up** — accept split into Phase 1a (graph tables, 2.5 wk) + Phase 1b (MemoryRecord ext, 3 wk)?
- My recommendation: **yes**, honest blast radius visibility
- Total v0.3 becomes ~16.5 weeks
- Detail: `10-scope-timeline-b1-b2-b3-analysis.md`

**D5. B3 Phase 0 deliverable checklist** — block Phase 1 start until all 7 architectural decisions closed?
- My recommendation: **yes**, prevents re-litigating mid-implementation
- Detail: same doc

### Block 3 — Spec additions (Phase 0 work, mostly mechanical once approved)

**D6. C1 Entity state rules** — approve proposed update rules (α=0.2 EMA, ACT-R shared curve, no-retro-remove)?
- Detail: `11-spec-gaps-class-c-analysis.md`

**D7. C2 fusion weights** — approve `w_embed=0.5, w_alias=0.3, w_context=0.15, w_temporal=0.05, threshold=0.72`?
- Plus tie-breaker: prefer recent `last_seen`, then higher `importance`
- Detail: same doc

**D8. C5 query heuristic rules** — approve the 5-rule regex/keyword classifier with 0.7 confidence threshold?
- Detail: same doc

### Block 4 — API & migration

**D9. D3 Public API backward compat** — Option A (keep v0.2 `recall` sync, add new `recall_v3` async) vs Option B (break compat)?
- My recommendation: **Option A**. NG1 honest, no ecosystem breakage
- Detail: `12-nits-class-d-analysis.md`

**D10. D4 migration rollback story** — `*.pre-v03.bak` auto-backup + `--accept-forward-only` confirm flag acceptable?
- Detail: same doc

**D11. D6 `explain_recall`** — ship in v0.3.0 or defer to v0.3.1?
- My recommendation: **v0.3.0**. Debuggability is essential with 5-layer architecture
- Detail: same doc

### Block 5 — New finding from analysis

**D12. (Not in r1) Interoceptive signal taxonomy** — file as ISS-030 for future split of `SignalSource` into `AffectSource` + `ResourceSource`?
- Not v0.3 blocker, but latent design smell surfaced by §4.5 gating discussion
- Detail: `09-interoceptive-gating-a4-a5-analysis.md` end section

---

## Findings Resolved Without Decision

| Finding | Status | Resolution |
|---|---|---|
| C3 | ✅ Resolved | §3.4 already has bi-temporal (valid_at + invalid_at + asserted_at). r1 flagged "verify" — verified. 3-line clarifying comment recommended. |
| C4 | ✅ Resolved | Subsumed by A4/A5 — gating being deleted, so gating-rule spec is moot. |
| D5 | ✅ Resolved | Subsumed by A2 — hybrid schema removes `RelatedTo` fallback entirely, no landmine to tune around. |

---

## Findings with Easy Rephrase (no decision, just edits)

| Finding | Action |
|---|---|
| B2 | Rephrase NG5: "not replacing; v0.3 extends with provenance + reliability, episodic-trace role preserved" |
| D1 | Rephrase §0: "none combine decay + affect + consolidation + interoception at engram's depth" |
| D2 | Tighten §11: counter in write_stats.rs, N=500 episodes benchmark |

---

## Effort Summary

### Phase 0 (decisions + doc writing, post-approval)
- ~1 day of concentrated potato-decision time for Blocks 1–4
- ~6–8 hours of DESIGN rewrite work (batched across all findings)
- ~1 week total if decisions happen sync, ~2 weeks if async

### Roadmap delta
- +2.5 weeks from B1 carve-up honesty (Phase 1a + 1b)
- -3 to -4 days from A4/A5 deletion (no backlog mechanism)
- ~0 net for everything else
- **New total: ~16.5 weeks** vs original 14

### Post-Phase-0 implementation additions
- Phase 2: +1 hour (LLM counter per D2)
- Phase 3: +5 hours (dual-API per D3 + explain_recall per D6)
- Phase 5: +2 hours (migration backup + warning per D4)

---

## Process Notes

**What went well**:
- Systematic A→B→C→D traversal with per-finding verification
- Surfaced DESIGN-internal contradictions r1 didn't catch (§3.5 vs §10 Q2 on emergent schema)
- Caught factual error in r1's evidence (§3.2 `// was computed` — v0.2 never had that field)
- Caught r1's unverifiable "14/422 nodes" numbers — substituted grep-based ground truth (28/25 files)
- Found new issue r1 missed (signal taxonomy conflation)

**What to double-check in the morning**:
- A3 — is my "Alt 1 default" take too conservative? If potato wants SQL-queryable confidence for a specific feature I don't know about, Alt 3 is the right answer.
- A4 — is my refined "keep error handling" too soft relative to r1's absolute "zero role"? Both positions are defensible.
- D3 — Option A (dual API) vs Option B (break compat) is partly a philosophical call about v0.2.2's user base. If rustclaw is the only real consumer, breaking compat might be fine.

**Double-write backup**:
- All 6 analysis docs in `docs/working-memory/07–12` (details)
- This summary in `docs/working-memory/13-r1-findings-consolidated.md`
- Engram store for semantic recall (TBD — will store a compact summary)
- Daily log entry TBD

---

## Next Actions for Morning potato

1. Read this doc top-to-bottom (~5 min)
2. Read any per-finding doc you want to dive into (07–12)
3. Make calls on D1–D11 (D12 is "file a future issue or drop it")
4. Once decisions are made, I (or a sub-agent) can batch-apply all DESIGN edits in one pass
5. After DESIGN is coherent, run `draft-requirements` skill to split into feature docs

**Estimated time for your part**: 30–45 min reading + decisions if you go fast, 1–2 hours if you want to debate each one.

---

## Documents in this series

- `06-session-wm-rename-blast-radius.md` — A1 (earlier, working memory rename, done)
- `07-predicate-schema-a2-analysis.md` — A2 predicates
- `08-confidence-a3-analysis.md` — A3 MemoryRecord.confidence
- `09-interoceptive-gating-a4-a5-analysis.md` — A4 + A5 gating + backlog
- `10-scope-timeline-b1-b2-b3-analysis.md` — B-series scope/timeline
- `11-spec-gaps-class-c-analysis.md` — C-series spec gaps
- `12-nits-class-d-analysis.md` — D-series polish
- `13-r1-findings-consolidated.md` — this doc

---

## Tier 3 Batch Applied — 2026-04-24

Applied all "execution details with ready fixes" (Tier 3) to `docs/DESIGN-v0.3.md`:

| ID | Section | Change |
|---|---|---|
| B2 | §1 NG5 | Reworded: "not replacing; v0.3 extends with provenance + reliability, episodic-trace role preserved" |
| D1 | §0 | Weakened claim: "none combine decay + affect + consolidation + interoception at engram's depth" (acknowledges mem0/Letta/A-MEM have partial cognitive elements) |
| D2 | §1 G3 + §11 | Tightened measurement: `write_stats.rs` per-stage counter, N = 500 benchmark (LOCOMO + rustclaw trace) |
| D4 | §8.1 | Added rollback story: `{db_path}.pre-v03.bak` auto-backup + `--accept-forward-only` + interactive confirm + `--no-backup` escape hatch + `migrate --status` |
| D6 | §7.1 | Added `explain_recall(query) -> RecallTrace` public API. Marked opt-in (standard `recall()` pays no trace cost). |
| SR1 (self-review) | §3.5 | Added extractor guidance: prefer `Proposed(raw_string)` over shoehorning into `RelatesTo`; no-edge beats mis-categorized edge |
| SR2 (self-review) | §4.5 | Clarified `extraction_error` metadata lives on `Episode.metadata` (§3.1 `serde_json::Value`) |

**Result**: DESIGN-v0.3.md 761 → 777 lines. All cross-references verified. `cargo test -p engramai` passes (1176/1176).
