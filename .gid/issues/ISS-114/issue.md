---
id: ISS-114
title: Decide fate of store_with_pre_extracted WIP (extractor-outside-lock optimization)
status: open
priority: P2
labels: [tech-debt, concurrency, decision-needed]
created: 2026-05-13
related: [ISS-068 (resolved — NoFactsExtracted blocks raw admission)]
---

# Decide fate of `store_with_pre_extracted` WIP

## What it is

A set of uncommitted changes to `crates/engramai/src/memory.rs` adds two new
methods plus a helper type:

- `Memory::take_extractor() -> Option<Box<dyn MemoryExtractor>>`
- `Memory::store_with_pre_extracted(&mut self, content, extraction_result, meta)`
- `struct PrecomputedExtractor` — one-shot extractor returning a pre-computed result

The design intent (per the doc comments): let `MemoryManager` hold the
extractor **outside** the `Memory` mutex, run the LLM extraction call
outside the lock (avoiding multi-second lock-hold during Anthropic calls),
then re-enter the lock with `store_with_pre_extracted(content,
Some(Ok(facts)), meta)` for the DB writes only.

Full diff: `/tmp/iss114-wip.diff` (79 lines, captured 2026-05-13). Also
present in stash — see "Where it lives" below.

## How it got here

- Discovered 2026-05-13 in `git status` while shipping T28 (Phase D flag).
- `git log --grep="store_with_pre_extracted"` returns nothing — the diff
  was never committed.
- Doc comments reference ISS-068 (resolved 2026-04-29 in commit 925800b)
  but only as context ("facts may be empty (ISS-068)"), not as the
  motivating fix. ISS-068's actual fix landed in `6f5821a fix(ingestion):
  persist raw memory when extractor returns no facts` — a different
  change entirely.
- No other call site references the new methods (`grep -r
  store_with_pre_extracted crates/` finds only the definitions).
- No issue or design doc anywhere in `.gid/` references this work.

Best guess: a prior session started a concurrency optimization, didn't
finish wiring `MemoryManager` to use it, and the work sat in the working
tree across sessions. Not verified — could also be that the design was
rejected mid-stream and the diff is dead weight.

## Why decide now

Working-tree dirt costs every future session: `git add -A` flows have to
hand-pick files, and the next agent stumbles into the same "what is this,
should I commit it" loop. Today it cost ~30 minutes on T28.

## The three options

### (i) Finish it — ship the optimization

Real evidence needed first:
- A benchmark showing measurable lock-hold time on `store_raw` when
  extraction is enabled (LLM round-trip likely 500ms–3s while holding
  the `Memory` mutex)
- Concurrent-write workload that would benefit (single-agent batch
  ingestion? multi-agent shared substrate?)
- `MemoryManager` audit: does it actually hold a `Mutex<Memory>` today?
  Or is the lock per-call?

If (i) is chosen: wire `MemoryManager::store` to call `take_extractor()`
→ run extraction → call `store_with_pre_extracted`. Write a concurrency
regression test (two threads, one slow extractor, assert lock-hold time
≤ DB-write time, not extraction time).

### (ii) Drop it — design was wrong or superseded

Possible reasons to drop:
- v0.4 unified-substrate Phase D/E may change the writer architecture
  enough that this optimization no longer makes sense (see §6 Single
  Writer pattern in `.gid/features/v04-unified-substrate/design.md`).
- The "swap extractor + delegate to store_raw" trick is clever but
  fragile — `PrecomputedExtractor` panics if `extract` is called more
  than once, and the swap-back logic in `store_with_pre_extracted` is
  not exception-safe (if `store_raw` panics, the original extractor is
  lost).
- If extraction is rare (only on store path, not on recall), the
  amortized cost may not justify the API complexity.

### (iii) Partial — keep `take_extractor`, drop the rest

`take_extractor` is a small, useful primitive: it lets external code
move the extractor without unsafe gymnastics. The `store_with_pre_extracted`
+ `PrecomputedExtractor` combo is the part that's clever-and-fragile.

If (iii): commit `take_extractor` alone as a public helper, document
that callers must restore the extractor (or accept its loss), and let
future concurrency work build on top without committing to the
precomputed-extractor mechanism.

## What's needed to decide

1. Read `MemoryManager` (or whatever owns the `Mutex<Memory>` today).
   Is it really holding the mutex during extraction? If not, the
   optimization is solving a non-problem.
2. Look at the §6 Single Writer design in v04-unified-substrate — does
   it subsume this work?
3. Run a quick bench: 100 stores with a fake slow extractor (sleep 1s),
   measure wall clock with vs. without `store_with_pre_extracted`. If
   the delta is <20%, (ii) is correct.

None of these are urgent — Phase D/E will probably answer (2) implicitly.

## Where it lives

After this issue is filed, the diff will be moved out of the working
tree via:

```
git stash push -m "ISS-114: store_with_pre_extracted WIP — see issue body" \
    -- crates/engramai/src/memory.rs
```

Stash message references this issue ID. Recovery: `git stash list` →
find the entry tagged ISS-114 → `git stash apply <id>` to restore the
diff, or `git stash drop <id>` once a decision is made and the work is
either committed or rejected.

## Acceptance (when one of the three options is chosen)

- [ ] Option (i): MemoryManager wired, concurrency regression test, lib
      green
- [ ] Option (ii): stash dropped, this issue closed with reason
- [ ] Option (iii): `take_extractor` extracted into its own commit,
      stash dropped, issue closed
