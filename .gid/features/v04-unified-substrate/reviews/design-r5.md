# design-r5 spot-check — commit 8

**Scope:** 5 changed regions in commit fd98ff9 (closing r4 F1+F2+F3+F4+F5).
**Prior review:** design-r4.md
**Doc:** design.md (2232 lines)
**Date:** 2026-05-12

## Summary
- Critical: 0
- Important: 1
- Minor: 2
- All r4 findings closed: PARTIAL

**Status: ✅ All 3 findings applied in commit 9 (2026-05-12).**
- FINDING-1: ✅ Applied — rewrote §4.13 first bullet to defer to "next bullet" for the precise trigger; no longer contradicts the metacog-cycle definition.
- FINDING-2: ✅ Applied — `s/in_episode/belongs_to_episode/` in 3 new occurrences (§4.15.6 row, §6.1 WriteMemory field comment, §6.1 macro-op step 4). `grep -n "in_episode"` now returns 0 hits.
- FINDING-3: ✅ Applied — §6.9 paragraph 1 softened: removed "is NOT extracted by the supervisor" claim, replaced with "the supervisor does not consume or short-circuit it" + explicit forward-reference to the wrap mechanism in paragraph 3. Paragraph 3 unchanged. Forwarding cost note bumped from "zero allocation" to "one heap allocation" to match the wrap.

## Region-by-region verdict

### Region 1: §4.13 Cold→warm transition (r4 F3)
- r4 F3 closed: YES — transition trigger, read-only semantics, and rationale are now unambiguous.
- New issues: FINDING-1 (Important) — the new bullet (line 746) contradicts the pre-existing bullet (line 745) on what event triggers the cold→warm flip.

### Region 2: §4.15.6 Write-amp (r4 F2 partial)
- r4 F2 (write-amp accounting): YES — `in_episode` row added, totals and headroom recomputed correctly (12 P50 / 25 P95 / 8.8× headroom — math checks out).
- New issues: FINDING-2 (Minor) — `in_episode` predicate name doesn't match `belongs_to_episode` used in §3.2/§4.1/§4.10/§7.4.

### Region 3: §6.1 WriteMemory (r4 F2 + F4)
- r4 F2 (episode_id field): YES — `episode_id: Option<NodeId>` added to WriteMemory, macro-op step 4 added, `episode_edge: Option<EdgeId>` in WriteMemoryReply.
- r4 F4 (memory_type + layer): YES — both fields added to WriteMemory variant, caller-responsibility prose + defaults documented, macro-op step 1 updated.
- New issues: FINDING-2 (same as Region 2 — predicate name drift `in_episode` vs `belongs_to_episode` in step 4 and field comment).

### Region 4: §6.9 Supervisor (r4 F1)
- r4 F1 closed: YES — direct-send model committed, typed-closure crash-notifier map specified, `Arc<Mutex<Option<…>>>` sharing semantics documented, OpDone tick channel for completion signaling. Architecture is sound and race-free.
- New issues: FINDING-3 (Minor) — §6.9 first paragraph says sender "is NOT extracted by the supervisor" but third paragraph describes the supervisor extracting, wrapping, and rebuilding the variant. The architecture described in paragraph 3 is correct; paragraph 1's claim is misleading.

### Region 5: §8.11 T51 + T66 (r4 F3 + F5)
- r4 F3 (T51): YES — T51 now includes `cold_start | warm` initialization, transition triggers matching §4.13, and wm_snapshot capture.
- r4 F5 (T66): YES — T66 rewritten to match §6.9 direct-send + typed-closure architecture. Terminology consistent with §6.9.
- New issues: none (T51 inherits FINDING-1's contradiction from §4.13 but the fix is in §4.13, not T51).

## Findings

### FINDING-1 [Important]: §4.13 cold→warm transition trigger contradicts between adjacent bullets

- Region: 1 (§4.13)
- Location: design.md §4.13 (lines 745–746)
- Lines in conflict:
  - Line 745 (pre-existing, commit 7): "The flag flips to `warm` after the **first recall populates the buffer** (i.e. the first time WM holds at least one memory ID derived from this session's activity, not a stub)."
  - Line 746 (new, commit 8): "the transition fires the **first time the session's metacog loop completes its first cycle after the session opens**, OR when a `wm_snapshot` from a prior session is loaded back into the ring buffer (session resume)."
- Issue: These are two different events. "First recall populates the buffer" (a retrieval event) ≠ "metacog loop completes its first cycle" (a metacog evaluation event). Metacog cycles run asynchronously on a timer and may fire well after the first recall has already populated WM. An implementer reading both bullets cannot determine which event triggers the transition. T51 (Region 5) correctly follows the commit-8 bullet (metacog cycle), making the commit-7 bullet the stale one — but it hasn't been updated.
- Suggested fix: Update the line-745 bullet to remove the specific trigger, deferring to the precise bullet. Replace "The flag flips to `warm` after the **first recall populates the buffer** (i.e. the first time WM holds at least one memory ID derived from this session's activity, not a stub)." with "The flag flips to `warm` on the first qualifying event (see **Cold→warm transition timing** below)." This keeps the overview bullet and avoids contradiction.

### FINDING-2 [Minor]: Episode edge predicate name drift — `in_episode` vs `belongs_to_episode`

- Region: 2 (§4.15.6) + 3 (§6.1)
- Location: design.md §4.15.6 line 891 (`in_episode` edge), §6.1 line 1201 (`in_episode`), §6.1 line 1413 (`containment / in_episode`)
- Issue: Commit 8 introduced the predicate name `in_episode` in three places. The existing design uses `belongs_to_episode` consistently in four places: §3.2 edge taxonomy table (line 330), §4.1 pseudocode (line 439), §4.10 prose (line 638), and §7.4 (line 1843). These are two different predicate strings and would produce different values in the `edges.predicate` column, breaking queries that filter on one but not the other.
- Suggested fix: Replace all three occurrences of `in_episode` (lines 891, 1201, 1413) with `belongs_to_episode` to match the established predicate name in §3.2.

### FINDING-3 [Minor]: §6.9 self-contradicts on whether supervisor extracts the oneshot::Sender

- Region: 4 (§6.9)
- Location: design.md §6.9 (lines ~1742 vs ~1744)
- Lines in conflict:
  - Line ~1742: "The writer keeps the original `oneshot::Sender` inside the `WriteOp` variant (direct-send model) — it is NOT extracted by the supervisor."
  - Line ~1744: "the supervisor extracts each op's raw `oneshot::Sender<R>` from the WriteOp variant, wraps it in `Arc<Mutex<Option<oneshot::Sender<R>>>>`, and rebuilds the variant with the wrapped slot before forwarding."
- Issue: The sender IS extracted, wrapped, and replaced. The first paragraph's "NOT extracted" claim describes the *public API surface* (callers pass raw senders), not the internal forwarding path. An implementer reading paragraph 1 alone would skip the Arc/Mutex wrapping entirely; reading paragraph 3 alone gives the correct architecture. The contradiction is cosmetic (paragraph 3 is unambiguous and complete) but could waste 30 minutes of implementation time.
- Suggested fix: Amend the first paragraph to: "**The writer keeps the `oneshot::Sender` inside the `WriteOp` variant** (direct-send model) — the supervisor wraps it in `Arc<Mutex<Option<…>>>` on the forwarding hop (see below) but the writer still performs the reply directly, with zero supervisor involvement on the happy path."

## Recommendation
- [x] Apply 3 fixes in commit 9 before T01
- [ ] Re-review needed (only if Critical found)

All three findings are small prose edits (≤5 lines each). No architectural changes needed. The §6.9 supervisor design is sound — the `Arc<Mutex<Option<…>>>` + `take()` pattern correctly handles the writer-vs-supervisor race (mutual exclusion via Mutex, at-most-once delivery via Option::take, idempotent cleanup via the OpDone tick lag tolerance). No deadlock risk (single lock, no nested locking, lock held only for the duration of a `take()` + `send()`). The OpDone tick lag is explicitly documented as safe (stale entries are no-ops). Implementation-ready after the 3 prose fixes.
