# ISS-024: Dimensional Metadata Side-Channel — Half-Wired End to End

**Status:** investigation (root-cause complete, fix pending approval)
**Severity:** high — the dimensional pipeline is architecturally present but functionally dead on both ends. Every upstream improvement (ISS-019 write coverage, ISS-021 subdim coverage, ISS-022 schema) lands in a dimension store that the retrieval layer never reads, and the CLI surface needed to feed dimensions in from external adapters is not even exposed.
**Feature:** dimensional-extract × cli × retrieval (cross-cutting)
**Related:**
  - ISS-019 (dimensional-metadata-write-gap) — fixed *internal* write path; this issue fixes *external adapter* write path
  - ISS-020 (kc-dimensional-awareness) — KC ignores dimensions; this issue shows retrieval has the same blindness
  - ISS-021 (subdim-extraction-coverage) — even when dims are stored, they must also be *consumed*
  - ISS-022 (dimension-vec-string-schema) — schema refactor; orthogonal but touches the same fields
**Blocks:** ISS-020 Phase B (dim-aware ranking) cannot deliver LoCoMo gains until retrieval actually reads dimensions.
**Blocked by:** nothing — can start immediately.

---

## TL;DR

The dimensional metadata side channel is **half-wired end to end**. By design, dimensions flow:

```
caller/adapter ──(--meta)──▶ CLI ──▶ store_api ──▶ memory.metadata.dimensions ──▶ recall channels ──▶ ranking
                   ❌ A                                    ✅ (ISS-019 fix)             ❌ B
```

Both ends are broken:

- **Gap A (write surface):** `engram store` CLI has **no `--meta` / `--metadata` argument**. Any adapter that follows the documented protocol (`engram store "..." --meta dia_id=D1:3 --meta turn=5`) gets `rc=2` from clap and writes **nothing**. The adapter silently continues and reports "success", because neither side checks rc.
- **Gap B (read surface):** The 7 recall channels inside `recall_from_namespace` (embedding / FTS / entity / temporal / somatic / activation / confidence) operate entirely on `content`, `entities` table, and `created_at`. **Zero channels consult `metadata.dimensions.*`**. `temporal_score` uses `record.created_at`; nothing uses `dimensions.temporal`. There is no `dimension_match` channel.

Net result: every downstream issue (019/020/021/022) has been debugging the wrong end of a pipe whose *consumer* was never plumbed in. Fixing write coverage to 100% changes nothing the ranker sees.

**This is the root cause of the "Rust unit tests all green, real pipeline broken" class of bug we've hit three times now.** Unit tests exercise each stage in isolation. No test exercises adapter → CLI → storage → recall → ranking end to end with dimensions.

---

## Evidence

### Gap A — CLI missing `--meta`

`src/main.rs:73-116`, the `Store` subcommand definition:

```rust
/// Store a new memory
Store {
    content: String,
    #[arg(long, short = 'n', default_value = "default")] ns: String,
    #[arg(long, short = 't', default_value = "factual")] r#type: MemoryTypeArg,
    #[arg(long, short = 'i')] importance: Option<f64>,
    #[arg(long, short = 's')] source: Option<String>,
    #[arg(long, short = 'e')] emotion: Option<f64>,
    #[arg(long)] domain: Option<String>,
    #[arg(long, env = "ENGRAM_EXTRACTOR")] extractor: Option<ExtractorArg>,
    #[arg(long, env = "ENGRAM_EXTRACTOR_MODEL")] extractor_model: Option<String>,
    #[arg(long, env = "ANTHROPIC_API_KEY")] auth_token: Option<String>,
    #[arg(long)] oauth: bool,
},
```

There is no `meta`, no `metadata`, no pass-through `#[arg(last = true)]`. A call of the form

```
engram store "Alice met Bob at 3pm" --meta dia_id=D1:3 --meta participants=Alice,Bob
```

exits with clap error `rc=2` ("unexpected argument `--meta`"). Benchmark adapters and the LoCoMo pipeline invoke this path; their stdout/stderr capture is not being rc-checked, so a multi-hour ingestion job can silently produce memories with **zero caller-supplied dimensions**.

The internal Rust API (`Memory::remember(content, memory_type, importance, metadata: serde_json::Value)`) accepts arbitrary metadata and the write path at `src/memory.rs:1320-1345` correctly merges it. The plumbing stops one layer above, at the CLI ⇄ adapter boundary.

### Gap B — Retrieval channels ignore `dimensions`

`src/hybrid_search.rs:1-50` — header of the hybrid search module:

> Adaptive Hybrid Search — combines vector similarity with FTS for optimal retrieval. Uses both embedding-based semantic search and FTS5 keyword search, combining scores with adaptive weights based on result overlap.

No mention of dimensions. `grep -n dimensions src/hybrid_search.rs` → **zero matches**. The scoring loop reads `memory.content` and `memory.id`; dimensions are not even fetched.

`src/memory.rs:3154-3187` — `temporal_score`:

```rust
fn temporal_score(
    record: &MemoryRecord,
    time_range: &Option<crate::query_classifier::TimeRange>,
    now: chrono::DateTime<chrono::Utc>,
) -> f64 {
    match time_range {
        Some(range) => {
            if record.created_at >= range.start && record.created_at <= range.end { ... }
            else { 0.0 }
        }
        None => { /* sigmoid on age */ }
    }
}
```

Reads only `record.created_at`. The whole point of writing `dimensions.temporal = ["yesterday afternoon"]` during extraction was to let temporal queries match records whose *semantic* time ≠ *insertion* time. That cross-walk is never done.

`recall_from_namespace` in `src/memory.rs:2711+` wires together 7 channels; none of them touch `dimensions`. Ranking is therefore identical whether a memory has rich dimensions or empty ones. Coverage improvements in ISS-019 / 021 / 022 have **no ranking consequence**.

### Why unit tests missed this

`engram-ai-rust/src/` has:

- Unit tests for `extractor.rs` → verify fact JSON shape
- Unit tests for `memory.rs::remember` → verify metadata merges
- Unit tests for `hybrid_search.rs` → verify vector + FTS combination
- Unit tests for `temporal_score` → verify `created_at` range logic (see `test_temporal_score_within_range` at `src/memory.rs:5886`)

Each stage passes in isolation. **No test invokes `engram store ... --meta ...` via a shell, reads back via `recall`, and asserts that the supplied dimension biased the ranking.** There is no contract test that crosses the CLI boundary.

---

## Root Cause

The dimensional side channel was specified as an architectural concept (see ISS-020 §1 "Terminology: Two Kinds of Metadata") but implemented only in the middle tier:

| Tier | Status |
|---|---|
| Spec / adapter protocol | ✅ defined (`--meta key=value` from ISS-019 design) |
| CLI surface (`src/main.rs` Store) | ❌ **missing** — this issue, Gap A |
| Rust API (`Memory::remember`) | ✅ accepts `metadata: serde_json::Value` |
| Storage (`src/memory.rs:1320` merge + SQLite persist) | ✅ correct (ISS-019 landed) |
| Extractor-derived dimensions (`src/memory.rs:2149`, 2177) | ✅ merged into same blob |
| Recall SQL / join against dimensions | ❌ **missing** — no JSON extract on `metadata.dimensions.*` |
| Ranking channels (`recall_from_namespace` 7 channels) | ❌ **missing** — this issue, Gap B |
| KC snapshot load | ❌ missing (ISS-020) |

The middle three rows are healthy. The two outer rows — the surfaces that actually connect engram to **callers** (write side) and to **queries** (read side) — are the exact places we haven't wired. This is a recurring "donut" pattern: solid middle, hollow edges. Every debugging session so far has zoomed into the middle.

## Fix Plan

Three interlocking changes; doing any one alone leaves the pipe dead.

### Change 1 — CLI `--meta` surface (Gap A)

In `src/main.rs` `Store` enum:

```rust
/// Caller-owned metadata key=value pairs. May be repeated.
/// Passes opaque to engram; merged into memory.metadata at top level (alongside `dimensions`).
#[arg(long = "meta", value_parser = parse_kv, action = clap::ArgAction::Append)]
metadata: Vec<(String, String)>,
```

Plus a `parse_kv` helper that splits once on `=`, validates non-empty key, returns `Result<(String, String), String>`. At dispatch time, fold the `Vec<(String, String)>` into `serde_json::Value::Object`, using string values (no automatic JSON parsing — if a caller wants structured values, they use a separate `--meta-json` flag; scope of this change is the key=value happy path).

Corresponding update: `engram store --help` docstring (module-level doc comment at `src/main.rs:1-6`) must document the flag and the "caller-owned side channel" contract.

### Change 2 — Adapter rc check (defense in depth)

Wherever `engram store` is invoked by an adapter — benchmark runners, LoCoMo ingestion, the RustClaw agent skill — the adapter must:

1. Check `cmd.status.success()` and fail loudly on non-zero exit.
2. Log the exact CLI invocation on failure (especially the `--meta` pairs) for forensic replay.
3. A dry-run mode that prints the resolved command without executing, so schema mismatches surface pre-ingestion.

This is preventative; Change 1 fixes the current incident, but if a future CLI flag gets renamed and adapters silently fall back to rc=2 again, we should catch it in one record, not ten thousand.

### Change 3 — Read path consumes `dimensions` (Gap B)

Two sub-options, pick one based on LoCoMo ablation; **this design doc does not pick, it scopes both**:

**Option 3a — Extend `temporal_score` to also read `dimensions.temporal`**
- When `time_range` is present, compute two scores: one from `record.created_at` (as today) and one from parsing `record.metadata.dimensions.temporal` as a natural-language time expression against the same range.
- Take `max(insertion_time_score, dimension_time_score)` — best-available signal wins.
- Minimal, surgical, backwards-compatible. Only helps temporal queries.

**Option 3b — New `dimension_match` channel (8th channel)**
- Query-time: run the same extractor on the query to get `query_dimensions: ExtractedFact`.
- For each candidate, compute per-dimension overlap (participants ∩, relations overlap, etc.) → one score per dimension → weighted sum → channel score in [0, 1].
- Enter with weight 0 behind a feature flag; run A/B on LoCoMo against activation-only ranking.
- Bigger change, bigger potential gain. This is what ISS-020 Phase B is supposed to deliver.

Recommended order: ship Option 3a first (low risk, makes the pipe live end-to-end), then Option 3b as ISS-020 Phase B.

### Change 4 — End-to-end contract test

A single integration test that:

1. Spawns `engram store` with `--meta dia_id=D1:3`, LoCoMo-style content, and the extractor enabled.
2. Asserts `rc=0`, memory is persisted, `metadata.dimensions.participants` non-empty, `metadata.dia_id == "D1:3"`.
3. Runs `engram recall "who was at the meeting"` with the same namespace.
4. Asserts the memory from step 1 is in the top-3 results **and** its score benefits from dimension match (compare against a second memory with empty dimensions).

This test is the contract. If it passes, the pipe is wired. If it ever fails, we've broken the dimensional side channel again.

## Acceptance Criteria

- [ ] AC-1: `engram store "x" --meta foo=bar` returns rc=0 and the memory has `metadata.foo == "bar"`.
- [ ] AC-2: `engram store` with both extractor and `--meta` preserves both top-level caller keys and `metadata.dimensions.*` without collision. Caller key named `dimensions` is rejected with a clear error (reserved namespace).
- [ ] AC-3: `recall_from_namespace` returns identical ranking when `dimensions` are empty (backwards compat), and higher rank for dimension-matched candidates when they are populated.
- [ ] AC-4: The end-to-end integration test from Change 4 is in CI and passes.
- [ ] AC-5: Adapter RC check is added to the two known call sites (LoCoMo bench runner, RustClaw skill). Any new adapter must also check.

## Out of Scope

- Separating caller-opaque metadata from engram-owned dimensions into **physically separate SQLite columns** (deferred; ISS-020 §1.3 notes this as a parallel design).
- Migrating existing databases to backfill dimensions for records stored during the Gap A era. If needed, a separate one-shot `engram maintenance backfill-dimensions --from-content` tool.
- Query-side dimension extraction cost/latency budget — handled in ISS-020 Phase B when option 3b ships.
- Changing `ExtractedFact` field types (ISS-022).

## Risk

- **Low:** Change 1 is additive; Change 2 is defensive logging; Change 3a is a max() of two scores.
- **Medium (3b):** Running the extractor on every query adds latency. Must be cached / gated by feature flag.

## Implementation Order

1. Change 1 (CLI `--meta`) — 1 commit, ~50 LOC including tests
2. Change 4 (contract test) — immediately after Change 1 so it fails, then passes
3. Change 3a (temporal_score uses `dimensions.temporal`) — 1 commit, ~30 LOC
4. Change 2 (adapter rc checks) — sweep commit across adapter call sites
5. Change 3b — new issue (continuation of ISS-020 Phase B)

---

## Signed

- Discovered by: RustClaw, root-cause trace 2026-04-22 evening
- Issue opened: 2026-04-22 20:54 ET
- Approver: potato (pending)
