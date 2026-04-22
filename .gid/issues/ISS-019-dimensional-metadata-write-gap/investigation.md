# ISS-019: Dimensional metadata write gap

**Status:** investigation
**Feature:** dimensional-extract (in-progress, unstaged)
**Severity:** high — silently drops ~76% of dimensional metadata
**Downstream consumer:** ISS-020 (KC dimensional awareness) — Knowledge
Compiler's P0 work depends on `memory.metadata.dimensions` being reliably
present. Fixing ISS-019 directly raises KC's effective enrichment coverage
from ~24% → ~100% of newly written memories. No code coordination needed;
KC reads through `Option<Dimensions>` and improves as soon as write path
is patched.

## Observed behavior

During the dimensional-extract rebuild pilot:

- LLM extractor (`extractor.rs`) correctly returns rich dimensional facts:
  `valence`, `domain`, `participants`, `temporal`, `location`, `context`,
  `causation`, `outcome`, `method`, `relations`, `sentiment`, `stance`.
- Yet in sqlite, ~76% of recently stored memories have `metadata IS NULL`
  or lack any `dimensions` sub-object.
- Field coverage among memories that *do* carry metadata is sparse:
  `participants` 9–22%, most other dimensions in the same range.
- Time-series shows metadata writes only start succeeding on
  2026-04-22 02:00+ UTC, which is after the extractor path was
  partially wired up but before the write path was complete.

## Root cause (three leaks, same symptom)

All three leaks live on the WRITE path, not the extractor.

### Leak 1: extractor error → silent raw-fallback (LOSS)

`memory.rs::add_to_namespace()` around L1380–1393:

```rust
Err(e) => {
    log::warn!("Extractor failed, storing raw: {}", e);
    // Fall through to raw storage below
}
...
self.add_raw(content, memory_type, importance, source, metadata, namespace)
```

When the LLM call fails (timeout, rate-limit, parse error, any transient),
we fall through to `add_raw()` with the caller's `metadata` parameter —
which for the rebuild pipeline is `None`. All dimensional data that the
extractor *might* have produced on retry is gone, and the record is
stored with `metadata = NULL`.

The warn log is the only signal. There's no retry, no partial-save, no
marker on the record saying "extractor failed, this is raw fallback".

### Leak 2: `empty facts` path returns early with no record (LOSS)

```rust
Ok(_) => {
    log::info!("Extractor: nothing worth storing ...");
    return Ok(String::new());
}
```

Returning empty string id means the caller thinks nothing happened. For
a batch rebuild this is fine in principle, but combined with leak 1 it
hides how much content is silently skipped. We have no counter, no
sampling log, no way to audit "how many of the 58MB corpus were
classified as not-worth-storing".

### Leak 3: dedup merge discards incoming dimensional metadata (LOSS)

`storage.rs::merge_memory_into()` L3024+:

```rust
pub fn merge_memory_into(
    &mut self,
    existing_id: &str,
    new_content: &str,
    new_importance: f64,
    similarity: f32,
) -> Result<MergeOutcome, rusqlite::Error>
```

The signature **does not accept the incoming record's metadata**. The
function fetches the existing record's metadata, appends a
`merge_history` entry, bumps `merge_count`, and writes back. The
incoming fact's `dimensions` / `type_weights` are never considered.

Consequence: the first time a fact is stored determines the dimensional
signature forever. If an earlier, shallower extraction landed first
(fewer dimensions populated), every later richer extraction for the
same semantic content gets merged in and its dimensions are discarded.

This is especially bad for the rebuild pilot, because:
- Dedup is enabled.
- The rebuild re-processes content that already exists in the DB.
- So every "better" re-extraction hits the dedup path and loses data.

## Evidence

- `merge_memory_into` signature: `src/storage.rs:3024–3029`
- Dedup call sites: `src/memory.rs:1478, 1517` — both pass only
  `content`, `importance`, `similarity`. No metadata argument exists.
- `add_to_namespace` fallback: `src/memory.rs:1380–1393`.
- Caller metadata is provided to `add_to_namespace` but in the
  extraction branch it's only *merged into* the dimensional metadata
  (L1358–1367) — good. In the fallback branch it's passed through
  verbatim — which is useless for the rebuild pipeline because the
  rebuild caller doesn't synthesize dimensions itself.

## Why this wasn't caught

- Feature is in-progress, unstaged. No end-to-end test yet on the merge
  path for dimensional data.
- The warn log for extractor failures is easy to miss in a 58MB batch.
- Coverage metric was not part of the pilot acceptance criteria.

## Proposed fix (scope for a follow-up ISS / PR)

Three fixes, smallest first:

### Fix A: thread dimensional metadata through merge

Change `merge_memory_into` signature to accept an optional
`incoming_metadata: Option<&serde_json::Value>`. When present:

1. If existing record's `dimensions` object is missing or has fewer
   populated keys than incoming → replace / union (prefer richer).
2. Union `type_weights` by taking max weight per dimension.
3. Preserve `merge_history` / `merge_count` append behavior.

Policy: **never delete an existing dimension value** unless the
incoming one is strictly richer. Union, don't overwrite.

Update both call sites in `add_raw` (L1478, L1517) to pass the
incoming dimensional metadata built in `add_to_namespace` — which
requires either:

- (a) building the dimensional metadata *before* calling `add_raw`
  and passing it through (preferred; single source of truth), or
- (b) running the extractor again inside the merge path (rejected:
  double LLM cost).

Option (a) means `add_to_namespace` already builds `fact_metadata`
for each extracted fact — we just need to keep passing it all the
way down so dedup can merge it.

### Fix B: mark extractor-failed raw fallbacks

When the extractor returns `Err(_)`, before calling `add_raw`, inject
a marker into `metadata`:

```json
{ "extractor_status": "failed", "extractor_error": "<msg>" }
```

Purpose: make leak 1 visible in the data, not just in logs. The
rebuild pilot can then scan for `extractor_status = failed` and
either retry those records or exclude them from coverage metrics.

### Fix C: counter + structured log for empty-facts path

Increment a counter (`extractor_empty_count`) when the extractor
returns `Ok(empty)`. Log at `info` with a short content hash so we
can audit what's being skipped without storing the full content.

Expose via `MemoryManager::stats()` for the pilot to read.

## Validation plan

Before rolling out to the full 58MB corpus:

1. **Unit test for merge path**: store a fact with only `valence` +
   `domain`; re-store the "same" content with richer dimensions
   (`participants`, `temporal`, `causation`); assert the merged
   record has all dimensions populated.
2. **Unit test for extractor-failure path**: mock extractor returning
   `Err`; assert record is written with `extractor_status=failed`
   in metadata.
3. **5KB smoke rebuild**: pick one day of agent session data, run
   the pilot end-to-end, assert:
   - metadata coverage > 95%
   - `dimensions.participants` coverage > 60% (not 9%)
   - zero records with `metadata IS NULL` unless
     `extractor_status=failed` is set.
4. Only then: full 58MB rebuild.

## Non-goals (this ISS)

- Not changing the extractor prompt / schema.
- Not changing dedup thresholds.
- Not changing `source_text` storage policy (deliberately out per
  comment at L1354).
- Not adding retry on extractor failure — separate concern, ISS to
  be filed if pilot data shows high failure rate.

## Open questions

- Should the extractor-failure marker be at the top-level of
  metadata (`extractor_status`) or nested (`_engram.extractor_status`)?
  Recommendation: nest under `_engram` to avoid colliding with
  caller-provided metadata keys.
- For merge union of `dimensions`, when both sides have a non-empty
  value for the same dimension, which wins? Recommendation: the
  longer string wins (proxy for "richer extraction"). Alternative:
  keep both as an array. Simplicity argues for longer-wins.
