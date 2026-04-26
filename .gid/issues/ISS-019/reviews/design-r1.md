# Design Review r1 — ISS-019

**Document reviewed:** `.gid/issues/ISS-019-dimensional-metadata-write-gap/design.md` (v1)
**Reviewer:** self-review (author)
**Date:** 2026-04-22
**Verdict:** 8 findings — 2 critical, 3 important, 3 minor. Apply all.

---

## FINDING-1 — TypeWeights dimension missing from Dimensions struct

**Severity:** Critical
**Location:** design.md §2.1 (Dimensions struct definition)

**Problem.** The design omits `TypeWeights` (from `src/type_weights.rs`)
as a field of `Dimensions`. TypeWeights is produced by
`infer_type_weights(&ExtractedFact)` and is part of what the extractor
effectively emits — it gates type-gated recall. A `Dimensions` that
doesn't carry it silently drops half the feature's output and reintroduces
leakage on the very path we're fixing.

**Fix.** Add to `Dimensions`:

```rust
pub type_weights: TypeWeights,  // not Option; default = all 1.0
```

Add to `Dimensions::union` (§5.1):

```text
type_weights — per-variant max (episodic, factual, procedural, …).
               Never reduces a weight. Inferred type affinity only grows.
```

Update §6 v2 metadata layout to nest `type_weights` under
`engram.dimensions.type_weights`.

---

## FINDING-2 — Breaking-change surface undercounted

**Severity:** Critical
**Location:** design.md §3.3, §10 R4

**Problem.** Review claims "`add_to_namespace` kept as deprecated shim."
In reality there are **three** public write entry points, not one:

- `Memory::add` (memory.rs:1255)
- `Memory::add_to_namespace` (memory.rs:1281)
- `Memory::add_with_emotion` (memory.rs:1711)

And downstream RustClaw calls `.add()` (rustclaw/src/memory.rs:416, 610),
not `.add_to_namespace()`. The v1 design silently breaks RustClaw.

**Fix.** Pick one of two strategies, explicitly:

- **Strategy A (chosen):** All three entry points become `#[deprecated]`
  shims routing to `store_raw`. Signatures unchanged. Return type stays
  `Result<String>`; a `Quarantined` outcome is surfaced as
  `Err(StoreError::Quarantined { id })` rather than silent success.
  `add_with_emotion` routes to `store_raw` + a follow-up emotion hook
  (preserving its current two-step behavior).
- **Strategy B (rejected):** Hard-break, release as v0.3. Rejected
  because RustClaw is a production dependency and the dimensional
  extract feature is shipping incrementally; a double-migration hurts.

Add to §11 (Implementation plan): **Step 4.5 — shim layer for all three
legacy entry points + explicit RustClaw integration test**.

---

## FINDING-3 — Embedding ownership on store_enriched is undefined

**Severity:** Important
**Location:** design.md §3.1 (store_enriched)

**Problem.** Dedup works via embedding similarity. `store_enriched`
accepts an `EnrichedMemory` with content + dimensions, but does not
specify who computes the embedding. If the caller must pass one, the
field is missing from `EnrichedMemory`. If `store_enriched` computes
one, we lose the batching optimization that makes rebuild pilots
affordable (embedding per-call is the dominant cost).

**Fix.** Extend `EnrichedMemory`:

```rust
pub struct EnrichedMemory {
    pub content: String,
    pub dimensions: Dimensions,
    pub embedding: Option<Embedding>,  // None = compute inline
    pub importance: Importance,
    pub source: Option<String>,
    pub namespace: Option<String>,
    pub user_metadata: serde_json::Value,
}
```

Semantics: `store_enriched` computes embedding only if `None`. Rebuild
pilot batches via a separate `precompute_embeddings(&mut [EnrichedMemory])`
helper and then calls `store_enriched` with pre-filled embeddings. No
duplicated work, no hidden cost.

---

## FINDING-4 — Quarantine semantics break "extractor not configured"

**Severity:** Important
**Location:** design.md §4 (quarantine table)

**Problem.** The design routes every extractor failure to quarantine.
But two legitimate cases are not failures:

- Unit tests / integration tests without a live LLM.
- Deployments where the user opted out of dimensional extraction.

In both, every `store_raw` call would land in quarantine. That's wrong
— quarantine is for *transient failure*, not *intentional absence*.

**Fix.** Define a distinct fallback path. Add to `Dimensions`:

```rust
impl Dimensions {
    /// Minimal dimensions from raw content. core_fact = content,
    /// everything else None / default. Legitimate low-dimensional
    /// memory, not an error.
    pub fn minimal(content: &str) -> Result<Self, EmptyCoreFactError>;
}
```

Redefine `store_raw` behavior:

- **No extractor configured** → build `Dimensions::minimal(content)` +
  `EnrichedMemory` + `store_enriched`. Returns `RawStoreOutcome::Stored`.
- **Extractor configured, returns empty facts** → `Skipped { reason }`.
- **Extractor configured, runtime failure** → `Quarantined`.

The main-table invariant is preserved (every row has a `Dimensions`),
but we don't penalize extractor-less deployments.

Add to §12 (Non-goals): "Not requiring an extractor for basic storage."

---

## FINDING-5 — v1→v2 "information loss" predicate is vague

**Severity:** Important
**Location:** design.md §6 (Migration strategy)

**Problem.** The design says: "if reconstruction loses information,
record is flagged for re-extraction." No formal predicate. Without it
the backfill job is undefined.

**Fix.** Define `LegacyClassification` explicitly:

```rust
enum LegacyClassification {
    /// v1 row carries enough to rebuild Dimensions lossless-ly.
    CleanUpgrade,
    /// v1 row missing ≥1 of {participants, temporal, causation}
    /// AND content length > 40 chars (short content is OK to stay
    /// minimal; long content without these dimensions is suspicious).
    NeedsBackfill { missing: Vec<&'static str> },
    /// v1 row had no `dimensions` field at all; pre-extractor era.
    PreExtraction,
}
```

Migration rules:

- `CleanUpgrade` → rewrite as v2, mark `version: 2`, done.
- `NeedsBackfill` → rewrite as v2 with partial dimensions, enqueue in
  `backfill_queue` (table below), **keep original in memories table**
  (better partial dims than missing).
- `PreExtraction` → same as NeedsBackfill, with `missing = all`.

```sql
CREATE TABLE backfill_queue (
    memory_id    TEXT PRIMARY KEY,
    enqueued_at  REAL NOT NULL,
    reason       TEXT NOT NULL,         -- serialized LegacyClassification
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT
);
```

Backfill job reuses `retry_quarantined` logic but reads from
`backfill_queue` and UPDATEs the existing `memories` row via
`merge_memory_into(existing, incoming, 1.0)`. Same invariants, same
merge semantics. Information monotone grows.

---

## FINDING-6 — Domain::Other comparison is underspecified

**Severity:** Minor
**Location:** design.md §5.1 (union strategy per field)

**Problem.** `Domain` is an enum with `Other(String)`. Design states
"existing wins unless Other and other is concrete." Unclear what
happens when both sides are `Other(String)` with different values.

**Fix.** Merge rule for `Domain`:

1. Concrete variant (Coding, Trading, …) beats `Other(_)` regardless
   of side.
2. Two concretes: `existing` wins (stable).
3. Two `Other(a)` vs `Other(b)`: longer string wins; tie → `existing`.

Document in §5.1 verbatim.

---

## FINDING-7 — union is not commutative; tests must reflect this

**Severity:** Minor
**Location:** design.md §5.3 (idempotence + associativity claims)

**Problem.** Fields like `sentiment` and `stance` use "existing never
overwritten" semantics. This makes `union` **non-commutative**:
`a.union(b) ≠ b.union(a)` when both sides carry sentiment. The design
claims idempotence + associativity (true) but did not explicitly
disclaim commutativity. A proptest that asserts commutativity will
fail.

**Fix.** Add to §5.3:

> **Non-commutative by design.** `a.union(b)` treats `a` as "existing"
> and `b` as "incoming". For fields where speaker identity matters
> (sentiment, stance), the existing side is preserved. Tests must
> never assert commutativity — only idempotence, associativity, and
> monotonicity.

Add to proptest plan in §11 Step 3:

- ✅ `a.union(a) == a` (idempotent)
- ✅ `(a.union(b)).union(c) == a.union(b.union(c))` (associative)
- ✅ `info_content(a.union(b)) ≥ info_content(a)` (monotone)
- ❌ `a.union(b) == b.union(a)` — **NOT asserted**

---

## FINDING-8 — Implementation plan Step 5 ordering bug

**Severity:** Minor
**Location:** design.md §11 Step 5

**Problem.** Step 5 says "Delete legacy `merge_memory_into` overload in
the same PR." But Step 4 installed `add_to_namespace` as a shim, and
that shim still calls the *legacy* `merge_memory_into` signature at
memory.rs:714, 1478, 1517. Deleting the legacy overload in Step 5
breaks the shim.

**Fix.** Reorder implementation steps:

- **Step 5:** Introduce new `merge_memory_into(&EnrichedMemory)` signature
  under a different name (`merge_enriched_into` or `merge_memory_into_v2`).
  Both coexist. Shim can still call legacy.
- **Step 5.5 (new):** Migrate all internal callers to new signature.
  Shim now calls `merge_enriched_into`.
- **Step 5.9 (new, optional):** Delete legacy `merge_memory_into` once
  all callers verified migrated via `grep`. Rename
  `merge_enriched_into` → `merge_memory_into` as the final canonical
  name.

This keeps each step leaving the tree compilable (design.md §11
preamble invariant).

---

## Apply decision

All 8 findings accepted. Proceed to design.md v2.
