# ISS-024 Design: Dimensional Metadata Pipeline — CLI Surface, Temporal Consumption, Contract

**Status:** draft (revision r3 — pending review)
**Investigation:** `investigation.md` (same folder)
**Scope of this document:** Changes 1, 2, 3a, 4 (Changes 3b deferred to ISS-020 Phase B)
**Tasks unblocked by this design:** `iss-024-cli-meta`, `iss-024-adapter-rc`, `iss-024-temporal-dim`, `iss-024-contract-test`

---

## Revision Notes

### r3, 2026-04-22

Patch revision addressing the two findings from `reviews/design-r2.md`:

1. **FINDING-r2-1 (§5.3):** Replaced `and_hms_opt(0, 0, 0)?.and_utc()` pattern with `and_time(chrono::NaiveTime::MIN).and_utc()` in both the `Range` and `Day` match arms. The `?` operator cannot be used in a function returning `f64`; `NaiveTime::MIN` is a const, infallible constructor and avoids the type mismatch. Added `NaiveTime` to the §5.3 snippet's `use chrono::{...}` import line. The `Day` arm had the same bug pre-r2 (unchanged from r1); both are fixed together.
2. **FINDING-r2-2 (§9.2):** Added full YAML node definition for `iss-024-temporal-roundtrip` alongside the existing `iss-024-extractor-stub` block, plus explicit edges list. Priority 40 (higher than extractor-stub's 45) because this task blocks every other iss-024 implementation task.

No structural or semantic changes — both fixes are local to §5.3 (three lines + one import) and §9.2 (one new YAML block + four edges). r1 deferred findings (4–13) remain deferred.

### r2, 2026-04-22

Addresses the three critical findings from `reviews/design-r1.md`:

1. **FINDING-1 (§1.1, §5.3):** `TemporalMark::Range` corrected to `{ start: NaiveDate, end: NaiveDate }` (matches live code at `src/dimensions.rs:226`). §5.3 match arm rewritten to expand NaiveDate endpoints to a UTC `TimeRange` via `and_hms_opt(0,0,0).and_utc()` plus a `+1 day` offset on the end. Note that Range is thus day-granular, not arbitrary DateTime-granular.
2. **FINDING-2 (§6.1):** Serialized `temporal` JSON shape corrected — real on-disk format is internally-tagged `{"kind": "...", "value": ...}` per `#[serde(tag = "kind", content = "value")]` on `src/dimensions.rs:222`. Added explicit shape table for all four variants.
3. **FINDING-3 (§1.1a, §6.2, §9):** Pre-existing v2 roundtrip bug documented and scoped into this issue. `from_legacy_metadata` only recovered `Vague` (reads `temporal` as a string); all other variants silently dropped on read. New subtask `iss-024-temporal-roundtrip` added as a prep step blocking every other iss-024 task. Design chose option 1 (fix read path inline) over option 2 (scope down to Vague-only) because the design's value proposition depends on `Exact/Range/Day` actually roundtripping.

Important and minor findings (FINDING-4 through FINDING-13) are deferred — not addressed in r2. A follow-up revision may pick them up after implementation starts.

### r1, 2026-04-22

**What changed and why.** The r0 draft was written against a presumed schema ("flat `metadata` with top-level `dia_id` and a sibling `dimensions` object, where `dimensions.temporal` is a `Vec<String>` of natural-language phrases"). Direct inspection of `src/memory.rs::build_legacy_metadata` (ISS-019 Step 7a v2) and `src/dimensions.rs::Dimensions` revealed two structural mismatches. r1 rewrites the affected sections:

1. **Actual storage is v2 nested, not flat.** `MemoryRecord.metadata` is:
   ```json
   {
     "engram": { "version": 2, "dimensions": <typed Dimensions serialization>, "merge_count": 0, "merge_history": [] },
     "user":   { "dia_id": "D1:3", "speaker": "Alice", ... }
   }
   ```
   User `--meta` pairs land under `metadata.user.*`, not at the top level. The extractor-owned signature lives under `metadata.engram.dimensions.*`. The reserved-key set (§3) is now defined over `user` keys only — `engram` is a reserved *namespace*, not a reserved top-level key.

2. **`Dimensions::temporal` is typed, not a string array.** The real field is `Option<TemporalMark>` where `TemporalMark ∈ {Exact(DateTime), Range{start, end}, Day(NaiveDate), Vague(String)}`. **Three of the four variants are already a parsed time** — no `two_timer` needed for those. `two_timer` is invoked only for the `Vague(String)` fallback. This collapses §1 and §5.2 substantially: the `DimensionView` accessor becomes a thin `Option<&TemporalMark>` getter, and `DimParseCache` only caches `Vague` strings.

**Sections affected:** §1 (time parsing reshaped around `TemporalMark`), §3 (reserved keys scoped to `user.*`), §4.4 (contract test assertions repathed to `metadata.user.*`), §5.2–5.3 (`DimensionView` rewritten), §6.1 (data-flow diagram repainted with v2 layout). Earlier sections that only describe flow or LOC budgets are unchanged; those budgets shift slightly and are restated in §6.2.

**Sections NOT affected:** §2 (adapter call-sites — purely about bytes coming out of `recall`, layout-agnostic), §4.1–4.3 (CliHarness mechanics), §7 (guards — all invariants hold under v2 unchanged), §8 (acceptance criteria — restated in §4.4 wording but semantically identical).

---

## 0. Document Map

This design answers the five underspecified areas that the investigation deferred:

| § | Area | Blocks task |
|---|---|---|
| §1 | Natural-language time parsing (Change 3a substrate) | iss-024-temporal-dim |
| §2 | Adapter call-site inventory (Change 2 scope) | iss-024-adapter-rc |
| §3 | Reserved-namespace semantics (Change 1 behavior, AC-2) | iss-024-cli-meta |
| §4 | Contract test framework (Change 4 design) | iss-024-contract-test |
| §5 | 3a→3b forward-compat interface | iss-024-temporal-dim |

§6–9 cover architecture/interfaces/guards/acceptance for the whole issue.

---

## 1. Natural-Language Time Parsing (Change 3a substrate)

### 1.1 Requirement

`temporal_score` must consume the extractor-produced temporal signature stored at `record.metadata.engram.dimensions.temporal` and fold it into the existing `created_at`-based score. The *stored* shape is typed, not string:

```rust
// src/dimensions.rs
pub enum TemporalMark {
    Exact(DateTime<Utc>),                       // already a point (UTC)
    Range { start: NaiveDate, end: NaiveDate }, // already a day-range (inclusive NaiveDate endpoints)
    Day(NaiveDate),                              // already a calendar day
    Vague(String),                               // free-form phrase, e.g. "last summer"
}

pub struct Dimensions {
    // ...
    pub temporal: Option<TemporalMark>,
    // ...
}
```

Consequences:

- **Three of the four variants need zero parsing.** `Exact` maps directly; `Range` and `Day` are calendar-day granular and expand to `[start 00:00 UTC, end+1 00:00 UTC)` and `[day 00:00 UTC, day+1 00:00 UTC)` respectively. Note that `Range`'s endpoints are `NaiveDate` (not `DateTime`), so `Range` is essentially a multi-day `Day` — it does not carry sub-day precision.
- **`Vague(String)` is the only case that needs `two_timer`.** Its string payload (e.g. `"last summer"`, `"five minutes before midnight"`) is what §1.2 parses, anchored to `record.created_at` per §1.5.
- `Option<TemporalMark>` not `Vec<_>` — there is at most one temporal mark per memory (extractor chooses the most specific). No "best-of-N phrases" loop; the math in §5.3 simplifies to a single optional score.
- `Dimensions::from_stored_metadata` (already implemented) handles both v1 and v2 JSON layouts and returns a typed `Dimensions`. The read path will use that helper rather than hand-rolling a JSON walker.

### 1.1a Read-Path Roundtrip Gap (pre-existing bug, fixed as part of this issue)

The current `Dimensions::from_legacy_metadata` (`src/dimensions.rs:432`) reads the
`temporal` field as a JSON string:

```rust
let temporal = get_string("temporal").map(|s| {
    crate::enriched::parse_temporal_mark(&s)
});
```

However, `build_legacy_metadata` (`src/memory.rs`) serializes the full typed
`Dimensions` via serde, so `Exact`/`Range`/`Day` variants are written as JSON
objects (`{"kind": "...", "value": ...}`), not strings. On read-back,
`get_string` returns `None` for objects, and the non-`Vague` variants are
silently dropped.

Consequence: without fixing this, §5.3's match arms for `Exact`, `Range`, and
`Day` are dead code for any record that survives a write → read cycle. The
design's value proposition (consuming typed extractor signatures) would not
materialize.

**Fix, scoped to this issue.** Extend `from_legacy_metadata` to accept both
shapes for `temporal`:

```rust
let temporal = raw_obj.get("temporal").and_then(|v| match v {
    // Legacy path: string payload, parse via natural-language fallback.
    serde_json::Value::String(s) => crate::enriched::parse_temporal_mark(s),
    // New path: typed object, deserialize directly via serde.
    serde_json::Value::Object(_) => serde_json::from_value::<TemporalMark>(v.clone()).ok(),
    _ => None,
});
```

Add a roundtrip regression test in `src/dimensions.rs` (tests module):
populate `Dimensions { temporal: Some(TemporalMark::Range { start, end }), .. }`,
serialize via `build_legacy_metadata`, parse via `from_stored_metadata`,
assert the `Range` variant is preserved bit-for-bit. Repeat for `Exact` and
`Day`. The `Vague` case is already covered by existing tests.

This fix is small (~15 LOC in `dimensions.rs` + ~30 LOC of tests) but
essential — without it, §5.3's three non-Vague branches are unreachable.

### 1.2 Library Choice — `two_timer` v2.2.5

Decision: **use `two_timer` v2.2.5** (MIT, 1.4 KLoC, actively maintained, on docs.rs).

**Rationale (first-principles):**

- The problem is "parse English time expression → `(NaiveDateTime, NaiveDateTime)` range". That's exactly `two_timer::parse()`'s signature: `parse(phrase: &str, config: Option<Config>) -> Result<(NaiveDateTime, NaiveDateTime, bool), TimeError>`.
- Handles the investigation's example inputs natively: `"yesterday"`, `"last Friday"`, `"last year"`, `"at 3pm today"`, `"May 6 1968"`, `"Friday the 13th"`, relative phrases like `"five minutes before midnight"`.
- Supports `Config::now(NaiveDateTime)` — critical: the "now" reference for parsing **must** be `record.created_at`, not wall-clock `Utc::now()`. A memory stored on 2024-01-10 saying "yesterday afternoon" means 2024-01-09 afternoon, not "yesterday relative to query time".
- Returns a range, not a point — matches our `TimeRange { start, end }` shape.
- Pure Rust, dep surface: `chrono ^0.4` (already in Cargo.toml), `lazy_static`, `pidgin`, `regex`, `serde_json`. No new C libs.
- License compatible (MIT; engram is MIT/Apache-2.0 dual).

**Alternatives considered and rejected:**

- `chrono-english` — only produces a single `DateTime`, not a range. "Last Tuesday" returns a point, not the whole Tuesday. Would require re-deriving ranges, which is the hard part.
- `parse_datetime` — modeled on GNU `date`, good for ISO-ish strings but weak on English phrases ("yesterday afternoon" returns an error).
- Roll our own — the fallback if a dep is rejected, but first-principles: we don't have a reason to reinvent. The LoCoMo queries are fundamentally English temporal phrases.
- LLM parse — too slow and non-deterministic for a per-candidate scoring hot path.

### 1.3 API — `TemporalDimParser`

New module: `src/temporal_dim.rs` (~120 LOC + tests).

```rust
use chrono::{DateTime, Utc, NaiveDateTime};
use crate::query_classifier::TimeRange;

/// Parse a natural-language time expression against a reference point.
///
/// `phrase`: The extractor-produced string (e.g. "yesterday afternoon").
/// `reference`: The anchor for relative expressions. MUST be the memory's
///              `created_at`, not wall-clock now, so "yesterday" means
///              "the day before this memory was stored".
///
/// Returns None if the phrase is unparseable (common for vague extractor output
/// like "recently" or "a while ago"). None is a signal to fall back to
/// insertion-time scoring; it is not an error.
pub fn parse_dimension_time(
    phrase: &str,
    reference: DateTime<Utc>,
) -> Option<TimeRange>;

/// Cache entry for dimension parsing results.
/// Keyed by (phrase, reference_date) — rounded to day granularity because
/// "yesterday" parsed against 2024-01-10T10:00Z vs 2024-01-10T14:00Z gives
/// the same answer (same day range).
pub struct DimParseCache {
    // LRU, default capacity 4096. Sized for the LoCoMo haystack (~1K candidates
    // — only `Vague(_)` marks enter the cache, and most recur across queries).
    inner: lru::LruCache<(String, chrono::NaiveDate), Option<TimeRange>>,
}

impl DimParseCache {
    pub fn new(capacity: usize) -> Self;
    pub fn get_or_parse(
        &mut self,
        phrase: &str,
        reference: DateTime<Utc>,
    ) -> Option<TimeRange>;
}
```

### 1.4 Parse Failure Semantics

`two_timer::parse` returns `Err(TimeError)` for non-temporal strings. We treat errors as `None`:

- **Never panic.** Extractor output is untrusted (LLM can produce garbage).
- **Never error the recall.** A bad phrase is not an engram bug.
- **Log at DEBUG once per unique phrase** (to avoid log flooding during bench runs):
  ```
  DEBUG: dimension.temporal parse miss: "a while ago" (no time range extracted)
  ```

The cache stores `None` for unparseable phrases so we don't re-parse `"maybe"` ten thousand times.

### 1.5 Reference-Point Policy

- `reference = record.created_at` — **this is the critical decision**.
- Rationale: the extractor produced the phrase while reading the stored content; the temporal deixis is anchored to the storage moment, not the query moment.
- Edge case: if a memory says "next year" and was stored in 2024, that means 2025. If queried in 2026 with the question "what happened in 2025", we still want the match. Anchoring to `created_at` preserves this.

### 1.6 Performance

- **Only `Vague(_)` hits `two_timer`.** `Exact`/`Range`/`Day` are O(1) typed conversions. In practice the extractor produces `Day` or `Range` for anything it can resolve; `Vague` is the fallback.
- Per call (`Vague` only): `two_timer::parse` is ~50–200µs for typical phrases.
- Per recall: at most one temporal mark per candidate, so N parses = count(candidates with `Vague` temporal) ≤ N_candidates. With cache, steady-state is ~0 parses (same phrases recur across repeated runs, and non-`Vague` variants never enter the cache).
- Cache size: 4096 entries × (avg 30 B key + 64 B value) ≈ 400 KB. Trivial.
- **Budget: <2 ms added to the scoring phase in steady state.** If profiling shows >5 ms, bump cache size or memoize at the `MemoryRecord` level.

### 1.7 Tests (unit-level, live in `src/temporal_dim.rs`)

- `test_parse_yesterday_relative_to_reference` — "yesterday" anchored at 2024-01-10 returns 2024-01-09 00:00–24:00.
- `test_parse_absolute_date` — "May 6, 1968" returns the correct 24h range.
- `test_parse_compound_phrase` — "last Tuesday afternoon" returns a bounded afternoon range.
- `test_parse_unparseable_returns_none` — "recently", "a while ago", empty string, random noise all return `None` without error.
- `test_cache_idempotency` — parsing same `(phrase, day)` twice hits the cache.
- `test_timezone_alignment` — UTC is canonical; test that `created_at` in UTC produces correct UTC range.

### 1.8 Fallback Strategy

If the library or dimension is unavailable at call time, behavior MUST match the current `temporal_score`. No regression. The `max(insertion_time_score, dimension_time_score)` rule in §6 ensures this: when `dimension_time_score = 0.0` (no dimension or parse failure), the output equals `insertion_time_score` exactly.

---

## 2. Adapter Call-Site Inventory (Change 2 scope)

### 2.1 Method

Ecosystem grep across:
- `/Users/potato/clawd/projects/cogmembench` (benchmark adapters)
- `/Users/potato/clawd/projects/engram-ai-rust` (engram itself)
- `/Users/potato/clawd/projects/agent-memory-prototype` (legacy + new benches)
- `/Users/potato/rustclaw` (the agent framework)

Search patterns: `engram store`, `engram_cli`, `subprocess.*engram`, `Popen.*engram`, `engram-rs` binary invocation.

### 2.2 Complete Inventory

**Three real CLI call sites:**

| # | Path | Status | Notes |
|---|---|---|---|
| 1 | `cogmembench/benchmarks/locomo/engram_adapter.py` | **Has `--meta` code + rc-check with latent bug** | Investigation's Gap A victim |
| 2 | `cogmembench/benchmarks/longmemeval/engram_adapter.py` | No `--meta`; rc-check with same latent bug | Does not pass dimensions at all |
| 3 | `cogmembench/benchmarks/common/` (shared helpers) | Only defines `_get_oauth_token`; no `store` callers of its own | No action needed |

**Non-CLI callers (use engram as a Rust/Python lib, not via CLI) — OUT OF SCOPE for Change 2:**

- RustClaw (`src/memory.rs`) — imports `engramai` as a Rust crate directly. Calls `Memory::remember(...)` with `metadata: serde_json::Value`. No shell boundary; no `--meta` flag involved. Does not need rc-check.
- `agent-memory-prototype/benchmarks/*.py` — imports `from engram import Memory` (Python legacy). Pre-dates the Rust CLI; in-process. Not affected.
- `agent-memory-prototype/engram/mcp_server.py` — uses `mem._store` internal API. Not affected.
- MemoryAgentBench — no engram adapter exists; only Mem0/Zep/etc.

### 2.3 Per-Call-Site Work

#### Call site 1: `cogmembench/benchmarks/locomo/engram_adapter.py`

**Current state** (file lines, verified 2026-04-22):

- L85–93: `_run` helper with latent-buggy rc check:
  ```python
  if result.returncode != 0:
      stderr = result.stderr[:500] if result.stderr else ""
      # Don't raise on warnings from INFO logs
      if "error" in stderr.lower() and "info" not in stderr.lower():
          raise RuntimeError(f"engram failed: ...")
  ```
  **Bug**: `"info" not in stderr.lower()` is the silent-fail mechanism. Any stderr that contains BOTH an error AND an `[INFO]` log line (very common — our tracing emits INFO at startup) short-circuits and returns normally with `rc != 0`. This is why `--meta` rejection slipped through for months.

- L136–146: Store already builds `--meta` args. Serializes `str` values raw; non-string values via `json.dumps`.

**Required changes:**

1. **Fix rc-check logic** — replace the `"info" not in stderr.lower()` short-circuit with an explicit rc check:
   ```python
   if result.returncode != 0:
       cmd_summary = " ".join(args[:3])
       meta_args = [a for a in args if a == "--meta" or (args.index(a) > 0 and args[args.index(a)-1] == "--meta")]
       raise RuntimeError(
           f"engram CLI rc={result.returncode} for `{cmd_summary} ...`\n"
           f"  meta pairs: {meta_args}\n"
           f"  stderr (500c): {result.stderr[:500] if result.stderr else '(empty)'}\n"
           f"  stdout (500c): {result.stdout[:500] if result.stdout else '(empty)'}"
       )
   ```
   Any non-zero rc is a failure, full stop. INFO log noise on stderr does not change that.

2. **Add invocation logging** — before `subprocess.run`, log at DEBUG level the full command (redact `--auth-token`):
   ```python
   safe_cmd = [a if not (i>0 and cmd[i-1] == "--auth-token") else "***" for i, a in enumerate(cmd)]
   logger.debug("engram invoke: %s", " ".join(safe_cmd))
   ```

3. **Add `--dry-run` helper method** on the adapter (new, not on CLI):
   ```python
   def print_resolved_command(self, method: str, **kwargs) -> None:
       """Print the exact CLI invocation that would run. For schema validation pre-ingestion."""
   ```
   This is adapter-local — does not require CLI support. Caller can use this to eyeball arg lists before a multi-hour ingestion.

#### Call site 2: `cogmembench/benchmarks/longmemeval/engram_adapter.py`

**Current state**: same buggy rc-check pattern (L78–82). No `meta=` parameter on `store()`.

**Required changes:**

1. Fix rc-check (same patch as call site 1).
2. Add `meta: dict | None = None` parameter to `store()` (mirror LoCoMo adapter signature) — even though LongMemEval today doesn't need dimensions, symmetry keeps the two adapters from diverging and unblocks future LongMemEval dimensional work.
3. Add invocation logging.

#### Call site 3: (none — placeholder removed)

`cogmembench/benchmarks/common/` does not invoke engram directly. No action.

### 2.4 Future-Proofing

Add a **test in the cogmembench repo** (not in engram) that greps for `subprocess.run.*engram` and asserts every invocation has `check=True` OR is wrapped in a helper that does explicit rc checking. One-line `check=True` in `subprocess.run` is the Pythonic way to get rid of this class of bug permanently, but we're keeping the helper pattern because we need the structured error message.

A CI check is acceptable but not required for this issue — the two fixed call sites cover 100% of today's traffic, and adding a new adapter means writing a `_run` helper, which developers will copy from the existing ones (which will now be correct).

### 2.5 Scope Boundary

**Explicitly out of scope for Change 2:**

- Adding a CLI `--dry-run` flag to `engram store` itself. The adapter-local dry-run printout is sufficient for the "validate before ingestion" use case, and avoids a CLI feature we'd have to design and test.
- Migrating longmemeval to actually pass dimensions (separate benchmarking work, not a pipeline bug).
- Retroactively checking stored databases for memories written with dropped `--meta`. Covered in ISS-024 investigation's "Out of Scope" (backfill tool).

---

## 3. Reserved-Namespace Semantics (Change 1 behavior, AC-2)

### 3.1 The Question

`--meta` is the caller-owned side channel. The extractor writes to the same `metadata` blob. Since ISS-019 Step 7a v2, that blob is **namespaced** — caller data lives under `metadata.user.*`, engram-owned data lives under `metadata.engram.*`. Collision rules must be deterministic.

### 3.2 Storage Layout and Reserved Surface

The persisted `metadata` object has exactly two top-level keys:

```json
{
  "engram": {
    "version": 2,
    "dimensions": { /* typed Dimensions serialization */ },
    "merge_count": 0,
    "merge_history": []
  },
  "user": {
    "dia_id": "D1:3",
    "speaker": "Alice"
  }
}
```

- **`engram` is a fully reserved namespace.** Everything under it is engram-owned (dimensions, merge provenance, future extractor metadata). `--meta` MUST NOT write to `engram` or any of its subkeys.
- **`user` is the caller surface.** All `--meta key=value` pairs land under `metadata.user.<key>`. The caller owns this namespace and can put anything there.
- **No other top-level keys.** `--meta` does not create sibling top-level keys; it only appends to `user`.

The reserved set the CLI parser must reject is therefore a two-tier check:

| Form of rejection | What it catches |
|---|---|
| `--meta engram=...` | Attempt to clobber the whole engram namespace. |
| `--meta engram.foo=...` | Already rejected by §3.5 rule 2 (no `.` in keys); double-covered. |
| `--meta user=...` | Attempt to clobber the user namespace container itself. Rejected with the same reserved-key message. |
| Any `<key>` that shadows a currently-documented `user.*` convention? | No — `user.*` is the caller's to define. Benchmarks may standardize on `dia_id`, `speaker`, etc., but engram itself never reads from `user.*`, so no collision is possible. |

**The reserved-key set the `parse_kv` validator enforces** (the list shown in the `--help` text and the error message of §3.4):

| Reserved key | Why |
|---|---|
| `engram` | Entire engram-owned namespace. |
| `user` | The caller-namespace container — writing here would replace the whole thing. |

All other keys are caller-owned and routed under `metadata.user.<key>` by the CLI layer.

Rationale for keeping the list this tight: now that storage is namespaced, the engram-internal fields (`dimensions`, `merge_count`, `merge_history`, future extractor metadata) are *already* unreachable from the CLI because they live under `engram.*` and the parser rejects any `.` in keys (§3.5). We don't need to enumerate them individually — the namespace is the boundary. This is a strictly smaller and more future-proof reserved set than r0's flat-layout list.

### 3.3 Rejection Behavior — Hard Error, rc=2

**Decision: hard-reject at CLI parse time, exit rc=2.** No warn-and-continue, no silent-drop, no late-stage validation.

Rationale (first-principles):

- The entire bug class this issue fixes is "adapter calls CLI, CLI silently drops data, adapter happily continues". Any "soft" behavior — warning, fallback, override — reintroduces the same failure mode for a different key.
- rc=2 is clap's conventional "argument error" exit code. Adapters' rc-check (§2) will catch this loudly.
- Hard error makes the reserved-key set part of the CLI contract, visible in `--help` and CI-testable.

### 3.4 Exact Error Message

Produced by the `parse_kv` clap parser when key is reserved:

```
error: --meta key 'engram' is reserved (engram-owned namespace)

  Reserved keys: engram, user

  - `engram` is the container for engram-managed fields (dimensions,
    merge_history, merge_count, and future extractor metadata).
  - `user` is the container for caller-owned metadata; its contents
    are what `--meta <k>=<v>` populates. Passing `--meta user=...`
    would replace the whole namespace.

  To pass caller-owned data, use any other key (e.g. `--meta dia_id=...`).
  Your value will be stored at `metadata.user.<key>`.
  See docs/metadata-channel.md for the side-channel contract.
```

Implementation: the `parse_kv` closure returns `Err(String)` with this message; clap's `value_parser` machinery surfaces it as an argument error and sets rc=2.

### 3.5 Key-Name Validation

Beyond the reserved set, `parse_kv` enforces:

1. **Non-empty key**: `--meta =value` → rc=2 ("empty key not allowed").
2. **No `.` in keys**: `--meta foo.bar=x` → rc=2. Dots imply nested JSON paths; we don't support partial-path writes in v1. Future work if needed.
3. **Printable-ASCII keys only** (regex `^[A-Za-z0-9_][A-Za-z0-9_-]*$`): prevents injection of control characters, whitespace keys, unicode lookalikes that break JSON serialization assumptions. Error: `"key 'X' must match [A-Za-z0-9_][A-Za-z0-9_-]*"`.
4. **Length cap**: key ≤ 64 chars, value ≤ 4096 chars (reject with message). Defensive; prevents accidental giant-blob ingestion.

### 3.6 Value Semantics

- `--meta foo=bar` → `{"foo": "bar"}` (JSON string).
- `--meta foo=` → `{"foo": ""}` (empty string is legal; caller may want to signal "key present, no value").
- `--meta foo=bar=baz` → `{"foo": "bar=baz"}` (split on FIRST `=` only).
- No automatic type coercion. `--meta count=5` stores `"5"` as string. If callers need structured values (numbers, arrays), they use a future `--meta-json` flag (out of scope here; investigation §Change 1 notes this).
- UTF-8 strings allowed in values (regex on key only).

### 3.7 Duplicate Keys

If the same non-reserved key is passed twice, e.g. `--meta foo=a --meta foo=b`:

- **Last-write-wins**: `{"foo": "b"}`.
- Rationale: clap's `ArgAction::Append` collects into `Vec`; we fold into a `Map` by insertion order; later entries overwrite earlier. This is the least-surprising behavior and matches HTTP header / env var conventions.
- Log at INFO on duplicate: `"--meta key 'foo' specified multiple times; last value wins"`. Not a hard error — benchmarks legitimately build up arg lists programmatically and may double-set by accident.

### 3.8 Merge Semantics (CLI-parsed `--meta` vs extractor output)

At `Memory::remember` time:

1. CLI collects all `--meta <k>=<v>` pairs into a map and hands it to `Memory::remember` as the caller's `user_metadata` (a `serde_json::Value::Object`). Reserved-key validation (§3.2) has already fired, so every key is safe.
2. Extractor runs (when `--extractor` is set), populating the typed `EnrichedMemory.dimensions`.
3. At persistence (`build_legacy_metadata` in `src/memory.rs`), the two are composed into the v2 layout:
   ```rust
   { "engram": { "version": 2, "dimensions": <typed>, "merge_count": 0, ... },
     "user":   <caller map> }
   ```
4. **No shallow-merge collision is possible** — the two namespaces are disjoint by construction. The caller cannot write to `engram.*` (rejected at §3.2); the extractor never writes to `user.*` (it writes to `EnrichedMemory.dimensions`, which the builder places under `engram.dimensions`).
5. Invariant: at read time, `metadata.user.<key>` (caller) and `metadata.engram.dimensions.*` (extractor) coexist without collision. This is stronger than r0's "by policy" guarantee — it's "by layout".

### 3.9 `--help` Documentation

Update the `Store` subcommand docstring:

```rust
/// Caller-owned metadata key=value pairs. May be repeated.
///
/// Values are stored under `metadata.user.<key>` in the memory's metadata
/// object. The sibling `metadata.engram.*` namespace is engram-managed
/// (dimensions, merge history, etc.) and is not reachable from this flag.
///
/// Reserved keys (rejected with rc=2): `engram`, `user`. Using these would
/// clobber the namespace containers themselves. Any other key is allowed.
///
/// Keys must match [A-Za-z0-9_][A-Za-z0-9_-]* and be ≤ 64 chars.
/// Values must be ≤ 4096 chars (UTF-8 allowed).
///
/// Duplicate keys: last value wins (logged at INFO).
///
/// Example: `engram store "..." --meta dia_id=D1:3 --meta speaker=Alice`
///   → stored as `metadata.user.dia_id = "D1:3"`,
///              `metadata.user.speaker = "Alice"`
#[arg(long = "meta", value_parser = parse_kv, action = clap::ArgAction::Append)]
metadata: Vec<(String, String)>,
```

### 3.10 Tests (AC-2 coverage)

- `test_parse_kv_simple` — `foo=bar` → `Ok(("foo", "bar"))`.
- `test_parse_kv_empty_value` — `foo=` → `Ok(("foo", ""))`.
- `test_parse_kv_equals_in_value` — `foo=a=b` → `Ok(("foo", "a=b"))`.
- `test_parse_kv_reserved_rejected` — both `engram=...` and `user=...` return `Err` with the exact message from §3.4.
- `test_parse_kv_empty_key` — `=bar` → `Err("empty key")`.
- `test_parse_kv_invalid_chars` — `foo.bar=x` (covers `engram.dimensions=x` too), `foo bar=x`, `日本=x` → `Err`.
- `test_parse_kv_length_cap` — key >64 chars, value >4096 chars → `Err`.
- `test_store_meta_and_extractor_coexist` — end-to-end: `store ... --meta foo=bar --extractor anthropic` persists BOTH `metadata.user.foo == "bar"` AND `metadata.engram.dimensions.*` populated. **Assertions walk the v2 nested layout.**
- `test_store_duplicate_meta_keys` — `--meta foo=a --meta foo=b` → `metadata.user.foo == "b"`.

---

## 4. Contract Test Framework (Change 4 design)

### 4.1 Purpose

Change 4 is "the contract". If it passes, the pipe is wired. The test must be structured so that **future cross-CLI-boundary issues copy-paste the pattern**, not rebuild scaffolding.

Today we have zero tests that spawn the `engram` binary as a subprocess and assert on cross-tier behavior. This is the root cause of "unit tests all green, real pipeline broken" (investigation §Evidence).

### 4.2 Location

New file: `tests/cli_contract.rs` (integration test; runs via `cargo test --test cli_contract`).

Reusable helpers in the same file (no separate harness crate — YAGNI for v1, two tests isn't enough to justify extraction). If Change 4 spawns a third or fourth cross-boundary test in a future issue, extract `tests/common/cli_harness.rs` then.

### 4.3 Reusable Harness (inline in `tests/cli_contract.rs`)

```rust
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Harness for spawning the engram binary against an ephemeral database.
pub struct CliHarness {
    binary: PathBuf,
    db_dir: TempDir,
    workspace: TempDir,
}

impl CliHarness {
    /// Build the binary (once per test binary invocation) and set up temp dirs.
    pub fn new() -> Self {
        let binary = locate_built_binary(); // target/debug/engram or $ENGRAM_BIN
        Self {
            binary,
            db_dir: TempDir::new().unwrap(),
            workspace: TempDir::new().unwrap(),
        }
    }

    pub fn db_path(&self) -> PathBuf { self.db_dir.path().join("test.db") }

    /// Run `engram <args>` with standard flags (database, workspace) prepended.
    /// Returns Output. Does NOT panic on non-zero rc — caller decides.
    pub fn run(&self, args: &[&str]) -> Output {
        Command::new(&self.binary)
            .arg("--database").arg(self.db_path())
            .arg("--workspace").arg(self.workspace.path())
            .args(args)
            .output()
            .expect("spawn failed")
    }

    /// Convenience: run and assert rc=0.
    pub fn run_ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(out.status.success(),
            "engram {:?} failed: rc={:?}\nstdout={}\nstderr={}",
            args, out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        out
    }

    /// Convenience: run and assert rc=2 (argument error).
    pub fn run_arg_err(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert_eq!(out.status.code(), Some(2), "expected rc=2, got {:?}", out.status.code());
        out
    }

    /// Parse `engram recall --json` output into Vec<serde_json::Value>.
    pub fn recall_json(&self, query: &str, ns: &str, limit: usize) -> Vec<serde_json::Value> {
        let out = self.run_ok(&["recall", query, "--ns", ns, "--limit", &limit.to_string(), "--json"]);
        parse_json_array_from_stdout(&out.stdout)
    }
}

fn locate_built_binary() -> PathBuf { /* check $ENGRAM_BIN, else target/debug/engram */ }
fn parse_json_array_from_stdout(bytes: &[u8]) -> Vec<serde_json::Value> { /* strip INFO lines */ }
```

Design notes:

- **`tempfile::TempDir`** auto-cleans on drop. Every test gets a pristine DB.
- **No `unwrap` in test assertions** — the assertion message is the debugging surface.
- **`run()` vs `run_ok()` vs `run_arg_err()`** — three ergonomic variants so tests read like prose.
- **No hidden retries** — if the CLI misbehaves intermittently, the test should fail, not mask.

### 4.4 Contract Test — Dimensional Pipeline End-to-End

```rust
#[test]
fn iss024_dimensional_pipeline_end_to_end() {
    let h = CliHarness::new();
    let ns = "contract_test";

    // AC-1: --meta round-trips
    let out = h.run_ok(&["store", "Alice met Bob at 3pm yesterday",
                         "--ns", ns, "--type", "episodic",
                         "--meta", "dia_id=D1:3",
                         "--meta", "speaker=Alice"]);
    let id = extract_memory_id(&out.stdout);
    assert!(!id.is_empty(), "store did not return a memory ID");

    // Verify metadata persistence via a separate `engram inspect` or recall --json.
    // Assertions walk the v2 nested layout: caller keys live under `metadata.user.*`.
    let records = h.recall_json("Alice Bob meeting", ns, 5);
    let rec = records.iter().find(|r| r["id"].as_str() == Some(&id))
        .expect("stored memory not retrievable");
    assert_eq!(rec["metadata"]["user"]["dia_id"], "D1:3");
    assert_eq!(rec["metadata"]["user"]["speaker"], "Alice");
    // And the engram namespace is populated by the extractor pipeline.
    assert!(rec["metadata"]["engram"]["dimensions"].is_object(),
            "expected engram.dimensions to be populated by extractor");

    // AC-2: reserved namespace rejected (both containers — see §3.2 revised set)
    h.run_arg_err(&["store", "x", "--ns", ns, "--type", "factual",
                    "--meta", "engram=should_reject"]);
    h.run_arg_err(&["store", "x", "--ns", ns, "--type", "factual",
                    "--meta", "user=should_reject"]);

    // AC-3: dimension-matched memory ranks higher for a temporal query
    //   Seed: two memories, one with extractor-populated dimensions.temporal = yesterday
    //         matching the query, one without.
    //   (Extractor must be stubbed or mocked — see §4.5)
    // Assert the dimensional memory is in top-3 AND its score > the control's score.
    // This is the actual Gap B contract.

    // AC-4: contract test runs in CI (covered by being in tests/)
}

#[test]
fn iss024_empty_dimensions_preserves_ranking() {
    // Backwards compat: when dimensions are empty everywhere, ranking is identical
    // to pre-ISS-024 behavior. Run the same recall twice, once with dimensional
    // code path disabled (via env var or feature flag), once enabled. Assert
    // identical top-K ordering.
}
```

### 4.5 Extractor Handling in CI

The extractor (Anthropic API call) is non-deterministic and requires network + auth. Contract tests MUST NOT depend on it.

**Strategy**: A `--extractor stub` mode that reads a JSON fixture from env var `ENGRAM_EXTRACTOR_STUB_PATH` and returns its contents as the extracted fact. Implementation effort: ~30 LOC in `src/extractor.rs`, one new `StubExtractor` variant. Existing tests that need determinism can switch to this.

Fixtures live in `tests/fixtures/iss024/`:
- `alice_met_bob.json` — one fact, temporal dimension populated.
- `control.json` — one fact, empty dimensions.

This is a **separate contract-test prerequisite task**: `iss-024-extractor-stub`. Currently not in the graph; will add after this design is approved (see §9 Graph Updates).

Alternative considered and rejected: VCR-style replay of real Anthropic responses. Too fragile (prompt changes invalidate cassettes; auth token leakage risk).

### 4.6 Performance

- Contract test runs take ~2 s (binary spawn + DB init + 2 stores + 2 recalls).
- CI budget: acceptable. If it grows beyond 10 s, promote the harness to a shared test binary that reuses a DB across tests.

### 4.7 Out of Scope

- Parameterized tests across all 7 recall channels. This issue only wires `dimensions.temporal` via `temporal_score`. Full channel coverage lands with ISS-020 Phase B.
- Fuzzing `parse_kv`. The regex in §3.5 is simple enough that unit tests are sufficient.

---

## 5. 3a → 3b Forward-Compat Interface

### 5.1 Goal

Change 3a (temporal_score uses `dimensions.temporal`) is the MVP. Change 3b (new `dimension_match` 8th channel covering participants/relations/etc.) is ISS-020 Phase B. 3a's code must not need refactoring when 3b lands.

### 5.2 Interface Shape

Introduce a **typed dimension accessor layer** — a small module that parses the v2 metadata blob into engram's canonical `Dimensions` struct once per record, then exposes typed getters. This replaces r0's free-form JSON walker with a wrapper around the existing `Dimensions::from_stored_metadata` helper (`src/dimensions.rs`, already handles v1/v2 layout detection and defaulting).

```rust
// src/dimension_access.rs (new, ~60 LOC)

use crate::dimensions::{Dimensions, TemporalMark};
use crate::MemoryRecord;

/// Typed accessor for the extractor-populated dimension signature of a record.
///
/// Internally holds a parsed `Dimensions` (cheap — `Dimensions::from_stored_metadata`
/// is a pure JSON deserialize, no allocation of persistent state). All getters are
/// thin borrows of the typed struct; no string-array fallbacks, no JSON walking.
///
/// Safe against missing/malformed metadata: `from_record` always returns a view
/// backed by at least `Dimensions::minimal(record.content)`.
pub struct DimensionView<'a> {
    record: &'a MemoryRecord,
    dims: Dimensions,
}

impl<'a> DimensionView<'a> {
    pub fn from_record(record: &'a MemoryRecord) -> Self {
        let dims = match record.metadata.as_ref() {
            Some(meta) => Dimensions::from_stored_metadata(meta, &record.content)
                .unwrap_or_else(|_| {
                    // core_fact empty — synthesize from a placeholder;
                    // recall path never feeds empty-content records, so
                    // this branch is defensive only.
                    Dimensions::minimal(&record.content)
                        .unwrap_or_else(|_| Dimensions::minimal("?").unwrap())
                }),
            None => Dimensions::minimal(&record.content)
                .unwrap_or_else(|_| Dimensions::minimal("?").unwrap()),
        };
        Self { record, dims }
    }

    /// Typed temporal mark as the extractor stored it. `None` means the
    /// extractor produced no temporal signal for this memory.
    pub fn temporal(&self) -> Option<&TemporalMark> {
        self.dims.temporal.as_ref()
    }

    /// Participants — free-form string as stored by the extractor today.
    /// (If ISS-020 Phase B splits this into a Vec, the return type changes
    /// here; 3a doesn't read this field so it's untouched by Change 3a.)
    pub fn participants(&self) -> Option<&str> {
        self.dims.participants.as_deref()
    }

    pub fn relations(&self) -> Option<&str> { self.dims.relations.as_deref() }
    pub fn sentiment(&self) -> Option<&str> { self.dims.sentiment.as_deref() }
    pub fn location(&self) -> Option<&str>  { self.dims.location.as_deref() }
    pub fn context(&self) -> Option<&str>   { self.dims.context.as_deref() }
    pub fn causation(&self) -> Option<&str> { self.dims.causation.as_deref() }
    pub fn outcome(&self) -> Option<&str>   { self.dims.outcome.as_deref() }

    /// True if ANY narrative dimension is populated. Cheap guard for skipping work.
    pub fn has_any_narrative(&self) -> bool {
        self.dims.temporal.is_some()
            || self.dims.participants.is_some()
            || self.dims.relations.is_some()
            || self.dims.location.is_some()
            || self.dims.context.is_some()
            || self.dims.causation.is_some()
            || self.dims.outcome.is_some()
            || self.dims.sentiment.is_some()
    }

    /// Expose the underlying typed struct for callers that want everything
    /// (scalars, type_weights, etc.). 3b uses this to avoid adding a
    /// getter-per-field as new dimensions appear.
    pub fn dimensions(&self) -> &Dimensions {
        &self.dims
    }
}
```

Notes:

- **No `Vec<&str>` anywhere.** The extractor's output is already typed; we don't pretend otherwise.
- **`Dimensions::from_stored_metadata` handles both v1 and v2 layouts** (see `src/dimensions.rs`). If we later encounter a row from an older write path, it still parses. If the row has no `engram.dimensions` (e.g. no-extractor store), parsing falls through to `Dimensions::minimal` semantics — every getter returns `None`.
- **No `DimParseCache` is needed for the accessor itself.** Parsing a `Dimensions` from JSON is microseconds; the expensive path is §1 (`two_timer::parse` for `Vague(_)`), which is still cached separately.

### 5.3 3a Usage

`temporal_score` in `src/memory.rs`:

```rust
use crate::dimensions::TemporalMark;
use chrono::{DateTime, Duration, NaiveTime, Utc};

fn temporal_score(
    record: &MemoryRecord,
    time_range: &Option<TimeRange>,
    now: DateTime<Utc>,
    dim_parser: &mut DimParseCache,  // still used — but only for Vague(_)
) -> f64 {
    let insertion_score = /* existing logic against record.created_at */;

    let dim_score = match time_range {
        Some(range) => {
            let view = DimensionView::from_record(record);
            let dim_range: Option<TimeRange> = match view.temporal() {
                Some(TemporalMark::Exact(dt)) => Some(TimeRange { start: *dt, end: *dt }),
                Some(TemporalMark::Range { start, end }) => {
                    // NaiveDate endpoints (inclusive) → UTC range covering
                    // [start 00:00, end+1 00:00). Semantically similar to `Day`
                    // but spans multiple days. `NaiveTime::MIN` is a const and
                    // infallible, so no `?` is needed in this f64-returning fn.
                    let s = start.and_time(NaiveTime::MIN).and_utc();
                    let e = end.and_time(NaiveTime::MIN).and_utc() + Duration::days(1);
                    Some(TimeRange { start: s, end: e })
                }
                Some(TemporalMark::Day(date)) => {
                    // Midnight-to-midnight UTC range for the stored calendar day.
                    let start = date.and_time(NaiveTime::MIN).and_utc();
                    let end = start + Duration::days(1);
                    Some(TimeRange { start, end })
                }
                Some(TemporalMark::Vague(phrase)) => {
                    dim_parser.get_or_parse(phrase, record.created_at)
                }
                None => None,
            };
            dim_range
                .map(|dr| range_overlap_score(range, &dr))
                .unwrap_or(0.0)
        }
        None => 0.0,
    };

    insertion_score.max(dim_score)  // max-of-two as per investigation §Change 3a
}
```

Key differences from r0:

- **No phrase-loop.** `temporal` is a single `Option<TemporalMark>`, so `.fold(0.0, f64::max)` across phrases collapses to a single `.unwrap_or(0.0)`.
- **Three of four variants bypass `two_timer` entirely.** Only `Vague(phrase)` hits `DimParseCache`.
- **`Day` expansion rule is explicit** (midnight UTC to midnight UTC of the *next* day, 24-hour span). Document this in the doc comment; a future refinement could use the row's stored timezone if we add one, but engram today is UTC-only.

### 5.4 3b Reuse (proof that refactoring isn't needed)

Future `dimension_match_score` in ISS-020 Phase B will look like:

```rust
fn dimension_match_score(record: &MemoryRecord, query_dims: &DimensionView) -> f64 {
    let candidate = DimensionView::from_record(record);
    if !candidate.has_any_narrative() { return 0.0; }

    // 3b either adds per-field scoring on the existing Option<&str> getters...
    let participants_score = str_jaccard(candidate.participants(), query_dims.participants());
    let relations_score    = str_overlap(candidate.relations(),    query_dims.relations());

    // ...or, if ISS-020 promotes `participants: String` to `Vec<String>` in
    // `Dimensions`, the underlying `dimensions.rs` change flows up through
    // `view.dimensions().participants` without any API shift in `DimensionView`.
    // 3a never reads these fields so it's unaffected either way.

    weighted_sum(participants_score, relations_score, /* ... */)
}
```

**3b adds NEW scoring logic but reuses the same typed accessor.** 3a's code (only the `temporal()` getter) is unchanged when 3b lands. The forward-compat guarantee holds, and it is now stronger than r0's — because we sit on top of `Dimensions` rather than raw JSON, field-type evolution in `Dimensions` propagates through a single `from_stored_metadata` call site.

### 5.5 Threading `DimParseCache` Through the Recall Path

`temporal_score` needs the cache, but the surrounding `recall_from_namespace` caller is the natural owner (cache lifetime = one recall call). Plumbing:

- `recall_from_namespace` constructs a `DimParseCache::new(CACHE_CAPACITY_DEFAULT)` at entry.
- Passes `&mut DimParseCache` to all channel scorers that need it (today: only `temporal_score`; tomorrow: potentially others).
- Cache is discarded at function return. No global state, no cross-call bleed.

If future profiling shows a hot loop where the same queries reparse identical phrases across recall calls, we can hoist to a `Memory`-level cache behind a `Mutex` or `RwLock`. Not needed now.

### 5.6 Feature Flags

None. This is not a behavior-flagged rollout; it's a correctness fix. The `max(insertion, dimension)` rule guarantees no regression — worst case dimension_score = 0 and we return the old value.

If `two_timer` turns out to be unacceptably slow under production load (watch profiling after deploy), we can gate on `ENGRAM_TEMPORAL_DIM` env var. Ship without the flag; add if needed.

---

## 6. Architecture Summary

### 6.1 Data Flow (Post-Fix)

```
                  ┌─────────────────────────────────────────┐
                  │  Adapter (locomo/longmemeval)           │
                  │  store(content, meta={dia_id: "D1:3"})  │
                  └────────────────┬────────────────────────┘
                                   │ subprocess with --meta
                                   ▼
                  ┌─────────────────────────────────────────┐
                  │  engram store ... --meta dia_id=D1:3    │
                  │  parse_kv: validate, reject reserved    │
                  │  ─► Vec<(String, String)>               │
                  │  fold into serde_json::Value::Object    │
                  └────────────────┬────────────────────────┘
                                   │ metadata: serde_json::Value
                                   ▼
                  ┌─────────────────────────────────────────┐
                  │  Memory::remember(content, type, imp,   │
                  │                   metadata)             │
                  │  Extractor runs (if configured)         │
                  │   → merges dimensions.* into metadata   │
                  │  SQLite insert                          │
                  └────────────────┬────────────────────────┘
                                   │ persistence
                                   ▼
            ╔════════════════════════════════════════════════════════╗
            ║  metadata: {                                           ║
            ║    engram: {                ← engram-owned (v2)        ║
            ║      version: 2,                                       ║
            ║      dimensions: {          ← extractor-owned          ║
            ║        core_fact: "Alice met Bob...",                  ║
            ║        temporal: { kind: "day", value: "2026-04-21" }, ║
            ║        participants: "Alice, Bob",                     ║
            ║        relations: "met",                               ║
            ║        ...                                             ║
            ║      },                                                ║
            ║      merge_count: 0,                                   ║
            ║      merge_history: [],                                ║
            ║    },                                                  ║
            ║    user: {                  ← caller-owned via --meta  ║
            ║      dia_id: "D1:3",                                   ║
            ║      speaker: "Alice",                                 ║
            ║    },                                                  ║
            ║  }                                                     ║
            ╚════════════════════════════════════════════════════════╝

Notes on the serialized `temporal` shape (per `src/dimensions.rs:222` — `#[serde(tag = "kind", content = "value")]`):
- `Exact` → `{"kind": "exact", "value": "2026-04-22T10:30:00Z"}`
- `Day`   → `{"kind": "day", "value": "2026-04-21"}`
- `Range` → `{"kind": "range", "value": {"start": "2026-04-20", "end": "2026-04-22"}}`
- `Vague` → `{"kind": "vague", "value": "last summer"}`

                                   │ at query time
                                   ▼
                  ┌─────────────────────────────────────────┐
                  │  recall_from_namespace                  │
                  │  ┌─────────────────────────────────┐    │
                  │  │ DimParseCache::new()            │    │
                  │  │ for each candidate:             │    │
                  │  │   DimensionView::from_record    │    │
                  │  │   temporal_score =              │    │
                  │  │     max(insertion_score,        │    │
                  │  │         dim_time_score via      │    │
                  │  │         two_timer::parse)       │    │
                  │  └─────────────────────────────────┘    │
                  │  (ranks dimension-matched memories      │
                  │   higher when query has time range)     │
                  └─────────────────────────────────────────┘
```

### 6.2 New / Modified Files

| File | Change | LOC est. |
|---|---|---|
| `src/main.rs` | Add `--meta` arg + `parse_kv` helper to `Store` subcommand; thread metadata to `Memory::remember` | +60 / −0 |
| `src/dimension_access.rs` | **NEW** `DimensionView` accessor | +100 (+60 src, +40 tests) |
| `src/temporal_dim.rs` | **NEW** `parse_dimension_time` + `DimParseCache` | +150 (+120 src, +30 tests) |
| `src/dimensions.rs` | Fix `from_legacy_metadata` to accept both string and typed-object `temporal` payloads (see §1.1a); add roundtrip tests | +15 / −2 |
| `src/memory.rs` | Modify `temporal_score` signature to take `&mut DimParseCache`; consume `DimensionView`; update `recall_from_namespace` caller | +50 / −10 |
| `src/lib.rs` | `pub mod dimension_access; pub mod temporal_dim;` | +2 |
| `Cargo.toml` | Add `two_timer = "2.2"`, `lru = "0.12"` | +2 |
| `tests/cli_contract.rs` | **NEW** harness + contract tests | +250 |
| `tests/fixtures/iss024/*.json` | **NEW** extractor stub fixtures | +20 |
| `src/extractor.rs` | Add `StubExtractor` (reads `ENGRAM_EXTRACTOR_STUB_PATH`) | +40 |
| `cogmembench/benchmarks/locomo/engram_adapter.py` | Fix rc-check; redact auth in logs | +15 / −3 |
| `cogmembench/benchmarks/longmemeval/engram_adapter.py` | Fix rc-check; add `meta` parameter | +20 / −3 |
| `docs/metadata-channel.md` | **NEW/UPDATE** reserved namespace contract | +80 |

Total estimate: ~800 LOC across 11 files.

### 6.3 Dependencies Added

- `two_timer = "2.2"` — natural-language time parsing (see §1.2).
- `lru = "0.12"` — LRU cache for `DimParseCache`.
- `tempfile = "3"` in `[dev-dependencies]` if not already present — for `CliHarness`.

No runtime dep on `tokio`, `reqwest`, or any large crate. The Rust supply-chain impact is minimal (two_timer itself pulls `chrono`, `lazy_static`, `pidgin`, `regex`, `serde_json`, all of which are already transitive or std-level).

---

## 7. Guards / Invariants

- **GUARD-1 (backwards-compat):** For any recall where ALL candidates have empty `metadata.dimensions`, ranking output is bit-identical to pre-ISS-024. Contract test `iss024_empty_dimensions_preserves_ranking` enforces.
- **GUARD-2 (no silent data loss):** The CLI exits non-zero (rc=2 for arg errors, rc=1 for runtime errors) whenever `--meta` is rejected, malformed, or fails to persist. `parse_kv` rejection tests (§3.10) enforce for parse-time; existing storage test patterns enforce for persist-time.
- **GUARD-3 (reserved-key integrity):** No code path outside the extractor, dedup merger, and synthesis pipeline may write to reserved keys. Enforced socially today (code review); a unit test can grep the codebase for writes to `metadata["dimensions"] =` as a smoke check. Out of scope for this issue.
- **GUARD-4 (cache safety):** `DimParseCache` is per-recall-call (§5.5); no cross-call leakage. Unit test with two back-to-back recalls against different namespaces.
- **GUARD-5 (parse failure is soft):** A malformed `TemporalMark::Vague(phrase)` must never crash recall or produce NaN scores. Test: seed a record with `dimensions.temporal = Some(TemporalMark::Vague("🤡 nonsense".to_string()))`, verify recall succeeds and ranking is finite. The other `TemporalMark` variants are constructed from typed inputs and cannot be malformed.
- **GUARD-6 (extractor doesn't overwrite caller keys):** End-to-end test `test_store_meta_and_extractor_coexist` (§3.10) proves extractor-written `metadata.engram.dimensions.*` does not touch caller-written `metadata.user.*` keys. The two namespaces are disjoint by construction (§3.8).

---

## 8. Acceptance Criteria (mapped from investigation §AC)

| AC | Description | How verified |
|---|---|---|
| AC-1 | `engram store "x" --meta foo=bar` returns rc=0, stores `metadata.foo == "bar"` | Contract test `iss024_dimensional_pipeline_end_to_end` asserts |
| AC-2 | Extractor + `--meta` coexist; `--meta dimensions=...` → rc=2 | Contract test `run_arg_err` + unit tests §3.10 |
| AC-3 | Recall ranking unchanged for empty dimensions; higher for matched | Contract test `iss024_empty_dimensions_preserves_ranking` + `iss024_dimensional_pipeline_end_to_end` AC-3 assertion |
| AC-4 | Contract test lives in CI | Test lives in `tests/cli_contract.rs`, runs under `cargo test` |
| AC-5 | Adapter rc-checks added | Direct diff on both adapter files; rc-check verified to raise on `--meta` rejection |

All five ACs are covered by concrete tests; no "looks reasonable" gaps.

---

## 9. Implementation Order & Graph Updates

### 9.1 Recommended Order

1. **Prep (blocks everything):** `iss-024-temporal-roundtrip` — fix `from_legacy_metadata` to deserialize typed `temporal` objects, add roundtrip tests. Without this, §5.3's `Exact/Range/Day` branches are dead code.
2. **Prep: `iss-024-extractor-stub`** (new task — add to graph) — add `StubExtractor` in `src/extractor.rs`. Needed before contract test can deterministically exercise dimensions.
3. **Parallel tier A (after this design approval):**
   - `iss-024-cli-meta` (Change 1) — `--meta` flag + `parse_kv` + reserved-key rejection
   - `iss-024-adapter-rc` (Change 2) — fix rc-check bug in both adapters
4. **After tier A: `iss-024-temporal-dim`** (Change 3a) — `DimensionView`, `TemporalDimParser`, `temporal_score` extension
5. **After 1, 2, 3: `iss-024-contract-test`** (Change 4) — harness + end-to-end tests
6. **After 4: unblock `iss-020` Phase B** for dimension_match channel (3b) — separate issue

### 9.2 Graph Updates Needed

- **New task `iss-024-temporal-roundtrip`:** Fix v2 `temporal` roundtrip in `from_legacy_metadata`. Blocks: `iss-024-temporal-dim`, `iss-024-contract-test`. See §1.1a.

One new task node to add before implementation begins:

```yaml
- id: iss-024-extractor-stub
  title: "ISS-024 Prep: Add StubExtractor for deterministic testing"
  status: todo
  description: |-
    Add a `StubExtractor` variant in src/extractor.rs that reads a fixture JSON
    from ENGRAM_EXTRACTOR_STUB_PATH and returns it as the extraction result.
    Needed by iss-024-contract-test for deterministic dimension seeding in CI
    (real Anthropic extractor is non-deterministic and network-dependent).
    ~40 LOC + 2 fixture JSON files in tests/fixtures/iss024/.
  tags: [iss-024, testing, prep]
  priority: 45
  type: task
  metadata:
    files: [src/extractor.rs, tests/fixtures/iss024/*.json]
    iss: ISS-024
    blocked_by: iss-024-design
```

Edges to add (for `iss-024-extractor-stub`):
- `iss-024-extractor-stub` → `iss-024-design` (depends_on)
- `iss-024-extractor-stub` → `iss-024` (subtask_of)
- `iss-024-contract-test` → `iss-024-extractor-stub` (depends_on)

Second new task — the prep fix for the temporal roundtrip bug (see §1.1a):

```yaml
- id: iss-024-temporal-roundtrip
  title: "ISS-024 Prep: Fix v2 temporal roundtrip in from_legacy_metadata"
  status: todo
  description: |-
    Extend `from_legacy_metadata` in src/dimensions.rs to accept both
    string (legacy) and typed-object (v2 serde) shapes for the `temporal`
    field. Without this fix, TemporalMark::{Exact,Range,Day} variants are
    silently dropped on read-back, making §5.3's match arms dead code.
    ~15 LOC + ~30 LOC of roundtrip regression tests. See §1.1a.
  tags: [iss-024, correctness, prep]
  priority: 40
  type: task
  metadata:
    files: [src/dimensions.rs]
    iss: ISS-024
    blocked_by: iss-024-design
```

Edges to add (for `iss-024-temporal-roundtrip`):
- `iss-024-temporal-roundtrip` → `iss-024-design` (depends_on)
- `iss-024-temporal-roundtrip` → `iss-024` (subtask_of)
- `iss-024-temporal-dim` → `iss-024-temporal-roundtrip` (depends_on)
- `iss-024-contract-test` → `iss-024-temporal-roundtrip` (depends_on)

Priority 40 is higher than `iss-024-extractor-stub` (45) because this task blocks **every** other iss-024 implementation task — not just the contract test.

### 9.3 Token Cost Estimate for Implementation

- `iss-024-cli-meta` — ~60 LOC src + tests → 1 sub-agent, ~25k tokens with pre-loaded context (src/main.rs + design §3 + §6)
- `iss-024-adapter-rc` — trivial Python edits, ~40 LOC total → main agent direct, ~5k tokens
- `iss-024-temporal-dim` — ~250 LOC across 3 files → 1 sub-agent, ~40k tokens (per ISS-010 Rule 2, 3 files × ~80 LOC each → below 300-line threshold, delegation OK)
- `iss-024-extractor-stub` — ~40 LOC + fixtures → main agent direct, ~8k tokens
- `iss-024-contract-test` — ~250 LOC → main agent with incremental writes (harness skeleton → tests one by one), ~30k tokens

Total implementation budget: ~110k tokens across all tasks. Design phase (this doc) consumed ~25k to write.

---

## 10. Risks & Mitigations

| Risk | Likelihood | Severity | Mitigation |
|---|---|---|---|
| `two_timer` parses phrases differently than extractor expects | Medium | Low | §1.7 unit tests against known extractor output shapes; `None` on mismatch falls back cleanly |
| `DimParseCache` hit rate low in practice → latency regression | Low | Low | `max()` rule caps per-memory cost at O(1) parse; 4K cache + per-call lifetime is conservative |
| Adapter rc-check fix reveals previously-hidden CLI failures | High | Medium (good kind) | Expected — we WANT to see these. Log messages in §2.3 make them actionable. First bench run post-fix may fail noisily; this is working-as-intended |
| Reserved-key list incomplete (miss a future engram-owned key) | Low | Low | Adding keys is backwards-compatible (new rejections); remove is a breaking change (would need deprecation). Start conservative, grow the list. |
| `StubExtractor` fixture format drifts from real extractor output | Medium | Low | Contract test also runs against a minimal real extractor smoke test (optional, env-gated) to catch drift |
| `two_timer` becomes unmaintained | Low | Medium | MIT license + self-contained → could vendor if needed. Build a thin wrapper (§1.3 `parse_dimension_time`) so swapping implementations is a one-function change. |

---

## 11. Signed

- Author: RustClaw, 2026-04-22 22:xx ET
- Based on investigation: `investigation.md` (same folder)
- Reviewer: potato (pending)
- Status: draft — ready for `/ritual review-design`


