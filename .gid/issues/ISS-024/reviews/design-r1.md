# Design Review: ISS-024 r1

**Document:** `.gid/issues/ISS-024-dimensional-read-path-and-cli-meta-gap/design.md` (993 lines, revision r1)
**Reviewer:** RustClaw (main agent, live-code cross-check)
**Date:** 2026-04-22
**Method:** Full-document read + cross-check against `src/dimensions.rs`, `src/memory.rs`, `src/enriched.rs`, `src/store_api.rs`, `src/query_classifier.rs`, `src/main.rs`.

## Summary

| Severity | Count |
|---|---|
| Critical | 3 |
| Important | 5 |
| Minor | 4 |
| **Total** | **12** |

The r1 revision correctly fixes both of the r0 structural mismatches at the **conceptual** level (nested v2 layout, typed `TemporalMark`). But r1 introduced **new** code-level mismatches while writing the fix, because the `TemporalMark` variant shapes and the legacy read-path behavior weren't cross-checked against live code. Three of those are compile-breaking.

Once FINDING-1..3 are resolved, the design is implementation-ready.

---

## Critical (must fix before implementation)

### FINDING-1 — `TemporalMark::Range` uses `NaiveDate`, not `DateTime<Utc>` (§1.1, §5.3)

**Location:** §1.1 (line ~68), §5.3 match-arm for `Range` (line ~773).

**Problem.** The design declares:

```rust
Range { start: DateTime<Utc>, end: DateTime<Utc> }
```

and §5.3 then writes:

```rust
Some(TemporalMark::Range { start, end }) => {
    Some(TimeRange { start: *start, end: *end })
}
```

Live code (`src/dimensions.rs:231`):

```rust
Range { start: NaiveDate, end: NaiveDate }
```

`*start: NaiveDate` does not satisfy `TimeRange { start: DateTime<Utc>, ... }` (`src/query_classifier.rs:138`). Code as written will not compile. This is the **same class of bug** as r0's "Vec\<String\>" mistake — a shape that was guessed instead of read from source.

**Fix.** Rewrite §1.1 and §5.3 to reflect real shape:

```rust
Some(TemporalMark::Range { start, end }) => {
    // Two NaiveDate days → UTC range covering [start 00:00, end+1 00:00)
    let s = start.and_hms_opt(0, 0, 0)?.and_utc();
    let e = end.and_hms_opt(0, 0, 0)?.and_utc() + Duration::days(1);
    Some(TimeRange { start: s, end: e })
}
```

Also update §1.1's enum citation to match. Note this makes `Range` semantically almost identical to `Day` (both operate at day granularity); the design should acknowledge that rather than describing `Range` as an "already parsed" richer variant.

---

### FINDING-2 — `TemporalMark` JSON shape in §6.1 diagram is externally-tagged; live serde is internally-tagged (§6.1)

**Location:** §6.1 data-flow diagram (line ~813), showing:

```
temporal: { Day: "2026-04-21" },  ← typed!
```

**Problem.** Live code (`src/dimensions.rs:222`):

```rust
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum TemporalMark { ... }
```

Actual on-disk shape of `TemporalMark::Day(NaiveDate(2026-04-21))` is:

```json
{"kind": "day", "value": "2026-04-21"}
```

The design's `{Day: "2026-04-21"}` shape never exists anywhere in the system. This is cosmetic in the diagram, but it leaks into test assertions if someone copies from the diagram.

**Fix.** Update §6.1 diagram and any other references to show real serialized shape:
- `Exact` → `{"kind": "exact", "value": "2026-04-22T10:30:00Z"}`
- `Day` → `{"kind": "day", "value": "2026-04-21"}`
- `Range` → `{"kind": "range", "value": {"start": "2026-04-20", "end": "2026-04-22"}}`
- `Vague` → `{"kind": "vague", "value": "last summer"}`

---

### FINDING-3 — `from_stored_metadata` reads `temporal` as a **string**, not as a typed enum — §5.2 assumption is wrong (§5.2, §1.1 "typed getters")

**Location:** §5.2 `DimensionView::from_record` (line ~649), §1.1 narrative ("Dimensions::from_stored_metadata ... returns a typed Dimensions").

**Problem.** Live code (`src/dimensions.rs:432`):

```rust
let temporal = get_string("temporal").map(|s| {
    crate::enriched::parse_temporal_mark(&s)
});
```

`get_string("temporal")` reads the field as a `&str`. But `build_legacy_metadata` (`src/memory.rs:5362`) serializes the entire `Dimensions` struct, which produces `temporal: {"kind": "range", "value": {...}}` — an **object**, not a string.

Consequence: when a record has `TemporalMark::Range` or `TemporalMark::Exact` or `TemporalMark::Day` written out via v2, reading it back returns `temporal: None`. Only `Vague(String)` round-trips, because `get_string` coerces. The design's §5.3 match arms for `Exact / Range / Day` are **dead code** for any record that went through the disk.

This is a **pre-existing bug in v2 round-tripping** (not introduced by ISS-024), but ISS-024's entire value proposition rests on it working. The design needs to either:

1. **Fix the read path** as part of this issue (add a typed deserialization branch in `from_legacy_metadata`). Extra ~30 LOC in `src/dimensions.rs`. Most honest option.
2. **Acknowledge the gap and scope down §5.3** to handle only the `Vague` variant, with `Exact/Range/Day` deferred to a follow-up. Smaller design, but §1.1's "three of four variants need zero parsing" becomes theoretical (all surviving v2 records become Vague or None once read back).
3. **Add a passing test first** — write a dimensions round-trip test that populates `temporal = Some(Range{...})`, serialize via `build_legacy_metadata`, parse via `from_stored_metadata`, assert `Range` survives. If the test passes, this finding is wrong and the design is fine. If it fails (which I believe it will), follow option 1 or 2.

**Fix.** Add a new subtask `iss-024-temporal-roundtrip` that either fixes the read path (option 1) or explicitly scopes it out (option 2). Either way, §1.1 and §5.3 must reflect the decision. Option 3's test belongs in the design regardless, as a precondition check.

---

## Important (fix before implementation, or document why not)

### FINDING-4 — `TemporalMark::Range` start/end ordering never validated (§1, §5)

**Problem.** Nothing in the design or live code enforces `start <= end` for `TemporalMark::Range`. The §5.3 code as written (after FINDING-1 fix) will produce inverted or empty `TimeRange`s if the extractor ever emits a reversed range. `range_overlap_score` is not shown but is likely also untested against inverted ranges.

**Fix.** Either (a) add a guard in §1.3 `parse_dimension_time` and in the match arm saying "if start > end, treat as None", or (b) add a GUARD-7 saying "malformed range variants are treated as no signal, never crash". The latter is cheaper.

---

### FINDING-5 — §5.5 threading change not scoped to existing callers (§5.5, §6.2)

**Location:** §5.5 "Threading DimParseCache Through the Recall Path" (line ~795), §6.2 LOC table.

**Problem.** The design says "Passes `&mut DimParseCache` to all channel scorers that need it (today: only `temporal_score`)". Live code (`src/memory.rs:3155`) shows `temporal_score` is a free-associated `fn` (no `&self`, no state). Changing its signature to take `&mut DimParseCache` requires updating **every call site**. The design doesn't enumerate these, and the LOC budget in §6.2 shows only `+50/-10` for `src/memory.rs`. Without a call-site inventory, this is a blind estimate.

**Fix.** Grep `temporal_score(` in the codebase, list every caller, include the count in §6.2 and the risk table §10. Estimate LOC more carefully. If there's only one caller (the fusion scorer in `recall_from_namespace`), say so and the current budget is fine.

---

### FINDING-6 — Reserved key list narrowing loses a real key we already reject: `dimensions` (§3.2)

**Location:** §3.2 reserved set table (line ~375).

**Problem.** The design reduces the reserved set to `{engram, user}`, arguing that `engram.*` inner keys are reachable only via `.`-keys which §3.5 already bans. True for writes via `--meta`. But this misses a subtler category: **top-level keys that are NOT `engram` or `user` but are currently being populated by the storage layer**. For example, if any pre-ISS-019 code path writes `metadata.dimensions = ...` at the top level (the v1 layout, which `from_legacy_metadata` still accepts), allowing `--meta dimensions=xyz` would land a user value at `metadata.dimensions`, shadowing the legacy read path.

Live `from_stored_metadata` checks `metadata.engram.dimensions` first, falls back to `metadata.dimensions`. If user writes `--meta dimensions=junk`, `metadata.dimensions = "junk"` at the top level; read path first path succeeds from `engram.*` so the junk is ignored — but only as long as the v2 writer keeps writing `engram.*`. A mixed-layout DB (old rows + new rows) becomes a footgun.

**Fix.** Either:
- Keep `dimensions` in the reserved set as a defensive extra (cheap, documents the v1/v2 dual-layout reality), OR
- Explicitly state in §3.2 that v1 rows are read-only (no new writes produce v1), and cite the code path that guarantees this (`build_legacy_metadata` always emits v2).

Recommend the former. Small list, large safety margin.

---

### FINDING-7 — §5.2 `DimensionView::from_record` uses `record.metadata.as_ref()` but `MemoryRecord.metadata` may not be `Option<_>` (§5.2)

**Location:** §5.2 code block, the `match record.metadata.as_ref()` line.

**Problem.** The design assumes `MemoryRecord.metadata: Option<serde_json::Value>`. I can't verify this from what I read; the one `MemoryRecord` reference I saw (`src/memory.rs:2307 let user_metadata = match row.user_metadata.as_deref()`) suggests user_metadata is stored as `Option<String>`. The on-record metadata field's exact shape is unverified in this review.

**Fix.** Verify `MemoryRecord.metadata`'s real signature (grep `struct MemoryRecord` in `src/`) and adjust §5.2 accordingly. If `metadata` is `serde_json::Value` (not `Option<_>`), the match collapses to a single branch and the `None` arm is dead.

---

### FINDING-8 — §1.8 fallback rule and §7 GUARD-1 describe backwards-compat but don't cover the common "time_range present but all dim temporals unparseable" case

**Location:** §1.8 (line ~218), §7 GUARD-1 (line ~858).

**Problem.** Both say "when dimension_score = 0, output = insertion_score". True for recall calls with **no** time range (§6.1 existing `None` branch) and for records **in range**. But for the interesting regression case — query has time range, record's `created_at` is OUT of range (insertion_score = 0.0), dimension parsing fails (dim_score = 0.0) — the output is `max(0.0, 0.0) = 0.0`. Pre-ISS-024 this record would also get 0.0, so no regression. Good. But the guard wording doesn't make this case explicit.

**Fix.** Expand GUARD-1 or GUARD-5 to include a test case: "when time_range present AND record out of range AND dim unparseable, score is 0.0 identical to pre-ISS-024". One assertion in the contract test covers it.

---

### FINDING-9 — AC-3 contract test references a stubbed extractor but §4.5 says `StubExtractor` lives in a separate prep task (§4.4, §4.5, §9.2)

**Location:** §4.4 (contract test AC-3 assertion, line ~540), §4.5 (line ~573), §9.2 (line ~938).

**Problem.** §4.4's test seeds dimensions via a stubbed extractor. §4.5 acknowledges the stub doesn't exist yet and defines a new task `iss-024-extractor-stub`. §9.2 adds it to the graph. The ordering is right, but the dependency is not transitively visible in §9.1:

```
1. Prep: iss-024-extractor-stub
...
4. After 1,2,3: iss-024-contract-test
```

Nothing in §9.1 or the existing graph edges actually makes `iss-024-contract-test` depend on `iss-024-extractor-stub` at the graph level until §9.2's edges are added. If implementation starts before §9.2's edges land, a contributor could pick up `iss-024-contract-test` first and block.

**Fix.** Either (a) fold the stub into iss-024-contract-test itself (simpler, one task instead of two), or (b) explicitly state in the implementation-order §9.1 that §9.2's graph updates must be applied BEFORE any tier starts.

---

## Minor (nice to fix but not blocking)

### FINDING-10 — `domain_to_loose_str` normalization may drop info in round-trip (§3.8, §6.1)

**Location:** §3.8 merge semantics, §6.1 diagram.

**Problem.** `build_legacy_metadata` (`src/memory.rs:5373`) overwrites `domain` in the serialized `dimensions` object with a loose string form. On read-back, `Domain::from_loose_str` parses it. For `Domain::Other(s)`, the round-trip preserves `s` but loses the variant tag. Not a blocker for this design, but worth mentioning that the v2 layout is NOT a pure serde round-trip — `domain` is special-cased.

**Fix.** One-line note in §3.8: "See `domain_to_loose_str` — `Domain` is re-encoded as a loose string, not strict serde. Other fields round-trip through serde directly."

---

### FINDING-11 — §1.6 performance budget ("<2 ms added to scoring") not measured, stated as assertion

**Location:** §1.6 (line ~203).

**Problem.** The 2 ms number is a guess. With 4096-entry cache and typical LoCoMo queries, it's probably right. But presenting it as a budget without a benchmark citation invites surprise. `two_timer` at 50–200 µs per parse × N candidates (~1K haystack, upper-bound) × worst-case zero cache hit rate = 50–200 ms. That's 25–100× worse than the quoted 2 ms.

**Fix.** Change §1.6 from "Budget: <2 ms" to "Target: <2 ms in steady state (post-warmup); cold-start worst case ≈ 50 ms with 1K candidates and full cache miss". Add to acceptance: "iss-024-temporal-dim includes a micro-benchmark comparing scoring-phase latency pre/post."

---

### FINDING-12 — `two_timer` dep addition and LRU dep addition not justified against existing deps

**Location:** §6.3 deps added.

**Problem.** `lru = "0.12"` is fine. `two_timer` is 1.4 KLoC with five transitives (`pidgin`, etc.). The design dismisses alternatives briefly in §1.2. No mention of whether any of the transitives are already in the tree, or what the total binary-size delta is. For a crate like engram which cares about supply-chain, this is under-documented.

**Fix.** Add a line to §6.3: "Binary size delta measured at ~X KB after release build; transitive additions: pidgin (new), lazy_static (already in tree)." If the author hasn't measured, say "to be measured during iss-024-temporal-dim implementation; accept/reject deferred to that task's PR review."

---

### FINDING-13 — Revision Notes block says "ISS-019 Step 7a v2" but cite should be file+line, not prose

**Location:** Revision Notes r1 (line ~13).

**Problem.** The revision notes cite `src/memory.rs::build_legacy_metadata` and `src/dimensions.rs::Dimensions` but without line numbers. Design docs live alongside code; stale line numbers decay, but file + function name + a one-line excerpt survives moves better than pure prose.

**Fix.** Minor. Add the one-line excerpt inline:

> `build_legacy_metadata` emits `serde_json::json!({ "engram": engram, "user": user })` — two and only two top-level keys.

---

## Summary of required actions

| # | Action | Where |
|---|---|---|
| 1 | Fix `TemporalMark::Range` type in enum citation AND match arm | §1.1, §5.3 |
| 2 | Fix `TemporalMark` serialized JSON shape in diagrams | §6.1 |
| 3 | Decide: fix v2→v1 temporal round-trip in this issue, or scope §5.3 down to Vague-only | §1.1, §5.3, add subtask |
| 4 | Add malformed Range guard (ordering, NaN) | §1, §7 |
| 5 | Enumerate `temporal_score` call sites, update LOC budget | §5.5, §6.2 |
| 6 | Reconsider keeping `dimensions` in reserved-key list | §3.2 |
| 7 | Verify `MemoryRecord.metadata` signature | §5.2 |
| 8 | Expand backwards-compat guard wording | §1.8, §7 GUARD-1 |
| 9 | Move extractor-stub task to run before contract-test, or merge | §9.1, §9.2 |
| 10–13 | Minor wording/budget/citation fixes | as above |

Items 1–3 are compile/semantic blockers. 4–9 are contract-level gaps that will surface in review of the implementation PRs if not addressed now. 10–13 are polish.

**Recommendation.** Apply FINDING-1, FINDING-2, FINDING-3 (option 1 or 2) before starting any tier. FINDING-4..9 can be batched into the same r2 revision. FINDING-10..13 are optional.
