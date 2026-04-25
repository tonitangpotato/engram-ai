# ISS-032: Cached / Causal Recall Path Produces Broken Confidence Scores

**Status:** 🔴 Open
**Priority:** High — affects every cached recall (the hot path) and every causal recall
**Components:** `crates/engramai/src/memory.rs`, `crates/engramai/src/session_wm.rs`
**Discovered:** 2026-04-10 (originally tracked as RustClaw ISS-007 Bug 3)
**Reporter:** potato + RustClaw (code-level investigation)
**Filed in engram:** 2026-04-25
**Upstream tracker:** rustclaw `.gid/issues/ISS-007/issue.md` (Bug 3 section) — kept open there until this lands

---

## Executive Summary

When `session_recall` takes the cached working-memory path (topic continuous) — and at four other `compute_query_confidence` call sites in causal / cached recall paths — the function is invoked with all three meaningful relevance signals zeroed out. The resulting `confidence` collapses to ~0.05–0.22 regardless of how relevant the cached memory actually is to the query. Every memory in the cached path then surfaces with `confidence_label: "very low"` or `"low"`, which directly harms downstream consumers (RustClaw, autopilot, callers using `min_confidence` filters).

A second, related issue: the cached path also issues a redundant full recall purely to compute a `continuity_ratio` metric — wasted ~50ms per call.

This was originally bundled into RustClaw's ISS-007 (three Engram-recall bugs). The other two bugs lived on the RustClaw side and have been fixed there. This bug is internal to engramai and gets its own issue here.

---

## The Problem

### Site 1 — Cached working-memory path

`crates/engramai/src/memory.rs:4164` (and the equivalent block at 4314 in the causal sibling path):

```rust
let confidence = compute_query_confidence(
    None,        // no embedding similarity
    false,       // not an FTS match
    0.0,         // no entity score
    age_hours,
);
```

`compute_query_confidence` weights (see line 5261):
- Embedding similarity → 0.55 weight → `None` → 0
- FTS match → 0.20 weight → `false` → 0
- Entity overlap → 0.20 weight → `0.0` → 0
- Recency → 0.05 weight → only non-zero contributor

Result: `confidence ≈ 0.05 * recency / 0.45` ≈ 0.11–0.22 for everything in the cached path, regardless of whether the cached memory is a perfect semantic match or barely related.

### Site 2 — Redundant probe for metrics

After the cached path has already returned its results, the code performs a **second full recall** purely to populate the `continuity_ratio` metric:

```rust
// already returned cached results above
let probe = self.recall_from_namespace(query, 3, None, None, namespace)?;
```

This re-runs embedding + FTS + scoring for 3 items per cached call. Pure waste — the metric is informational only, and the prior probe (executed before deciding to take the cached path) already has the data needed to compute the ratio.

### Other affected call sites

`grep -n compute_query_confidence crates/engramai/src/memory.rs` shows five call sites:
- 2954, 3419 — full-recall paths (signals populated, **fine**)
- **4164, 4314** — cached / causal cached paths (signals zeroed, **broken**)
- 4685 — needs audit; likely same pattern as 4164/4314

The bug-fix work needs to inspect 2954 / 3419 to confirm those are healthy reference patterns, then bring 4164 / 4314 / 4685 in line.

---

## Impact

- **Every cached working-memory recall is mislabelled.** Cached recall is the *hot* path — when topic continuity holds, every subsequent recall in the conversation goes through it.
- **`min_confidence` filters drop perfectly relevant memories.** A consumer asking for `min_confidence=0.4` receives nothing from the cached path even when the underlying memories were originally retrieved at 0.7+.
- **`confidence_label: "low"` undermines downstream trust.** RustClaw injects recalled memories into the system prompt with their labels; "[low]" causes the agent to discount memories that are actually highly relevant. Observed in the wild — e.g. potato's recall trace at the top of every conversation shows "[low]" on memories that were the strongest hit in the original full recall.
- **~50ms of wasted latency per cached recall** from the redundant probe, on a path that exists specifically to be fast.
- **Session_recall benefits are partially undone.** ISS-007 Bug 2 (RustClaw side) introduced `SessionRegistry` for proper session isolation, expecting cached recall to deliver fast, trustworthy results. Broken confidence in the cached path defeats half of that win.

---

## Root Cause

The cached path was implemented as a fast lookup by memory ID — it never carries forward the relevance signals (embedding similarity, FTS hit, entity score) that the original full recall computed when it populated the working memory. It then calls `compute_query_confidence` as if a brand-new query were being scored against the cached items, but with no signal data to feed it.

There are two viable shapes for the fix:

1. **Carry-forward**: extend `SessionWorkingMemory` entries to store the original `(similarity, fts_match, entity_score, confidence)` tuple from the recall that populated them. Cached path reuses those values directly. Cheapest CPU; slight memory cost per WM entry.
2. **Re-compute**: on cached lookup, re-run embedding similarity (and optionally FTS) for the cached IDs against the new query. More accurate (the *current* query's similarity, not the original query's), but adds embedding compute back into the hot path — partly defeats the point of the cache.

**Recommended:** Option 1 (carry-forward) for the primary fix. The cached path's premise is "topic is continuous, so similarity from the populating query is still a good proxy". If topic continuity breaks, the path should not have been taken in the first place — that's a job for the continuity check, not the confidence scorer.

---

## Implementation Plan

| Sub-fix | Complexity | Risk | Notes |
|---------|------------|------|-------|
| 3a — Eliminate redundant probe | Low | None | Pure deletion + reuse of pre-existing probe data for `continuity_ratio` |
| 3b — Carry confidence forward via `SessionWorkingMemory` | Medium | Low | New fields on WM entries; populated at full-recall time; consumed at cached-recall time |
| 3c — Audit & align call sites 4314, 4685 | Low | None | Same pattern fix |

Recommended order: **3a → 3b → 3c**. 3a is risk-free cleanup that also makes the perf delta of 3b cleaner to measure.

---

## Files to Modify

- `crates/engramai/src/memory.rs` — call sites at 4164, 4314, 4685; redundant probe (search for the second `recall_from_namespace` call inside the cached branch)
- `crates/engramai/src/session_wm.rs` — extend `SessionWorkingMemory` entry struct to carry similarity / fts_match / entity_score / confidence from the populating recall

---

## Verification

### Sub-fix 3a — Redundant probe
- Benchmark: cached `session_recall` latency before vs after. Expect ~50ms drop.
- Assert `continuity_ratio` value is identical to pre-fix value on a fixed fixture (i.e. metric is computed correctly from the surviving probe data).

### Sub-fix 3b — Confidence carry-forward
- Unit test: full recall populates WM with memories at confidence ∈ {0.85, 0.62, 0.41}. Subsequent cached recall on continuous topic must return the same memories with confidence within ε of those values (allowing only for recency decay between the two calls).
- Regression test: `min_confidence=0.4` filter on cached path returns the same set of memories that the original full recall would have returned with the same filter. Before fix: cached path returns ∅. After fix: returns the high/medium memories.
- Label test: `confidence_label` distribution from a 100-recall session no longer collapses to ≥90% "low/very low".

### Sub-fix 3c — Call site audit
- Code review: 4314 and 4685 use the same carry-forward pattern as 4164.
- Unit tests for causal recall path mirror those of cached path.

---

## Cross-References

- **rustclaw ISS-007** (`/Users/potato/rustclaw/.gid/issues/ISS-007/issue.md`) — original tri-bug report. Bugs 1 & 2 fixed on the RustClaw side. Bug 3 (this issue) was carved out to engram on 2026-04-25.
- **rustclaw `src/memory.rs`** — consumer; once this lands and engramai is published, RustClaw bumps the dep version and ISS-007 closes.

## Next Step

Run a `start_ritual` against engram with this issue. Expect Phase 1 (design) to formalize the `SessionWorkingMemory` schema extension; Phase 2 (implement) to land the three sub-fixes in order; Phase 3 (verify) to prove the cached path now matches full-recall confidence within ε.
