# Class D Analysis: Smaller Nits

**Context**: r1 Class D findings — polish-level issues. Quick batch analysis.
**Date**: 2026-04-24

---

## D1 — §0 "none of [Graphiti/mem0/A-MEM/Letta/LightRAG] have the cognitive layer"

### r1's Claim
True in aggregate but each has *something* (mem0 importance scoring, Letta forgetting). Weaken to avoid cherry-picking.

### Verification
Not checked individually, but r1's framing is standard review discipline — absolute claims ("none") invite nit-picks. Rephrasing to relative ("none combine at engram's depth") is lower-risk marketing.

### Recommendation
r1's rephrase accepted: *"none combine decay + affect + consolidation + interoception at engram's depth"*. Trivial 1-line edit.

### Status
- [x] Agreed
- [ ] Apply to §0 (batched)

---

## D2 — G3 "2–3 LLM calls per episode vs Graphiti's 5–10" is aspirational

### r1's Claim
No measurement plan. Add concrete success criterion.

### Verification
Read §11 success criteria — currently says "Average LLM calls per episode ≤ 3 (measured over LOCOMO runs)" but doesn't specify HOW it's measured.

### Recommendation
Add to §11:
> *"Measured by a counter in `write_stats.rs` (to be added in Phase 2), averaged over a benchmark corpus of **N=500 LOCOMO episodes** spanning all intent categories. Counter increments on every LLM call originating from `ingest()`, excluding consolidation-time calls (which are measured separately)."*

Also add a **stretch goal**: "≤ 2 LLM calls/episode for P50 traffic" — most episodes should hit the cheap fusion path; LLM escalation should be the minority.

Effort: 10 min doc edit + 1 hour Phase 2 implementation (counter wiring).

### Status
- [x] Real gap
- [ ] Apply to §11

---

## D3 — §7 Public API `recall()`/`recall_with()` signature conflict with existing memory.rs

### r1's Claim
Verify call-site compatibility; if new signatures change, NG1 backward-compat is at risk.

### Verification — **Signatures DO conflict**

**Current v0.2 signature** (memory.rs:2687):
```rust
pub fn recall(
    &mut self,
    query: &str,
    limit: usize,
    context: Option<Vec<String>>,
    min_confidence: Option<f64>,
) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>>
```
- **sync** (not async)
- `&mut self` (mutates — updates access times for ACT-R)
- 4 positional params
- Returns `Result<_, Box<dyn Error>>`

**DESIGN §7.1 signature**:
```rust
pub async fn recall(&self, query: &str, opts: RecallOptions) -> Result<Vec<RecallResult>>
```
- **async**
- `&self` (no mutation?? — but ACT-R access-time update requires mut)
- `opts` struct instead of positional args
- `Result<_>` (error type not specified)

**Three concrete conflicts**:
1. **async vs sync** — breaking change to all 60+ callers in rustclaw + any external crates on v0.2.2
2. **mutability** — `&self` loses ACT-R access-time update (or requires interior mutability via Mutex/RwLock, which changes concurrency model)
3. **Signature shape** — positional → struct requires every call site to migrate

### Recommendation

**§7.3 backward compat claim is false as written.** Fix by one of:

**Option A — preserve v0.2 signature, add new method**:
```rust
// v0.2 API stays verbatim
pub fn recall(&mut self, query: &str, limit: usize, ctx: Option<Vec<String>>, min: Option<f64>)
    -> Result<Vec<RecallResult>, Box<dyn Error>>;

// New v0.3 API with opts struct + async extras
pub async fn recall_v3(&mut self, query: &str, opts: RecallOptions)
    -> Result<Vec<RecallResult>, Box<dyn Error>>;
```
v0.2 callers work untouched. v0.3 callers use `recall_v3` for new features (graph routing, temporal filters, affect weighting). No deprecation in v0.3.0; deprecate v0.2 `recall` in v0.4.

**Option B — break compat, bump major to v0.3 with migration guide**:
Accept that v0.2 → v0.3 is breaking. Remove `recall` as it was; new signature is the only one.

**Option C — impl `Into<RecallOptions>` for the v0.2 tuple**:
Keep one method name, use traits to accept both call shapes. Elegant but async vs sync cannot be reconciled this way.

**My take**: **Option A**. Preserves NG1 backward compat honestly. The "same name, new signature" approach in DESIGN §7.3 is wishful.

Effort: Option A is trivial in Phase 3 (~1 hour to keep both methods + differentiate semantics). Option B is months of ecosystem churn.

### Status
- [x] Real conflict, DESIGN §7.3 "backward compat" claim inaccurate
- [ ] Potato decision: Option A (recommended) or B
- [ ] §7.1/§7.3 rewrite based on choice

---

## D4 — §8.1 migration has no rollback plan

### r1's Claim
v0.2 → v0.3 schema migration is one-way. Add rollback script OR explicit "no rollback, snapshot first" warning.

### Verification
§8.1 text: "Migration tool: `engramai migrate --from 0.2 --to 0.3`. Runs idempotently: Add new tables and columns; Backfill..."

No rollback path defined. Confirmed.

### Recommendation

Pragmatic choice:

**Rollback is HARD** because:
- New columns can be dropped, but data written to them is lost on rollback
- New tables (episodes, entities, edges) can be dropped but their content is lost
- Backfilled entity/edge extractions are LLM-produced and expensive to redo

**Options**:
1. **Automatic backup + manual rollback**: migration tool creates `*.pre-v03.db.bak` before starting. Rollback is `mv bak db`. No data migration on rollback — user accepts losing everything written during v0.3 phase.
2. **Dual-write period**: during a transition window, v0.3 writes to both old schema (backward-compat) and new schema (v0.3). Rollback = point v0.2 at the DB. Complex; probably not worth it for a local embedded DB.
3. **Snapshot + warning**: no rollback script, but migration tool emits a prominent warning: "BACKUP YOUR DB FIRST — v0.3 migration is forward-only. Suggested: `cp engram-memory.db engram-memory.v02.db` before running."

**My take**: Option 1 (automatic backup) + Option 3 (warning) combined. The migration tool should:
- Require `--accept-forward-only` flag OR interactive confirm
- Create `{db}.pre-v03.bak` automatically unless `--no-backup` is passed
- Print: "Rollback: `mv {db}.pre-v03.bak {db}` — data written after migration WILL BE LOST on rollback."

Effort: Small addition to migration tool (~2 hours in Phase 5). Doc change ~20 minutes.

### Status
- [x] Real gap
- [ ] Apply to §8.1 (batched)

---

## D5 — `RelatedTo` is a semantic landmine

### r1's Claim
Fallback predicate will accumulate 60%+ of edges (every LLM extraction that doesn't match exactly). Either accept + deprioritize, or refuse rather than fall back.

### Verification — Connects to A2

This is **downstream of A2**. Once A2's hybrid `Seeded | Proposed(String)` is adopted:
- `RelatedTo` no longer exists as a fallback
- LLM either uses a seeded variant or proposes a concrete string
- "fallback landmine" problem dissolves

### Recommendation
D5 is **resolved by A2's resolution**. After adopting hybrid predicates, the extractor never falls back — it either picks a seeded match or proposes a phrase like "next_to" / "caused_by" / "works_with".

Add a note to the A2 DESIGN rewrite:
> *"Removing `RelatedTo` as a fallback: previous versions had a RelatedTo catchall; the new hybrid schema (Seeded | Proposed(String)) lets the extractor propose precise predicates instead of falling back. This sharpens edge semantics."*

### Status
- [x] D5 subsumed by A2 resolution
- [ ] Ensure A2 DESIGN rewrite mentions RelatedTo removal

---

## D6 — Missing observability (`explain_recall`)

### r1's Claim
With 5 layers + graph + cognitive signals, debuggability is essential. No mention of how users inspect "why did you think X?". Add `explain_recall(query) -> RecallTrace`.

### Verification
§7 public API — checked. No explain/trace method. Confirmed gap.

### Recommendation

Add to §7.1:
```rust
/// Returns a trace showing how the recall arrived at its results:
/// - which layer(s) were queried
/// - intent classification outcome (heuristic or LLM, confidence)
/// - individual signal contributions to each result's score
/// - graph traversal steps (if any)
/// - final ranking and cutoffs
pub fn explain_recall(&self, query: &str, opts: RecallOptions) -> Result<RecallTrace>;

pub struct RecallTrace {
    pub query: String,
    pub intent: (QueryIntent, f64, ClassificationMethod),
    pub routing: Vec<LayerQuery>,
    pub scoring: Vec<ScoreBreakdown>,
    pub results: Vec<RecallResult>,
    pub timings: TraceTimings,
}
```

Implementation note: cheapest version is to plumb a `trace: Option<&mut RecallTrace>` optional param through the recall path. When None, zero overhead. When Some, each stage appends its debug info.

Effort: Phase 3 add, probably ~4 hours of plumbing through existing recall internals.

### Status
- [x] Real gap, accepted
- [ ] Apply to §7.1 (batched)

---

## Combined Effort Summary

| Finding | Action | Effort | Phase |
|---|---|---|---|
| D1 | Rephrase §0 | 5 min | Phase 0 |
| D2 | Tighten §11 + Phase 2 counter | 10 min + 1 hr | Phase 0 + Phase 2 |
| D3 | Rewrite §7.1/§7.3 (Option A) | 30 min + Phase 3 impl | Phase 0 + Phase 3 |
| D4 | Add backup + warning policy to §8.1 | 20 min + 2 hr tool | Phase 0 + Phase 5 |
| D5 | Subsumed by A2 | — | — |
| D6 | Add `explain_recall` to §7.1 | 15 min + ~4 hr Phase 3 | Phase 0 + Phase 3 |

**Phase 0 doc time**: ~1.5 hours. **Deferred implementation**: ~7 hours spread across Phase 2/3/5.

---

## Open Sub-Questions (for potato)

- D3: Option A (dual methods) vs Option B (break compat)? I recommend A.
- D4: `--accept-forward-only` flag acceptable as the rollback story?
- D6: `explain_recall` worth it for v0.3.0 or defer to v0.3.1?

---

## Status

- [x] D1 — easy rephrase
- [x] D2 — easy tightening
- [x] D3 — real conflict, needs potato decision
- [x] D4 — real gap, solution drafted
- [x] D5 — resolved via A2
- [x] D6 — real gap, solution drafted
- [ ] Apply D-series edits (batched with other r1 findings)
