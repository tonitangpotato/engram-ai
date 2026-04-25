# A3 Analysis: MemoryRecord.confidence — Stored vs Computed

**Context**: r1 finding A3. Continuing DESIGN-v0.3-r1 resolution.
**Date**: 2026-04-24
**Status**: Analysis complete — recommendation ready

---

## r1's Claim

DESIGN §3.2 (line 131) adds to `MemoryRecord`:
```rust
pub confidence: f64,  // was computed; now stored
```

r1 argues this kills confidence's metacognitive character because:
- `retrieval_salience` is **relational** (depends on sibling records in the current set)
- A stored value is a snapshot — stale the moment related memories are added/decay
- Loses "how confident am I *right now*, given what I know *right now*" semantics

r1 proposed 3 alternatives, recommended **#3 (split — store reliability, compute salience)**.

---

## Evidence Verification

### Confidence.rs structure — matches r1's description exactly

`crates/engramai/src/confidence.rs` has exactly two primary functions:

**`content_reliability(record)` — pure intrinsic function** (L67–85):
- Inputs: `memory_type`, `contradicted_by`, `pinned`, `importance`
- All are fields on the record itself
- No dependence on other records or current state
- **Truly stateless given the record**

**`retrieval_salience(record, all_records: Option<&[MemoryRecord]>)` — relational** (L91–113):
- Normalizes `effective_strength(record)` against `max_strength` of `all_records`
- Returns different values depending on what set is passed
- Fallback to sigmoid when no set available
- **Inherently relational**

**`confidence_score` combines them** (L130–135):
```rust
pub fn confidence_score(record, all_records: Option<&[MemoryRecord]>) -> f64 {
    let reliability = content_reliability(record);
    let salience = retrieval_salience(record, all_records);
    (0.7 * reliability + 0.3 * salience).clamp(0.0, 1.0)
}
```

70% intrinsic + 30% relational. The code already embodies Alternative 3's split.

### Surprise finding — DESIGN's comment `// was computed` is factually wrong

Checked current `MemoryRecord` struct in `crates/engramai/src/types.rs`:

```
pub struct MemoryRecord {
    // ... id, content, memory_type, layer, created_at,
    //     access_times, working_strength, core_strength,
    //     importance, pinned, consolidation_count, last_consolidated,
    //     source, contradicts, contradicted_by, superseded_by, metadata ...
}
```

**No `confidence` field. Not stored. Not computed inline. Not present.**

Where confidence actually lives in v0.2:
```rust
pub struct RecallResult {
    pub record: MemoryRecord,
    pub activation: f64,
    pub confidence: f64,          // ← computed at recall time
    pub confidence_label: String,
}
```

Confidence is a **recall-time projection**, not a record property. It's produced by `calibrate_confidence(record, activation, all_records)` at the moment of recall.

**The DESIGN comment `// was computed; now stored` misdescribes v0.2 state.** It was never on `MemoryRecord` — it was on `RecallResult`. "Computed" is right; "was on the record" is not.

This makes r1's critique stronger: the change isn't migrating an existing field from computed-on-read to stored. It's **inventing a new stored field** and presenting it as a minor refinement of existing state. That's a subtle but important framing error in the DESIGN.

---

## What r1 Missed / Got Right

### Got right

- Relational nature of salience kills the stored semantics ✓
- Write amplification concern ✓ (every consolidation updates `core_strength` → stored confidence stale)
- Clean split between reliability (intrinsic) and salience (relational) already exists ✓
- Alternative 3 is the right answer ✓

### Missed

- **The framing error in DESIGN**: `// was computed; now stored` implies a move from one storage strategy to another for an existing field. In reality, the field didn't exist on `MemoryRecord` at all. This isn't a migration — it's an addition. Worth flagging in the DESIGN rewrite so readers don't misunderstand v0.2 behavior.
- **`calibrate_confidence` exists as a separate function** (L~228 in confidence.rs) — used specifically by recall to incorporate activation. This is a third dimension of "confidence" that DESIGN §3.2 elides entirely. If we split reliability from salience, we should also be honest that **recall-time confidence ≠ record-intrinsic reliability** — they're different measurements serving different purposes.

---

## Three Alternatives — Re-Evaluated

### Alternative 1: Don't store (keep everything computed)
- **Cost**: Compute on every recall. Already happens in v0.2; already cheap (pure function over ≤100 records typically).
- **Benefit**: Zero schema change, zero migration, zero staleness.
- **Loss**: Can't filter/query records by "confidence > 0.X" in SQL — must project in Rust.
- **Verdict**: Simplest. Defensible as "v0.3 keeps v0.2's behavior here."

### Alternative 2: Cache with TTL + dirty bit
- **Adds**: `last_computed_confidence: Option<f64>` + `confidence_computed_at: DateTime`
- **Benefit**: Avoids recompute when unchanged.
- **Cost**: Cache invalidation logic (every `core_strength`/`working_strength`/`pinned`/`contradicted_by` change → dirty). Two fields. Still needs recompute path for freshness.
- **Verdict**: Premature optimization. Confidence compute is cheap. Skip.

### Alternative 3: Split — store reliability, compute salience (r1's recommendation)
- **Adds**: `pub content_reliability: f64` on `MemoryRecord`
- **Benefit**: Matches actual function structure. SQL-queryable by reliability. Salience stays recall-time (correct semantics). No staleness (reliability updates only when intrinsic fields change — same triggers as the fields themselves).
- **Cost**: One stored field; recomputation trigger on `pinned`/`contradicted_by`/`importance` mutations (low frequency).
- **Verdict**: Best of both. Preserves relational salience semantics. Enables SQL queries on reliability. Matches codebase's actual split.

### My take: **Alternative 1 for v0.3.0, Alternative 3 for v0.3.x when SQL-side filtering becomes needed**

Reason: DESIGN §3.2 hasn't justified *why* confidence needs to be stored. If the motivating use case is "filter memories by confidence in a SQL query" — great, Alternative 3 is the move. If there's no concrete use case, **Alternative 1 is the Occam's razor choice** (no schema change, no new field, no migration, no semantics drift).

Asking potato to decide: is there a concrete v0.3.0 feature that needs `WHERE confidence > 0.X` at SQL level? If not, Alternative 1. If yes, Alternative 3.

Note: `RecallResult.confidence` stays regardless — it's the recall-time output, always computed from reliability + salience + activation.

---

## Effort Estimate

### If Alternative 1 (keep computed)
- **DESIGN change**: remove `pub confidence: f64` line from §3.2. Add a short paragraph: "Confidence is computed at recall time by `confidence_score(record, all_records)` — see `confidence.rs`. It is not a stored property because salience is relational to the current recall set."
- **Code change**: 0.
- **Tests**: unchanged.
- **Migration**: none.
- **Total**: ~30 min (DESIGN edit only).

### If Alternative 3 (split)
- **DESIGN change**: replace `pub confidence: f64` with `pub content_reliability: f64`. Add paragraph explaining the split. Reference confidence.rs's existing structure.
- **Code**:
  - Add `content_reliability: f64` field to `MemoryRecord` (types.rs).
  - On every mutation path that changes `pinned`/`contradicted_by`/`importance`/`memory_type`: recompute and update `content_reliability`. In practice these are: `store_raw`, `pin`, `unpin`, `set_contradiction`, `supersede`. ~5 call sites.
  - `RecallResult.confidence` stays — but internally pulls `record.content_reliability` instead of calling `content_reliability(record)`.
  - SQL schema: add column, backfill with `UPDATE ... SET content_reliability = ...` on migration.
- **Tests**: update existing confidence tests to verify reliability is persisted correctly. ~4–6 new test cases.
- **Migration**: idempotent SQL migration. Compute reliability for all existing records. Low risk.
- **Total**: **~2 days**, incl. migration + tests.

---

## Recommendation

1. **Default to Alternative 1** for v0.3.0 — no schema change, no field added, confidence stays recall-time computed.
2. **Fix the `// was computed; now stored` comment** regardless — it misdescribes v0.2 reality (field never existed on MemoryRecord).
3. **Document in §3.2** that confidence is a recall-time projection (see `RecallResult`, `confidence.rs`). Reference the reliability/salience/activation trifecta.
4. **If concrete SQL-filtering use case emerges** → upgrade to Alternative 3 in v0.3.x. Keep the option open.
5. **Don't go Alternative 2** — premature, confidence compute is cheap.

---

## Open Sub-Questions (for potato)

- Is there a planned v0.3 feature that needs SQL-level filtering by confidence? (Determines Alt 1 vs Alt 3.)
- Should `MemoryRecord` expose a `fn content_reliability(&self) -> f64` convenience method (calls `confidence::content_reliability`)? Low-cost API improvement.

---

## Status

- [x] Evidence verified — r1 accurate on all claims
- [x] Surprise finding: DESIGN comment misdescribes v0.2 state (`// was computed` false)
- [x] Codebase already implements Alt 3's split at function level
- [x] Recommendation formed (Alt 1 default, Alt 3 if SQL-filter need arises)
- [ ] potato decision: any SQL-filter use case in v0.3?
- [ ] DESIGN §3.2 rewrite (batched with other r1 findings)
