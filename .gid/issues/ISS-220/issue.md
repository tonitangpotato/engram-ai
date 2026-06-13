---
id: ISS-220
title: Stored memory content is not byte-matchable via SQL LIKE — defeats operational lookup, audit, and dedup
kind: issue
status: todo
priority: P2
labels: [engram, storage, content-encoding, observability, data-integrity]
created: 2026-06-10
---

# ISS-220: Recalled content cannot be found by `SELECT ... WHERE content LIKE '%phrase%'`

## Summary

Text that `engram_recall` clearly returns to the agent **cannot be located in the
`memories.content` column by substring SQL**. Multiple distinctive phrases from a
recalled memory all returned **zero rows** from `LIKE` queries, even though the recall
engine rendered that exact text moments earlier.

This breaks every operational path that depends on finding a memory by its visible
text: targeted deletion (ISS-219), manual audit, dedup verification, and `UPDATE`
maintenance (e.g. pinning a specific memory).

## Reproduction (2026-06-10, default namespace, trader bot)

1. `engram_recall("...")` returned an item whose rendered content contained the
   verbatim strings: `全损反模式`, `你账本里最痛`, `甩你脸上`, `错配中的错配`,
   `要不要买一个短一点的call`, and a stored correction containing `权威更正`.
2. Each of these was searched directly against the DB:
   ```sql
   SELECT id FROM memories WHERE content LIKE '%全损反模式%';      -- 0 rows
   SELECT id FROM memories WHERE content LIKE '%账本里最痛%';      -- 0 rows
   SELECT id FROM memories WHERE content LIKE '%错配中的错配%';    -- 0 rows
   SELECT id FROM memories WHERE content LIKE '%权威更正%';        -- 0 rows  (just stored!)
   ```
3. Even a memory **stored seconds earlier** in the same session
   (`content` beginning with `【权威更正 ...`) could not be found by `LIKE '%权威更正%'`.
4. By contrast, *some* memories' content IS matchable (e.g. `LIKE '%COIN%' AND
   '%ARM%' AND '%TSM%'` returned two unrelated rows), so the failure is **selective**,
   not "LIKE never works".

## Hypotheses (need confirming by whoever fixes)

The selective nature points to one of:

- **(A) Rendered ≠ stored.** `recall` may synthesize / re-compose / summarize the
  text it shows the agent from `summary`, `tokens`, embeddings, or multiple fragment
  rows — so the rendered string never existed verbatim in any single `content` cell.
  If true, this is the deeper issue: *recall can show text that corresponds to no
  single deletable row* (directly worsens ISS-219).
- **(B) Content transform at write.** `content` may be tokenized / normalized /
  unicode-reshaped on store, so the persisted bytes differ from the input bytes
  (NFC/NFD normalization, full/half-width, smart-quote rewriting, token packing into
  the `tokens` column).
- **(C) Wrong column.** The human-visible text may live in `summary` or `metadata`
  or `tokens`, not `content`, for these rows.

A 10-minute probe (store a known unicode+ascii string, then `SELECT content, summary,
tokens, hex(substr(content,1,40))` for that id) will disambiguate A/B/C.

## Impact

- **Blocks ISS-219** (can't reverse-look-up an id to delete).
- **Blocks manual pinning / maintenance** (`UPDATE ... WHERE content LIKE` is a no-op).
- **Blocks audit & dedup** — you can't ask "do we already store this fact?" via SQL.
- **Erodes trust in storage** — if recalled text maps to no row, the substrate is not
  inspectable, which is dangerous for a memory system that agents act on.

## Proposed direction

1. **Diagnose A vs B vs C first** (the probe above). The fix differs hard per cause.
2. If (B): store content **byte-faithfully**, or expose a normalized search column +
   document that callers must search it. Provide an `engram find --text` command that
   searches the same normalized form recall uses.
3. If (A): this is the more serious finding — recall must be able to point every
   rendered item back to its source row(s). Tie the fix to ISS-219 (expose ids) so a
   rendered item always carries the id(s) it was composed from.
4. If (C): document the real text column and make `engram find` search it.

## Acceptance criteria

- [ ] AC-1: Root cause classified (A/B/C) with the disambiguation probe output attached.
- [ ] AC-2: A supported way exists to locate a memory by its recalled text (CLI/tool),
      working on the exact strings recall renders.
- [ ] AC-3: Round-trip test: store string S (mixed CJK + ASCII + emoji) → locate it by
      a substring of S → get back the same id recall would surface.
- [ ] AC-4: If cause is (A), recall output is amended so every rendered item maps to a
      concrete source id (shared with ISS-219).

## Relationship

- **ISS-219**: the id-exposure gap. ISS-219 + ISS-220 together make poisoned-memory
  deletion impossible; fixing one without the other only half-restores the loop.
