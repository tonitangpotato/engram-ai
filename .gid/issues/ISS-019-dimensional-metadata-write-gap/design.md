# ISS-019 Design: Dimensional memory as a first-class type

**Status:** draft v2 (review-r1 applied)
**Supersedes:** the Fix A/B/C patch plan in `investigation.md`
**Scope:** WRITE path + merge path for `dimensional-extract` feature
**Review history:** see `reviews/design-r1.md` (8 findings applied)

---

## 1. Guiding principle (first principles)

> **A memory without a dimensional signature is not a memory.**

The existing code models dimensions as `Option<serde_json::Value>` sitting
next to content. The type system therefore claims "dimensions are optional",
but the feature's reason for existing claims the opposite. That mismatch
is the root cause of every leak in ISS-019:

- Leak 1 (extractor error → raw fallback) — possible because the write
  path accepts `metadata = None`.
- Leak 2 (empty facts silent skip) — possible because the return type
  is `Result<String>`, which can't express "skipped, here's why".
- Leak 3 (merge drops dimensions) — possible because `merge_memory_into`
  takes `content` and `importance` but not dimensions; the merge
  signature physically cannot carry the information it should merge.

**Fix strategy:** lift dimensions from untyped JSON into a strongly-typed
Rust value, and split the write path by semantic intent rather than
optional arguments.

Result: the compiler refuses to construct the bug.

---

## 2. Core types

### 2.1 `Dimensions`

```rust
/// Dimensional signature of a memory.
///
/// Models 11 semantic dimensions. Every field is either present with a
/// value or explicitly absent (None). Construction is the only time
/// these fields can be "missing" — once a Dimensions is in the system,
/// every later operation is expressed in terms of this type, not raw
/// JSON.
///
/// Invariant: `core_fact` is non-empty. Enforced at construction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dimensions {
    // Core (required)
    pub core_fact: NonEmptyString,

    // Narrative dimensions (optional, free-form strings)
    pub participants: Option<String>,
    pub temporal:     Option<TemporalMark>,  // richer than string, see §2.2
    pub location:     Option<String>,
    pub context:      Option<String>,
    pub causation:    Option<String>,
    pub outcome:      Option<String>,
    pub method:       Option<String>,
    pub relations:    Option<String>,
    pub sentiment:    Option<String>,
    pub stance:       Option<String>,

    // Scalar dimensions (always present, have sensible defaults)
    pub valence: Valence,        // f64 clamped to [-1.0, 1.0]
    pub domain:  Domain,         // enum, not free string
    pub confidence: Confidence,  // enum: Confident | Likely | Uncertain
    pub tags: BTreeSet<String>,  // set, not Vec (dedup + stable ordering)

    // Inferred type affinity (from type_weights.rs:infer_type_weights)
    // Default = all 1.0 (no type bias). Present on every Dimensions
    // so recall gating code has a uniform path.
    pub type_weights: TypeWeights,
}
```

Notes:

- `NonEmptyString` (newtype) guarantees `core_fact` is not empty.
  Constructor returns `Result<NonEmptyString, EmptyCoreFactError>`.
- `Valence(f64)` (newtype) constructor clamps to `[-1.0, 1.0]`. No
  other constructor. Eliminates the repeated `.clamp()` calls scattered
  through `extractor.rs`.
- `Domain` is an enum with variants `Coding | Trading | Research |
  Communication | General | Other(String)`. `Other` captures future
  domains without requiring a code change, but keeps the common cases
  typed.
- `Confidence` is an enum. No string parsing at every read site.
- `tags: BTreeSet<String>` — merge-friendly (union is trivial) and
  naturally deduped.
- `type_weights: TypeWeights` — reuses existing `type_weights.rs`
  struct. Default = all-ones (no type affinity); extractor fills via
  `infer_type_weights()`. Merge rule: per-variant `max` (§5.1) —
  affinity never decays under union.

**Minimal constructor** (for FINDING-4 / extractor-less deployments):

```rust
impl Dimensions {
    /// Build a minimal Dimensions from content only. Used when no
    /// extractor is configured, or during migration of pre-extractor
    /// rows. `core_fact = content`; all narrative dimensions None;
    /// scalars at their defaults (valence 0.0, domain General,
    /// confidence Uncertain, tags empty, type_weights all 1.0).
    ///
    /// This is legitimate low-dimensional memory, not an error state.
    pub fn minimal(content: &str) -> Result<Self, EmptyCoreFactError>;
}
```

### 2.2 `TemporalMark`

```rust
pub enum TemporalMark {
    Exact(DateTime<Utc>),      // "2026-04-22 01:16"
    Day(NaiveDate),            // "today" / "2026-04-22"
    Range { start, end },      // "this week"
    Vague(String),             // "a while back" — never parseable
}

impl TemporalMark {
    /// Precision ordering for merge: Exact > Range > Day > Vague
    pub fn precision_rank(&self) -> u8 { ... }
}
```

Rationale: the extractor already produces temporal strings of varying
precision. Parsing them into a typed mark at extraction time lets
`Dimensions::union()` pick the more precise one without heuristics on
string length.

### 2.3 `EnrichedMemory`

```rust
/// Invariant: every EnrichedMemory has dimensions. There is no
/// constructor that produces one without.
pub struct EnrichedMemory {
    pub content: String,        // redundant with dimensions.core_fact
                                // for search-index ergonomics; kept
                                // in sync by the constructor.
    pub dimensions: Dimensions,
    pub embedding: Option<Embedding>,  // None = computed inline by
                                       // store_enriched. Some = caller
                                       // pre-computed (enables rebuild
                                       // pilot to batch embedding API
                                       // calls — the dominant cost).
    pub importance: Importance, // newtype, clamped [0.0, 1.0]
    pub source: Option<String>,
    pub namespace: Option<String>,
    pub user_metadata: serde_json::Value, // caller-supplied extras,
                                          // never overlaps with
                                          // dimensional namespace
}

impl EnrichedMemory {
    pub fn from_extracted(
        fact: ExtractedFact,
        source: Option<String>,
        namespace: Option<String>,
        user_metadata: serde_json::Value,
    ) -> Result<Self, ConstructionError> { ... }

    /// Build an EnrichedMemory from raw content with minimal dimensions.
    /// Used when no extractor is configured (FINDING-4). This is NOT
    /// the same as extractor failure — failure goes to quarantine.
    pub fn minimal(
        content: &str,
        importance: Importance,
        source: Option<String>,
        namespace: Option<String>,
    ) -> Result<Self, ConstructionError> { ... }
}

/// Batch helper: caller pre-computes embeddings for a slice of
/// EnrichedMemory before handing them to store_enriched. Any item
/// with embedding already Some is skipped.
pub fn precompute_embeddings(
    engine: &EmbeddingEngine,
    items: &mut [EnrichedMemory],
) -> Result<(), EmbeddingError>;
```

This is the **only** type accepted by the primary write path.
`ExtractedFact` remains the wire format the LLM emits; `EnrichedMemory`
is its validated, typed counterpart.

---

## 3. Write-path API (public)

Two entry points, split by semantic intent. No `Option<Metadata>`.

### 3.1 `store_enriched` — caller already has dimensions

```rust
pub fn store_enriched(
    &mut self,
    mem: EnrichedMemory,
) -> Result<StoreOutcome, StoreError> { ... }

pub enum StoreOutcome {
    Inserted  { id: MemoryId },
    Merged    { id: MemoryId, similarity: f32 },
}
```

Used by:
- internal extraction path (after `ExtractedFact` → `EnrichedMemory`),
- external callers that produce their own dimensions (rare, but
  the rebuild pilot hits this when re-ingesting structured archives).

### 3.2 `store_raw` — caller has text only, engram extracts

```rust
pub fn store_raw(
    &mut self,
    content: &str,
    meta: StorageMeta,       // importance hint, source, namespace
) -> Result<RawStoreOutcome, StoreError>;

pub enum RawStoreOutcome {
    /// Extractor produced one or more facts; each one stored/merged.
    Stored(Vec<StoreOutcome>),

    /// Extractor returned empty — content wasn't memory-worthy.
    Skipped { reason: SkipReason, content_hash: ContentHash },

    /// Extractor failed (transient or permanent); content quarantined
    /// for retry. Nothing is written to the main memory table.
    Quarantined { id: QuarantineId, reason: QuarantineReason },
}
```

Dispatch inside `store_raw` (FINDING-4):

| Scenario                            | Action                                         | Outcome           |
|-------------------------------------|------------------------------------------------|-------------------|
| No extractor configured             | `Dimensions::minimal(content)` → `store_enriched` | `Stored(vec![Inserted or Merged])` |
| Extractor present, returns `[]`     | Do nothing                                     | `Skipped`         |
| Extractor present, returns facts    | Each fact → `EnrichedMemory::from_extracted` → `store_enriched` | `Stored(vec![...])` |
| Extractor present, runtime failure  | Write row to `quarantine` table                | `Quarantined`     |

**Key invariant reaffirmed:** rows in `memories` always have a
`Dimensions`. Extractor-less deployments get minimal dimensions (valid).
Extractor-runtime-failure goes to `quarantine` (separate table).
Empty-facts is `Skipped` (nothing written, counter incremented).

Key properties:

- The main `memories` table only ever receives rows with complete
  dimensions. There is no in-band "raw fallback" row.
- `StoreError` is for programmer errors (DB unreachable, invalid
  state). Extractor failure is **not** an error — it's a legitimate
  outcome modeled by `Quarantined`. Callers that want best-effort
  behavior can ignore the variant; callers running a rebuild pilot
  can match on it and drive a retry.

### 3.3 Backward-compatible shims for all three legacy entry points

Public write API today has **three** entry points, all of which must
route through the new path:

- `Memory::add` (memory.rs:1255)
- `Memory::add_to_namespace` (memory.rs:1281)
- `Memory::add_with_emotion` (memory.rs:1711)

And downstream RustClaw currently calls `.add()`, not
`.add_to_namespace()` (rustclaw/src/memory.rs:416, 610).

**Chosen strategy: Strategy A — zero-downtime shim migration.**

All three become `#[deprecated]` shims with unchanged signatures:

```rust
#[deprecated(note = "use store_raw / store_enriched")]
pub fn add(
    &mut self,
    content: &str,
    memory_type: MemoryType,
    importance: Option<f64>,
    source: Option<&str>,
    metadata: Option<serde_json::Value>,
) -> Result<String, ...> {
    self.add_to_namespace(content, memory_type, importance, source, metadata, None)
}

#[deprecated(note = "use store_raw / store_enriched")]
pub fn add_to_namespace(
    &mut self,
    content: &str,
    memory_type: MemoryType,
    importance: Option<f64>,
    source: Option<&str>,
    metadata: Option<serde_json::Value>,
    namespace: Option<&str>,
) -> Result<String, ...> {
    let meta = StorageMeta {
        importance_hint: importance,
        source: source.map(str::to_string),
        namespace: namespace.map(str::to_string),
        user_metadata: metadata.unwrap_or(serde_json::Value::Null),
        // memory_type hint is threaded into ExtractedFact construction
        // when extractor-less fallback kicks in, preserving today's
        // explicit-type behavior.
        memory_type_hint: Some(memory_type),
    };
    match self.store_raw(content, meta)? {
        RawStoreOutcome::Stored(outcomes) => {
            // Legacy callers expect a single String id. Return the
            // first outcome's id; log a warning if N > 1 (extractor
            // produced multi-fact from what legacy thought was one).
            Ok(outcomes.first().map(StoreOutcome::id).unwrap_or_default())
        }
        RawStoreOutcome::Skipped { .. } => {
            // Legacy returned a dummy id on skip; we preserve that
            // behavior via a sentinel "skipped:<hash>" id so callers
            // don't break, with a structured warn-log.
            Ok(format!("skipped:{}", hash))
        }
        RawStoreOutcome::Quarantined { id, reason } => {
            // Legacy had no quarantine concept. Surface as Err so
            // silent success doesn't hide the regression from callers.
            Err(StoreError::Quarantined { id, reason })
        }
    }
}

#[deprecated(note = "use store_raw / store_enriched + record_emotion")]
pub fn add_with_emotion(...) -> Result<String, ...> {
    let id = self.add_to_namespace(...)?;
    self.record_emotion(&id, ...)?;
    Ok(id)
}
```

**Private:** `add_raw` (memory.rs:1390 trail) becomes private, wrapped
by `store_enriched` as its final DB step. Never called externally.

**Integration test added (Step 4.5, §11):** a test harness boots a
real `engramai::Memory` and replays RustClaw's three call sites
against the shims to prove: (a) compile still works, (b) return type
unchanged, (c) stored row now has v2 metadata with dimensions
(minimal, since test has no extractor configured).

**Deprecation timeline:** shims ship in engramai v0.2.next, marked
deprecated. Removed no earlier than v0.4 (one minor release of
grace). RustClaw migrates to `store_raw` / `store_enriched` at its
own pace; both crates stay green throughout.

---

## 4. Quarantine path

New table:

```sql
CREATE TABLE quarantine (
    id              TEXT PRIMARY KEY,
    content         TEXT NOT NULL,
    content_hash    TEXT NOT NULL,       -- dedup within quarantine
    received_at     REAL NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    last_attempt_at REAL,
    source          TEXT,
    namespace       TEXT,
    importance_hint REAL,
    user_metadata   TEXT                 -- JSON
);
CREATE INDEX idx_quarantine_hash ON quarantine(content_hash);
```

**Retry API:**

```rust
pub fn retry_quarantined(
    &mut self,
    max_items: usize,
) -> Result<RetryReport, StoreError>;

pub struct RetryReport {
    pub attempted: usize,
    pub recovered: Vec<MemoryId>,    // now in main table
    pub still_failing: Vec<(QuarantineId, String)>, // id + error
    pub permanently_rejected: Vec<QuarantineId>,    // attempts >= N
}
```

Policy:

- First attempt lives inline with `store_raw` (synchronous, same LLM
  call the caller expected). Failure writes to quarantine.
- `retry_quarantined` is a separate operation, owned by the caller
  (e.g., a background job in the rebuild pilot), not by `store_raw`.
  This keeps `store_raw` latency-bounded.
- After `max_attempts = 5` (configurable), the record is marked
  `permanently_rejected` and stays in quarantine for forensic review.
  It never gets deleted by engram.

Why a separate table, not a flag on `memories`:

- Keeps the main table's invariant clean ("every row has dimensions").
- Retrying a quarantined record may produce multiple facts in the main
  table — 1-to-N relationship is more natural across two tables.
- FTS/vector indexes on `memories` stay focused on real memories.

---

## 5. Merge semantics (Leak 3 root fix)

### 5.1 Responsibility shift

`merge_memory_into` currently holds merge logic for content, importance,
history — it's a god-function. Move dimensional merging to `Dimensions`
itself:

```rust
impl Dimensions {
    /// Union two signatures. Never loses information.
    ///
    /// Strategy per field:
    ///   core_fact      — longer wins (proxy for fuller extraction)
    ///   participants   — set-union of comma-separated names
    ///   temporal       — higher precision_rank wins
    ///   location       — longer wins
    ///   context        — longer wins
    ///   causation      — longer wins
    ///   outcome        — longer wins
    ///   method         — longer wins
    ///   relations      — set-union (semicolon-separated)
    ///   sentiment      — existing wins if present, else incoming
    ///                    (speaker-specific; non-commutative — see §5.3)
    ///   stance         — same as sentiment
    ///   valence        — importance-weighted average
    ///   domain         — see domain rule below
    ///   confidence     — min (most cautious)
    ///   tags           — set-union
    ///   type_weights   — per-variant max (affinity never decays)
    ///
    /// Domain merge rule (FINDING-6):
    ///   1. Concrete variant beats Other(_) regardless of side.
    ///   2. Two concretes: existing wins (stable).
    ///   3. Two Other(_): longer string wins; tie → existing.
    pub fn union(self, other: Dimensions, weights: MergeWeights) -> Dimensions;
}

pub struct MergeWeights {
    pub existing_importance: f64,
    pub incoming_importance: f64,
}
```

**Invariant:** for every field `f`, if either input has a populated
value, the output has a populated value. Information is monotone under
`union`.

### 5.2 `merge_memory_into` new signature

```rust
pub fn merge_memory_into(
    &mut self,
    existing_id: &MemoryId,
    incoming: &EnrichedMemory,
    similarity: f32,
) -> Result<MergeOutcome, rusqlite::Error>;
```

Steps:

1. Fetch existing row as `EnrichedMemory`.
2. `merged = existing.dimensions.union(incoming.dimensions, weights)`.
3. `merged_content = pick_content(existing.content, incoming.content)`
   (same "longer wins" rule, kept in sync with `core_fact`).
4. `merged_importance = max(existing.importance, incoming.importance)`.
5. Single `UPDATE memories SET content=?, metadata=?, importance=?`.
6. Append to `merge_history` (existing behavior, unchanged).

### 5.3 Algebraic properties

Running the 58MB rebuild N times must produce the same end state as
running it once (modulo `merge_history` growing). Proven by:

- `Dimensions::union` is **associative**:
  `(a.u(b)).u(c) == a.u(b.u(c))`.
- `Dimensions::union` is **idempotent**: `a.u(a) == a`.
- `Dimensions::union` is **monotone** (information-only-grows):
  `info_content(a.u(b)) ≥ max(info_content(a), info_content(b))`.
- `max` on importance is idempotent + associative.
- Content "longer wins" is idempotent + associative.

**Non-commutative by design (FINDING-7).**
`a.union(b)` assigns `a` the "existing" role and `b` the "incoming"
role. Fields where speaker identity matters (`sentiment`, `stance`)
keep the existing side when both are populated. Therefore:

- ✅ `a.union(a) == a` — **asserted** (proptest)
- ✅ `(a.union(b)).union(c) == a.union(b.union(c))` — **asserted**
- ✅ `info_content(a.union(b)) ≥ info_content(a)` — **asserted**
- ❌ `a.union(b) == b.union(a)` — **NOT** asserted (would fail and
  shouldn't be a property of this operation).

Tests in Step 3 of §11 use `proptest` with three asserted properties
above, plus targeted unit tests for non-commutative cases to pin the
"existing wins" semantics against regression.

---

## 6. Schema migration

On-disk format of `metadata` JSON blob:

### Before

```json
{
  "merge_count": 3,
  "merge_history": [...],
  "dimensions": {                  // sometimes missing
    "participants": "...",
    "valence": 0.3,
    ...
  },
  "type_weights": {...}            // sometimes missing
}
```

### After

```json
{
  "engram": {
    "version": 2,
    "dimensions": {                // ALWAYS present for rows in main table
      "core_fact": "...",
      "participants": "...",
      "temporal": {"kind": "day", "value": "2026-04-22"},
      "valence": 0.3,
      "domain": "coding",
      "confidence": "likely",
      "tags": ["..."],
      "type_weights": {"episodic": 1.0, "factual": 1.2, "procedural": 0.8, ...},
      ...
    },
    "merge_count": 3,
    "merge_history": [...]
  },
  "user": { ... caller-supplied ... }
}
```

Key points:

- Namespaced under `engram.*` so user metadata cannot collide with
  engram internals. Answers Open Question #1 from `investigation.md`.
- `version: 2` for future migration.
- Rows without `engram.dimensions` are `version: 1` legacy rows.

### Migration strategy

Classification (FINDING-5):

```rust
enum LegacyClassification {
    /// v1 row carries enough to rebuild Dimensions losslessly.
    /// Criteria: dimensions field present AND has at least
    /// {participants, temporal, causation} ∪ {domain, valence}.
    CleanUpgrade,

    /// v1 row has partial dimensions but is missing ≥1 of
    /// {participants, temporal, causation} AND content length > 40
    /// chars (short content is OK to stay minimal; long content
    /// without these dimensions is suspicious and merits re-extraction).
    NeedsBackfill { missing: Vec<&'static str> },

    /// v1 row has no `dimensions` field at all; pre-extractor era.
    PreExtraction,
}
```

Migration rules:

- **CleanUpgrade** → deserialize into `Dimensions`, rewrite metadata
  blob as v2 layout, set `version: 2`. Done, single row UPDATE.
- **NeedsBackfill** → build partial `Dimensions` from whatever v1
  fields exist (others default), rewrite as v2, **keep the row in
  `memories`**, and enqueue in `backfill_queue`. The row is fully
  valid (main-table invariant holds: dimensions present) — we just
  know it's lower quality and want to improve it when budget allows.
- **PreExtraction** → `Dimensions::minimal(content)` + enqueue in
  `backfill_queue` with `reason = PreExtraction`.

New table:

```sql
CREATE TABLE backfill_queue (
    memory_id    TEXT PRIMARY KEY,
    enqueued_at  REAL NOT NULL,
    reason       TEXT NOT NULL,         -- serialized LegacyClassification
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_attempt_at REAL,
    last_error   TEXT
);
CREATE INDEX idx_backfill_enqueued ON backfill_queue(enqueued_at);
```

Backfill API:

```rust
pub fn backfill_dimensions(
    &mut self,
    max_items: usize,
) -> Result<BackfillReport, StoreError>;

pub struct BackfillReport {
    pub attempted: usize,
    pub upgraded:  Vec<MemoryId>,           // dimensions meaningfully enriched
    pub unchanged: Vec<MemoryId>,           // extractor returned same or worse
    pub failed:    Vec<(MemoryId, String)>, // re-enqueued for retry
}
```

Backfill job reuses the merge machinery: load row → re-run extractor
→ build incoming `EnrichedMemory` → `merge_memory_into(existing, incoming, 1.0)`.
Same invariants (idempotent / monotone), same code path. Information
only grows; re-running backfill is safe.

**No eager migration.** Touching a v1 row via the normal write path
rewrites it to v2 as a side-effect (shim path triggers read-modify-write).
The `backfill_dimensions` job is an **explicit** operation — never
automatic, never silent. The 58MB rebuild pilot runs it as a named
phase and reports `BackfillReport`.

---

## 7. Wire-format → typed-value boundary

`ExtractedFact` (extractor output, JSON-friendly, loose) is kept as the
extractor's public surface. It crosses **exactly one type boundary**:

```
LLM → JSON → ExtractedFact → EnrichedMemory::from_extracted → …
                              ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                              validation happens here, once
```

After that boundary, no code ever deals with `Option<serde_json::Value>`
for dimensional data. Callers that want to introspect use `Dimensions`
accessors.

This is the elegance payoff: the loose type exists only at the edge,
and the system is typed everywhere else.

---

## 8. Telemetry (Leak 2 root fix)

`store_raw` emits a structured event for every call:

```rust
pub enum StoreEvent {
    Stored       { id, fact_count, ms_elapsed },
    Skipped      { content_hash, reason, ms_elapsed },
    Quarantined  { id, error_kind, ms_elapsed },
}
```

Events go through a lightweight `EventSink` trait. Default impl
increments in-memory counters exposed via `MemoryManager::stats()`:

```rust
pub struct WriteStats {
    pub stored_count:      u64,
    pub skipped_count:     u64,
    pub quarantined_count: u64,
    pub merged_count:      u64,
    pub skipped_by_reason: HashMap<SkipReason, u64>,
}
```

Rebuild pilot can then assert coverage thresholds directly from stats,
instead of SQL-scraping the DB afterwards.

---

## 9. Open questions → resolved

### Q1 (from investigation.md): where does `extractor_status` live?

**Resolved:** not needed. Failed extractions don't live in the main
table at all. They live in `quarantine`. The table itself is the
status signal.

### Q2 (from investigation.md): same-key merge tie-breaker?

**Resolved:** per-field strategy on `Dimensions::union()`. No global
"longer wins" rule. See §5.1.

---

## 10. Risks and mitigations

### R1 — `EnrichedMemory` is too strict; some legitimate inputs can't construct one

Mitigation: `from_extracted` returns `Result`, not `Option`. The one
known reason to fail is empty `core_fact`. Anything else we discover
during migration becomes a new `ConstructionError` variant with a
specific message, not a `panic!`.

### R2 — Quarantine table grows unbounded

Mitigation: quarantine has a TTL (default 30d); `retry_quarantined`
skips expired rows. A CLI command `engram quarantine purge` removes
`permanently_rejected` rows older than TTL. Deletion is never
automatic — honors the "never delete data without permission" rule.

### R3 — v1 → v2 migration misses some rows

Mitigation: `dimensional_backfill` is explicit (a method on
`MemoryManager`), not automatic. Pilot runs it as a named step and
gets a report of what converted / what couldn't.

### R4 — API churn breaks downstream (RustClaw)

Mitigation (FINDING-2): **all three** legacy public entry points
(`add`, `add_to_namespace`, `add_with_emotion`) kept as
`#[deprecated]` shims that forward to `store_raw`. Signatures and
return types unchanged. A dedicated integration test (Step 4.5)
replays RustClaw's actual call sites to guarantee no silent break.
Shims live until v0.4 minimum — RustClaw migrates to
`store_raw` / `store_enriched` at its own pace.

---

## 11. Implementation plan (ordered)

Each step leaves the tree compilable and tests green.

**Step 1 — Types (no behavior change).**
Create `src/dimensions.rs` with `Dimensions`, `Valence`, `Domain`,
`Confidence`, `TemporalMark`, `NonEmptyString`. No callers yet.

**Step 2 — `EnrichedMemory` + construction from `ExtractedFact`.**
Create `src/enriched.rs`. Unit tests for `from_extracted` round-trip
and failure modes.

**Step 3 — `Dimensions::union` with proptest coverage.**
Property tests for idempotence, associativity, monotonicity (exactly
the three asserted in §5.3). Targeted unit tests for non-commutative
cases (`sentiment` / `stance` "existing wins"). Targeted unit tests
for `Domain` merge rule (concrete-beats-Other, Other-vs-Other longer
wins). This is the highest-risk piece of logic; pin it down first.

**Step 4 — New write API in `memory.rs`.**
Add `store_enriched`, `store_raw`, `StoreOutcome`, `RawStoreOutcome`.
Route internal callers to the new API where the caller already has
`Dimensions`. Legacy entry points (`add`, `add_to_namespace`,
`add_with_emotion`) still call their legacy implementations — not
yet shimmed.

**Step 4.5 — Shim the three legacy entry points (FINDING-2).**
`add`, `add_to_namespace`, `add_with_emotion` marked `#[deprecated]`
and forwarded to `store_raw`. Add integration test that boots a real
`engramai::Memory` and replays RustClaw's call sites against shims —
verifies return type, stored row, metadata v2 with minimal dimensions
in extractor-less test env.

**Step 5 — Introduce `merge_enriched_into` (coexist with legacy).**
Add new signature `merge_enriched_into(&EnrichedMemory)` alongside
the existing `merge_memory_into(&str, ...)`. Both coexist. Shim layer
still calls legacy for now. Zero breakage.

**Step 5.5 — Migrate internal callers to new merge signature (FINDING-8).**
Every internal call site of `merge_memory_into` (currently
memory.rs:714, 1478, 1517) rewritten to build an `EnrichedMemory`
and call `merge_enriched_into`. Shim updated to the new path. Legacy
`merge_memory_into` now has zero internal callers; `grep` verifies.

**Step 5.9 — Delete legacy `merge_memory_into`; rename new.**
After `grep` confirms no internal callers, delete the legacy overload.
Rename `merge_enriched_into` → `merge_memory_into` as the canonical
name. This is the only intentional breaking rename in the plan; it
happens in the same PR as the delete to avoid dangling references.

**Step 6 — Quarantine table + retry API.**
New migration. Integration test: induce extractor failure, confirm
row lands in quarantine, retry recovers it.

**Step 7 — v2 metadata layout + read-path backward compat.**
`Dimensions::from_stored_metadata` handles v1 and v2.

**Step 8 — WriteStats telemetry.**
Counters exposed, old `info!` logs demoted to `debug!` where
redundant.

**Step 9 — 5KB smoke rebuild.**
Run pilot on one day of agent session data. Assertions:
- `stats.stored_count / (stored + skipped + quarantined) > 0.95`
- dimensional field coverage on stored rows:
  `participants > 60%`, `temporal > 40%`, `causation > 30%`
- zero rows in `memories` with missing `engram.dimensions`.

**Step 10 — 58MB full rebuild.**
Only after step 9 green.

---

## 12. Non-goals

- Not changing extractor prompt or `ExtractedFact` wire format.
- Not changing dedup similarity threshold.
- Not changing the `source_text` storage policy (out per the comment
  at `memory.rs:1354`).
- Not adding automatic LLM retry inside `store_raw`. Retry policy
  belongs to the pilot, not the storage layer.
- **Not requiring an extractor for basic storage** (FINDING-4):
  extractor-less deployments store minimal `Dimensions` and are
  first-class, not error cases.
- Not deleting any data. Ever.

---

## 13. Summary

The current design embeds an `Option`-shaped hole where a `Dimensions`
should be. Every leak in ISS-019 exploits that hole. Closing it means:

1. `Dimensions` is a strongly-typed value, not JSON — including the
   `type_weights` field, so no dimensional output is left behind.
2. Write path is split by intent (`store_enriched` / `store_raw`),
   not by optional parameters. Extractor-less deployments use
   `Dimensions::minimal`, not quarantine.
3. Failed extractions go to `quarantine`, not to main table as
   metadata-less rows.
4. Legacy v1 rows migrate via a typed `LegacyClassification` and
   (when needed) an explicit `backfill_dimensions` job — never
   automatic, never silent.
5. Merge delegates dimensional logic to `Dimensions::union`, whose
   invariants (idempotent, associative, monotone; **non-commutative
   by design**) are machine-verified.
6. Metadata layout is versioned and namespaced — `engram.*` vs
   `user.*` — closing the collision question once and for all.
7. All three public legacy entry points (`add`, `add_to_namespace`,
   `add_with_emotion`) are shimmed with unchanged signatures; a
   dedicated RustClaw-shape integration test guards the interface.

The result is a write path where the bug we're fixing cannot
re-occur, because the compiler refuses to construct it. That is the
root fix.
