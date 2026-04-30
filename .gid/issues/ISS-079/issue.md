---
title: Episodic plan over-downgrades on queries without time window
status: open
priority: P2
filed: 2026-04-30
filed_by: rustclaw
labels:
- retrieval
- episodic-plan
- fallback
- downgrade
---

# Episodic plan over-downgrades on queries without time window

## Symptom

When a query has no `time_window` extracted (most queries don't carry explicit temporal anchors), the `Episodic` retrieval plan immediately exits with `DowngradedFromEpisodic` and returns 0 memories. This blanks out the episodic sub-plan inside `Hybrid`, contributing to Hybrid hit@5 = 0/2 in eval.

## Evidence

- Diagnostic session 2026-04-30 traced Hybrid plan execution on cat=4 queries: episodic sub-plan returns 0 items because no time_window was set on the query.
- `episodic.rs::execute()` current behavior: if `time_window.is_none()` → return `DowngradedFromEpisodic` empty result.
- Together with ISS-078 (L5 compiler not wired), this fully explains Hybrid 0/2 in RUN-0008.

## Root cause

The `execute()` function treats `time_window` as a hard requirement. There's no soft-fallback path for the common case where the user didn't say "last week" or "yesterday" but still wants recent episodic chunks.

## Expected behavior

If `time_window.is_none()`, fall back to filtering by entity/namespace and return recent chunks (e.g., top-K most recent within the namespace, with normal activation/relevance scoring). The result should be a soft fallback, not a hard downgrade.

If `time_window.is_some()` but yields no matches, current behavior (downgrade) is fine.

## Fix sketch

In `crates/engramai/src/retrieval/plans/episodic.rs::execute()`:

```
if time_window.is_none() {
    // soft fallback: entity/namespace filter + recency scoring
    return execute_no_window_fallback(...);
}
// existing path
```

The `execute_no_window_fallback` should:
- Use the same activation/relevance scoring path as Factual plan
- Filter to namespace + entity matches if those signals are present
- Return a non-empty result when the namespace has any memories at all

## Acceptance criteria

- Eval queries without explicit time_window produce non-empty episodic plan results when the namespace has matching content.
- RUN-0008-style eval Hybrid hit@5 improves (in conjunction with ISS-078 fix).
- No regression on queries that DO have time_window — those should behave identically to today.

## Out of scope

- Time-window extraction quality (separate concern in temporal reasoning track)
- Episodic plan ranking algorithm — only the no-window fallback path is in scope

## Related

- relates_to: ISS-078 (L5 compiler wiring; together these explain Hybrid 0/2)
- substrate: RUN-0008 / locomo-conv26-iss076
