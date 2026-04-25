# B1 + B2 + B3 Analysis: Scope & Timeline Credibility

**Context**: r1 Class B findings on §9 roadmap realism and §1 NG5 contradiction.
**Date**: 2026-04-24
**Status**: Analysis complete

---

## B1 — Phase 1 (3 weeks) underestimates blast radius

### r1's Claim
- `MemoryRecord` touches 14 nodes in the code graph
- `storage.rs` touches 422 nodes
- 4 new fields × (14 + 422) ≈ 1744 touchpoints
- Realistic Phase 1 estimate: 5–7 weeks, not 3

### Evidence Verification

r1's specific numbers (14 / 422) **cannot be verified** — engram/.gid/graph.db has only 25 issue-level nodes, no code graph. Those figures likely came from a cached memory of an older extraction (r1 says "recalled from memory 04-24 02:23"). Treat the exact numbers with skepticism.

**Ground truth from grep:**

| Symbol | Files | References |
|---|---|---|
| `MemoryRecord` | 28 files | 188 references |
| `storage::` imports | 25 files | 37 imports |
| Total `.rs` in engramai | 101 files | — |

So roughly **25–28% of the crate touches either MemoryRecord or storage**. The exact "1744 touchpoints" math is aspirational, but the qualitative finding holds: **these are the two highest-blast-radius symbols in the crate.**

### ISS-024 Calibration

r1 compares Phase 1 scope to ISS-024 (dimensional read-path — single field through read pipeline). Verified:

- ISS-024 issue directory contains design.md + investigation.md + verification-report.md + **3 review rounds** (r1 258 lines, r2 184 lines, r3 67 lines)
- Total reviews alone: ~509 lines across 3 rounds
- Single field, single pipeline, thorough quality bar

**Phase 1 scope is demonstrably larger than ISS-024**:
- 4 new fields on MemoryRecord (ISS-024 had 1)
- 2 new tables (Entity, Edge) with their own schemas, CRUD, migration — ISS-024 had 0 new tables
- New migration scaffolding (forward + rollback paths) — ISS-024 had none
- Bi-temporal invalidation semantics — new conceptual territory

If ISS-024 (1 field) took ~3 weeks of design + implementation + 3 review rounds, Phase 1 at 3 weeks is **significantly optimistic**.

### My Estimate

r1's "5–7 weeks" is defensible. I'd break it differently:

**Phase 1a — graph tables only** (new territory, no MemoryRecord changes):
- Entity + Edge + invalidation schemas
- Basic CRUD, indexes
- Unit tests, invariant checks
- No LLM integration yet
- **~2.5 weeks** (all-new code, isolated from existing)

**Phase 1b — MemoryRecord extension** (retrofit territory, high blast radius):
- Add `episode_id`, `entity_ids`, `edge_ids`, `confidence` (or reliability per A3)
- Migration forward + rollback
- Update 28 consumer files
- SQL schema migration for `memories` table
- **~3 weeks** (retrofit, requires careful review, many touch-points)

**Total Phase 1: ~5.5 weeks**. Aligns with r1's 5–7 range.

### Recommendation

Adopt r1's suggestion (b): carve Phase 1 into 1a (graph tables, 2.5 wk) + 1b (MemoryRecord extension, 3 wk). Benefits:

- Phase 1a is pure-additive, can parallelize with other work
- Phase 1b has a clear prerequisite (1a) and a clear scope
- Milestone after 1a is meaningful: "can manually insert entities/edges, query them, no MemoryRecord changes yet"
- Makes the honest blast radius visible in the plan rather than hiding it in one 3-week bucket

Total v0.3 roadmap grows from 14 weeks to ~16.5 weeks — still "~4 months MVP, 6 months polished" territory, honest.

---

## B2 — NG5 contradicts §3.2

### r1's Claim
- §1 NG5: *"Not retrofitting v0.2's MemoryRecord — it stays, it plays a specific role"*
- §3.2 title: *"MemoryRecord — kept, extended"* (adds 4 fields)
- "Kept, extended" **is** retrofitting

### Verification

Direct quote check on both locations:

§1 NG5 (line 38):
> *"Not retrofitting v0.2's MemoryRecord — it stays, it plays a specific role (see §3)"*

§3.2 (line 106, header + struct):
> *"### 3.2 MemoryRecord (L2/L3) — kept, extended"*
>
> Struct diff: adds `episode_id`, `entity_ids`, `edge_ids`, `confidence` (4 fields)

Confirmed. The two sections **literally contradict**. Anyone reading NG5 first would believe MemoryRecord is untouched; anyone reading §3.2 first sees 4 new fields.

### Recommendation

Rephrase NG5. r1's suggestion is fine; I'd tighten further:

> **NG5**: *"Not replacing MemoryRecord with graph-only storage. v0.3 **extends** MemoryRecord with provenance links (`episode_id`, `entity_ids`, `edge_ids`) and a reliability/confidence field — see §3.2 for the exact diff. The episodic-trace role (decay, consolidation, affect-bearing) is preserved."*

Points:
- Replaces "not retrofitting" (misleading) with "not replacing" (accurate)
- Says "extends" explicitly
- Cross-references §3.2 so readers hit the truth immediately
- Keeps the non-goal's spirit (don't imagine we're throwing MemoryRecord away) while being honest about the diff

Also — consider pairing this with A3's resolution. If A3 lands on Alternative 1 (don't store confidence), the field count drops from 4 to 3. NG5 text should match whatever A3 resolves to.

---

## B3 — Phase 0 (Alignment, 1 week) is the wrong shape

### r1's Claim

Phase 0 is currently just "finalize DESIGN + run draft-requirements + gid_design". r1 argues it should explicitly produce:
1. Naming decision doc (working memory A/B/C — A1)
2. Predicate strategy doc (closed/emergent/hybrid — A2)
3. Confidence storage decision (stored/computed/cached/split — A3)
4. Scope carve-up of Phase 1 (single or 1a+1b — B1)

Without these, Phase 1 starts from ambiguity and re-litigates architecture mid-implementation.

### My Take

Agree entirely. This is good project hygiene. But let me expand the list based on the full A-series findings:

**Phase 0 deliverables (revised)**:
1. **A1** — working memory naming decision + rename executed (doc 06 analyzed, rename pending)
2. **A2** — predicate schema decision (seeded-with-override recommended; open: Phase 2 or 4)
3. **A3** — confidence field decision (Alt 1 recommended; open: any SQL-filter use case)
4. **A4/A5** — §4.5 rewrite (reactive error handling, not proactive gating)
5. **B1** — Phase 1a/1b carve-up approved by potato
6. **B2** — NG5 rephrased
7. **B3 implicit** — DESIGN §9 revised with new phase structure
8. Then: run `draft-requirements` skill to split master DESIGN into feature docs
9. Then: run `gid_design` to generate task graph

### Phase 0 Duration

Currently "1 week". With the explicit deliverables above, realistic sizing:
- Items 1–7 are decisions + doc edits — ~2 days IF potato is available to decide synchronously, ~1 week if decisions queue asynchronously
- Items 8–9 are mechanical — ~2 days

**Realistic Phase 0: 1–1.5 weeks**, matches current estimate IF decisions can happen in a concentrated session rather than async over days.

### Recommendation

Rewrite §9 Phase 0 with an explicit deliverable checklist. Block Phase 1 start until all 7 items above are checked. This prevents the "re-litigate during implementation" pattern.

---

## Combined DESIGN §9 Rewrite Preview

After B1 + B3 combined, §9 should look like:

```
### Phase 0 — Alignment & decision closure (~1 week)
Deliverables (all required before Phase 1 start):
- [ ] A1 working memory rename executed (SessionWorkingMemory → ActiveContext)
- [ ] A2 predicate schema decision (recommend: seeded + proposed)
- [ ] A3 confidence storage decision (recommend: keep computed)
- [ ] A4/A5 §4.5 rewrite (reactive error handling only)
- [ ] B1 Phase 1 carve-up approved (1a/1b split recommended)
- [ ] B2 NG5 rephrased
- [ ] B3 this §9 itself revised
- [ ] Master DESIGN split into feature-level docs (draft-requirements)
- [ ] Task graph generated (gid_design)

### Phase 1a — Graph tables foundation (~2.5 weeks)
- Entity + Edge + invalidation schemas
- CRUD, indexes, migrations
- Invariant checks, unit tests
- Milestone: manual entity/edge insertion and query works, no LLM

### Phase 1b — MemoryRecord extension (~3 weeks)
- Add provenance fields (episode_id, entity_ids, edge_ids)
- Add confidence/reliability field (per A3 decision)
- Update all 28 consumer files
- Migration forward/rollback
- Milestone: MemoryRecord consumers work with new fields, migration tested on real DB

### Phase 2 — Ingest pipeline (~3 weeks)
- Stages 1–6 end-to-end
- Extraction with graph context
- Multi-signal fusion (§4.3)
- Edge resolution prompt
- Schema inducer foundation (per A2, can partial-impl here or defer to Phase 4)
- Reactive error handling (per A4/A5)
- Milestone: ingest(episode) works; LOCOMO smoke test

### Phase 3 — Retrieval upgrade (~2 weeks) [unchanged]

### Phase 4 — Consolidation integration (~3 weeks)
- Retro-evolution
- Schema inducer full (per A2 if deferred from Phase 2)
- Offline audit
- Knowledge Compiler hook
- Milestone: 1000-episode simulation stays stable

### Phase 5 — Migration + benchmark + polish (~2 weeks) [unchanged]

Total: ~16.5 weeks (~4 months MVP, ~6 months polished).
```

Net: +2.5 weeks vs original 14wk estimate, but each phase is credibly scoped.

---

## Open Sub-Questions (for potato)

- Accept Phase 1 carve into 1a (2.5 wk) + 1b (3 wk)?
- Phase 0 deliverable checklist acceptable?
- NG5 rephrase acceptable?
- Should schema_inducer go in Phase 2 (partial) or Phase 4 (full)?

---

## Status

- [x] B1 evidence — r1's exact numbers unverifiable, but qualitative finding confirmed via grep (28 files use MemoryRecord, 25 use storage)
- [x] B1 calibration — ISS-024 comparison holds (3 review rounds for 1 field)
- [x] B2 contradiction confirmed — §1 NG5 vs §3.2 directly conflict
- [x] B3 agreed + expanded to 8-item Phase 0 deliverable list
- [x] §9 rewrite drafted above
- [ ] potato decisions per sub-questions
- [ ] Apply §9 + NG5 rewrites (batched)
