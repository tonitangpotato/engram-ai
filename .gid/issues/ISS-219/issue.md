---
id: ISS-219
title: recall tool doesn't expose memory_id — confabulated/poisoned memories cannot be targeted for deletion
kind: issue
status: todo
priority: P1
labels:
- engram
- recall
- deletion
- epistemic-hygiene
- data-integrity
created: 2026-06-10
depends_on: .gid/issues/ISS-220/issue.md
relates_to: .gid/issues/ISS-065/issue.md
---

# ISS-219: recall doesn't expose memory_id → poisoned memories are undeletable

## Severity

**P1.** This breaks the core remediation loop for memory poisoning. When an agent
(or its human) discovers a memory is wrong/poisoned, the only correct fix is to
**delete that specific memory**. Today that is impossible through the supported
interface, because the agent cannot obtain the `id` of the memory it just recalled.
The only available workaround (content-substring SQL) **does not work** (see ISS-220),
so wrong memories accumulate with no clean removal path.

## Concrete failure (2026-06-10, trader namespace)

The agent had repeatedly stated a false claim to potato:

> "potato's long-call history is an all-loss anti-pattern: COIN/ARM/TSM all −100%, EQT −63%"

Ground truth (verified from `trader.db` `positions`/`position_legs`):
long calls are **net +$2,918** (SLV 47C +$1,036.63 ✅, NVDA 180C +$1,882.05 ✅,
EQT 60C −$324.01 ❌). The fuller directional-options ledger is **+$7,912, 11W/5L**.
The "COIN/ARM/TSM all-loss" framing is a *subset* (single-name crowded high-vol naked
calls) being confabulated as the *whole* book — ignoring the SPY/IWM/SPX/SLV/NVDA winners.

potato has corrected this **multiple times**. Each time the agent stored a correction,
but the **poisoned memory was never removed**, and on later `recall` it came back with
*higher* confidence (0.76–0.78) than the correction (0.65), so the agent re-emitted the
error. This is exactly the "repeatedly reconstructing the same error" failure that the
`cite-before-claim` skill warns about — but here the agent *did* try to verify and *did*
try to delete, and **the tooling blocked the fix**.

## Root cause

`engram_recall` returns results as an **ordered, rendered list** — each item shows
`[Type] (confidence: X.XX)` + rendered content, but **never the `id` field** of the
underlying memory row. So an agent that wants to delete "the memory I just recalled"
has no handle to pass to `engram_forget(memory_id=...)`.

The agent is forced to *reverse-look-up* the id by content substring via raw SQL —
which fails for a second, independent reason (ISS-220: stored `content` is not
byte-matchable; every characteristic phrase of the recalled text — "全损反模式",
"账本里最痛", "权威更正", "短一点的call" — returned **zero** rows from
`SELECT ... WHERE content LIKE '%phrase%'`). With no id and no working substring search,
**deletion is impossible without blind-guessing a row**, which violates the
"never delete without confirming the exact id" safety rule.

## Why a correction-store is NOT sufficient

The agent's fallback was to store a high-importance (1.0) correction and rely on it
out-ranking the poison in future recall. This is *mitigation, not a fix*:

1. It leaves the poisoned row live; ranking is probabilistic and decay/consolidation
   can re-elevate the poison later.
2. It bloats the store with contradicting pairs instead of removing the bad data.
3. The agent could not even pin the correction (the `UPDATE ... SET pinned=1`
   matched 0 rows for the same byte-match reason as ISS-220).

## Proposed fix

1. **`engram_recall` must return a stable `id` for every result item** (alongside
   type/confidence/content). Minimal change, unblocks the whole deletion loop.
2. Optionally add a `recall --with-ids` / structured-output mode so the chat-facing
   render stays clean but the agent can request ids.
3. Once ids are exposed, `engram_forget(memory_id, confirm=true)` already exists and
   closes the loop. Verify it works end-to-end on a recalled id.
4. **Acceptance:** agent recalls a memory → obtains its id from the recall result →
   passes it to `engram_forget` → memory no longer appears in subsequent recall.

## Relationship to other issues

- **ISS-220** (content not byte-matchable) is the *second* reason deletion is blocked.
  Either one alone breaks the loop; both together make it impossible. They should be
  fixed together to fully restore the discover→verify→delete remediation path.
- **ISS-065** (pre-LLM claim verification) is upstream prevention; this issue is the
  *cure* path when prevention fails and a bad memory already exists.

## Acceptance criteria

- [ ] AC-1: `engram_recall` result items include a stable, deletable `id`.
- [ ] AC-2: An agent can take an id straight from a recall result and delete that
      memory via `engram_forget` with no raw-SQL reverse lookup.
- [ ] AC-3: After deletion, the same query no longer surfaces the deleted memory.
- [ ] AC-4: Regression test reproduces the 2026-06-10 loop: store poison → recall →
      grab id → forget → re-recall returns only the correction.
