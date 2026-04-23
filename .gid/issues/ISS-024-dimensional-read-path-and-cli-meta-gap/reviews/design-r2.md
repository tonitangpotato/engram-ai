# Design Review: ISS-024 r2

**Document:** `.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/design.md` (1065 lines, revision r2)
**Reviewer:** RustClaw (main agent, live-code cross-check)
**Date:** 2026-04-22
**Method:** Full cross-check of r1→r2 diff against live code (`src/dimensions.rs`, `src/memory.rs`). Re-verified Critical findings from r1. Did not re-run full 28-check battery on untouched sections (§2, §4.1-4.3, §7 unchanged).

## Summary

| Severity | r1 count | r2 status |
|---|---|---|
| Critical | 3 | **All 3 fixed** ✅ |
| Important | 5 | deferred (as agreed) |
| Minor | 4 | deferred (as agreed) |
| **r2-introduced** | — | **2 new findings** |
| **Total new** | | **2 new** (1 critical, 1 minor) |

r2 correctly addresses FINDING-1/2/3 but introduces one compile-breaking bug while fixing §5.3 (copied pattern from pre-existing `Day` match arm, which also has the same bug — so this is actually r1-era code that r2 propagated). One minor gap in §9.2 graph-node definition.

---

## r1 Critical findings — r2 verification

### ✅ FINDING-1 (Range type mismatch) — FIXED

**Verified:**
- §1.1 enum now reads `Range { start: NaiveDate, end: NaiveDate }` (line ~57) ✅
- §5.3 match arm now expands NaiveDate endpoints to UTC TimeRange via `and_hms_opt(0,0,0).and_utc() + Duration::days(1)` ✅
- §1.1 "three variants" bullet correctly notes Range is day-granular (line ~73) ✅

Matches live code at `src/dimensions.rs:226-231`. Semantics are now consistent throughout the doc.

### ✅ FINDING-2 (TemporalMark JSON shape) — FIXED

**Verified:**
- §6.1 diagram line now shows `temporal: { kind: "day", value: "2026-04-21" }` ✅
- Post-diagram note block lists all four variants with correct internally-tagged serde shape ✅
- Box width widened from 53→56 cols, right-edge `║` alignment preserved ✅

Matches `#[serde(tag = "kind", content = "value")]` on `src/dimensions.rs:222`.

### ✅ FINDING-3 (roundtrip gap) — FIXED with option 1

**Verified:**
- §1.1a subsection documents the bug, shows the fix snippet, and defines the test plan ✅
- Fix snippet correctly handles both `Value::String` (legacy) and `Value::Object` (v2) paths ✅
- §6.2 LOC table gained a `src/dimensions.rs` row at `+15 / −2` ✅
- §9.1 gained new step 1 `iss-024-temporal-roundtrip`; subsequent items renumbered correctly ✅
- §9.2 gained a bullet referencing the new task ✅

Live-code re-verification at `src/dimensions.rs:432` confirms `get_string("temporal")` drops objects — r2's diagnosis is accurate. The proposed fix snippet will correctly preserve `Exact`, `Range`, `Day` variants through the roundtrip.

---

## r2-introduced findings

### 🔴 FINDING-r2-1 (critical) — `?` operator used in `f64`-returning function — §5.3

**Location:** §5.3 `temporal_score` function body (lines ~777-793).

**Problem.** The function signature returns `f64`:

```rust
fn temporal_score(
    record: &MemoryRecord,
    time_range: &Option<TimeRange>,
    now: DateTime<Utc>,
    dim_parser: &mut DimParseCache,
) -> f64 {
```

But the `Range` and `Day` match arms use `?` for NaiveDate→DateTime conversion:

```rust
// Range arm (r2, new):
let s = start.and_hms_opt(0, 0, 0)?.and_utc();
let e = end.and_hms_opt(0, 0, 0)?.and_utc() + Duration::days(1);

// Day arm (r1, pre-existing but same bug):
let start = date.and_hms_opt(0, 0, 0)?.and_utc();
```

The `?` operator requires the enclosing function to return `Option<_>` or `Result<_, _>`. In a `f64`-returning function, this is a compile error: `the trait FromResidual<Option<Infallible>> is not implemented for f64`.

**Note:** This is not purely r2's mistake — the `Day` arm already had this bug in r1, and r1 review missed it. r2 copied the pattern into the new `Range` arm. Both are broken.

**Root cause.** `NaiveDate::and_hms_opt(0, 0, 0)` is theoretically fallible (if hour/min/sec out of range), but for the literal `(0, 0, 0)` input it can never return None. The correct options:

- **Option A (cleanest):** Use `and_hms_opt(0, 0, 0).unwrap()`. It's literally infallible for `(0,0,0)`.
- **Option B:** Use the const-constructible `NaiveTime::MIN` via `date.and_time(NaiveTime::MIN).and_utc()`. Infallible.
- **Option C:** Change the whole branch to return `Option<TimeRange>` internally, then `.unwrap_or(None)`. More verbose.

**Fix.** Adopt **option B** (most idiomatic chrono code). Replace both the `Range` and `Day` arms:

```rust
// Range arm:
Some(TemporalMark::Range { start, end }) => {
    let s = start.and_time(chrono::NaiveTime::MIN).and_utc();
    let e = end.and_time(chrono::NaiveTime::MIN).and_utc() + Duration::days(1);
    Some(TimeRange { start: s, end: e })
}
// Day arm:
Some(TemporalMark::Day(date)) => {
    let start = date.and_time(chrono::NaiveTime::MIN).and_utc();
    let end = start + Duration::days(1);
    Some(TimeRange { start, end })
}
```

Add `use chrono::NaiveTime;` to the import line (currently only `DateTime, Duration, Utc`).

**Why this matters.** §5.3 is the reference snippet for `iss-024-temporal-dim`. If the implementer copies it literally, CI catches it at compile time — but that's wasted iterations. Better to ship a copy-pasteable snippet.

---

### 🟡 FINDING-r2-2 (minor) — `iss-024-temporal-roundtrip` task has no YAML definition in §9.2

**Location:** §9.2 (line ~1005).

**Problem.** §9.2 mentions `iss-024-temporal-roundtrip` as a bullet at the top but provides a full YAML node definition only for `iss-024-extractor-stub`. The new prep task lacks:
- A YAML block with id/title/description/tags/priority/metadata
- Explicit edges (depends_on, subtask_of, blocks)

When the graph is actually updated (via `gid_add_task`), the maintainer has to invent these from scratch or read §1.1a for context. Small but easy to forget.

**Fix.** Add a second YAML block in §9.2, parallel to the existing `iss-024-extractor-stub` one:

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

Edges to add (explicit list):
- `iss-024-temporal-roundtrip` → `iss-024-design` (depends_on)
- `iss-024-temporal-roundtrip` → `iss-024` (subtask_of)
- `iss-024-temporal-dim` → `iss-024-temporal-roundtrip` (depends_on)
- `iss-024-contract-test` → `iss-024-temporal-roundtrip` (depends_on)

Priority 40 (above `iss-024-extractor-stub`'s 45) because this is a prerequisite-of-prerequisite.

---

## r1 deferred findings — status

Findings 4–13 were explicitly deferred in the r2 revision notes. They remain open and should be addressed in a future revision (r3) or rolled into implementation PR reviews. Listed here for tracking:

- FINDING-4: Range ordering guard
- FINDING-5: `temporal_score` caller inventory
- FINDING-6: Reserved-key `dimensions` coverage
- FINDING-7: `MemoryRecord.metadata` signature verification
- FINDING-8: Backwards-compat wording expansion
- FINDING-9: Extractor-stub task ordering
- FINDING-10: `domain_to_loose_str` round-trip note
- FINDING-11: Perf budget phrasing
- FINDING-12: `two_timer` dep size
- FINDING-13: Revision notes file+line citation

None of these block implementation of `iss-024-temporal-roundtrip` or `iss-024-cli-meta`, both of which are the earliest tasks to start.

---

## Recommendation

**r2 is implementation-ready after FINDING-r2-1 is fixed.** One-line import + two small code-block edits. FINDING-r2-2 is optional (the YAML can be written when `gid_add_task` is called).

Decision path:
- **Path A (recommended):** Fix FINDING-r2-1 inline (no sub-agent needed, it's 3 lines in 2 match arms + 1 import line). Ship as r3 "patch" revision. Start `iss-024-temporal-roundtrip` implementation.
- **Path B:** Declare r2 "good enough" since the bug will be caught by `cargo check` during implementation, ship as-is. Slight risk the implementer wastes 1 iteration on the compile error.

I recommend Path A — the edit is ~60 seconds of work and saves the implementer confusion.
