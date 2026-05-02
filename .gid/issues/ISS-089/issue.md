---
id: ISS-089
title: ISS-087 root-fix — store_raw Path A drops meta.occurred_at; CLI bypasses v0.3 pipeline via add_with_emotion_at
status: done
priority: P1
severity: high
tags:
- ingest
- temporal
- store-raw
- regression
- root-fix
- iss-087-followup
related:
- ISS-087
- ISS-019
- ISS-024
relates_to: .gid/issues/ISS-090/issue.md
resolution_note:
- Implementation landed in ISS-087 commit 815b319 (store_raw Path A + Path B both transmit meta.occurred_at; CLI routes --occurred-at without emotion/domain through store_raw full v0.3 pipeline). Regression test at crates/engramai/tests/iss089_occurred_at_roundtrip.rs (5 tests
- all passing). Status was not updated when work landed — confirmed done 2026-05-01. Remaining work (retire add_with_emotion/add_with_emotion_at as deprecated shims) tracked in ISS-090.
---

# ISS-089: ISS-087 root-fix

## Summary

ISS-087 ("Ingest API lacks `occurred_at` override") landed as a **patch, not a root fix**. The implementation:

1. Added `StorageMeta.occurred_at` field ✅
2. Added CLI `--occurred-at` flag ✅
3. **Did NOT** transmit `meta.occurred_at` through `store_raw` Path A → `EnrichedMemory.occurred_at` → `created_at` ❌
4. Instead, added a **new public method `add_with_emotion_at`** that calls `add_raw` (the deprecated v0.2 path), bypassing the v0.3 pipeline (extractor + graph emit + resolution enqueue) entirely ❌
5. Modified CLI `Commands::Store` to branch on `occurred_at_dt.is_some()` and route through this bypass ❌

The acceptance test (`engram store --occurred-at … && engram show` shows correct `created_at`) passes, but the v0.3 substrate is silently noop'd whenever `--occurred-at` is set. This is the exact failure mode that surfaced in RUN-0010 cogmembench replay (and triggered this audit).

`add_with_emotion_at` is **not in ISS-087's scope**. Its existence is unauthorized API growth — a sibling to the deprecated `add_with_emotion` shim that ISS-019 Step 4.5 was supposed to be retiring.

## Evidence

### Path A drops `meta.occurred_at`

`crates/engramai/src/memory.rs:2768` (`store_raw`, Path A — extractor present):
- Line 2905: `let ref_time = meta.occurred_at.unwrap_or_else(chrono::Utc::now);` — used **only** as reference for temporal extractor
- After extractor runs, each `EnrichedMemory` is constructed via `EnrichedMemory::minimal(...)` (line ~2841) or from extracted facts — **`em.occurred_at` is never set from `meta.occurred_at`**
- `store_enriched` → `SqliteKnowledgeStore::insert` → `created_at = em.occurred_at.unwrap_or_else(Utc::now)`
- Result: every fact's `created_at` = wall-clock now, regardless of `meta.occurred_at`

### CLI bypasses store_raw

`crates/engram-cli/src/main.rs` `Commands::Store` handler:
- When `(emotion, domain).is_some() || occurred_at.is_some()` → routes to `mem.add_with_emotion_at(...)` (deprecated `add_raw` path)
- Otherwise → routes to `mem.store_raw(content, StorageMeta { … })`

This means `--occurred-at` users **never go through Path A** — they go through the v0.2-compat `add_raw` path (line 2312), which:
- Skips extractor dispatch
- Skips graph-emit
- Skips resolution-queue enqueue
- Does honor `occurred_at` (line 2465: `let created_at = occurred_at.unwrap_or_else(Utc::now);`)

So `engram store --occurred-at 2023-05-08 …` correctly sets `created_at`, but produces **zero graph nodes, zero facts, zero v0.3 substrate**. RUN-0010 ingested a corpus through this path and got a substrate that looked structurally identical to v0.2.

### `add_with_emotion_at` is out of ISS-087 scope

ISS-087 issue body lists scope as: "no way for a caller to specify the logical event time of a memory". The minimal fix surface is `StorageMeta` + `store_raw` + CLI flag — three touch points. The implementation added a **fourth** unrelated touch point (a new `MemoryManager` public method) that's not mentioned anywhere in ISS-087's ACs.

The Resolution note self-justifies as "zero call-site breakage", but the truly zero-breakage fix is "`store_raw` honors `meta.occurred_at`, CLI keeps calling `store_raw` like always" — which requires no new API surface at all.

## Impact

- **RUN-0010 corpus is invalid for v0.3 evaluation.** Ingested via bypass; no graph substrate. Cannot be compared against RUN-0009 baseline (52.7% recall@5) for ISS-085 J-score work.
- **Any other caller using `--occurred-at` + emotion/domain silently bypasses v0.3.** Same trap.
- **Public API surface bloat.** `add_with_emotion_at` is now a documented method that future code might call, perpetuating the bypass.
- **ISS-019 Step 4.5 ("unify all writes through store_raw") is regressed.** The deprecated `add_with_emotion` shim was supposed to shrink. Instead it gained a sibling.

## Acceptance Criteria

- [ ] **AC-1**: `store_raw` Path A transmits `meta.occurred_at` to every produced `EnrichedMemory.occurred_at` (both extractor-success and `no_facts_extracted` fallback branches).
- [ ] **AC-2**: `store_raw` Path B (no extractor) also honors `meta.occurred_at` (already works via `EnrichedMemory.occurred_at`, but verify).
- [ ] **AC-3**: CLI `Commands::Store` removes the `occurred_at_dt.is_some()` branch — when the user passes `--occurred-at`, the call routes through `mem.store_raw(content, StorageMeta { occurred_at: Some(t), … })` instead of `add_with_emotion_at`. The `(emotion, domain).is_some()` branch (calling `add_with_emotion`) stays as-is for now — its cleanup is **out of scope**, tracked in a follow-up issue.
- [ ] **AC-4**: e2e test `tests/iss_089_occurred_at_roundtrip.rs`:
  - Ingest 1 sentence via CLI with `--occurred-at 2023-05-08T12:00:00Z`
  - Assert `engram show <id>.created_at == 2023-05-08T12:00:00Z`
  - Assert at least 1 row in `nodes` table (graph emit happened)
  - Assert at least 1 row in `extracted_facts` (extractor ran)
- [ ] **AC-5**: RUN-0010 redo — ingest cogmembench conv-26 with new binary, confirm graph nodes exist (sanity), then run RUN-0009's `02_retrieve.sh` and report new recall@5 vs 52.7% baseline. (Validation, not a code AC — but blocks closing.)

## Out of Scope (explicitly)

- **Cleanup of `add_with_emotion` / `add_with_emotion_at` shims.** Both methods continue to exist and continue to call `add_raw` (the deprecated v0.2 path) — `ISS-089` does NOT touch them. Their cleanup (collapsing them to `store_raw` shims, eventual removal) is the natural conclusion of ISS-019 Step 4.5 and tracked separately as **ISS-090**.
- **`StorageMeta.emotion` / `StorageMeta.domain` fields.** Not needed for ISS-089's scope (the `--occurred-at`-only path doesn't carry emotion/domain). These fields are part of ISS-090's design.
- **Fact-level emotion broadcasting semantics.** The "should caller-emotion be a prior, an override, or aggregated" question is part of ISS-090's scope, not here.
- **`add_raw` deprecation/removal.** Tracked elsewhere (ISS-019 followups).

## Implementation Plan (preview)

1. `store_raw` Path A: thread `meta.occurred_at` into all `EnrichedMemory` construction sites (~3 sites: extractor success, no_facts_extracted fallback, error→admit fallback)
2. CLI `Commands::Store`: when `occurred_at` is set, build `StorageMeta { occurred_at: Some(t), … }` and call `store_raw` (instead of `add_with_emotion_at`). The `emotion + domain` branch stays untouched.
3. Write `tests/iss_089_occurred_at_roundtrip.rs`
4. Rebuild → redo RUN-0010 ingest → run retrieve → report numbers

## Why This Is a Real Root Fix (not another patch)

- Fixes the actual omission in ISS-087 (Path A transmission)
- Removes the bypass that hid the omission (CLI branch + `add_with_emotion_at`)
- Restores ISS-019 Step 4.5 invariant ("all writes flow through `store_raw`")
- Reduces public API surface instead of growing it
- Makes future `--occurred-at` callers automatically v0.3-compliant — no foot-gun remains

## Lessons (for retrospective on ISS-087 implementation)

The previous-session implementation took the path of least resistance: it noticed the CLI already had a branch for `add_with_emotion`, so it copy-pasted that branch + added an `_at` suffix variant. That made the AC test pass without touching `store_raw` Path A — which was the actually-hard-but-correct change.

Signs that should have caught this at review time:
- A new public method appeared that wasn't in the issue's ACs
- The test verified `created_at` but not graph-substrate side effects
- The Resolution note self-justified ("zero call-site breakage") instead of pointing to the simpler `store_raw`-only diff

Captured for `.gid/lessons/` after closure.
