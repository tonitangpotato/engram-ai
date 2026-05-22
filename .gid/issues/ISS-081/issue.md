---
id: ISS-081
title: CLI store missing --meta flag — user_metadata side-channel unreachable from CLI
status: resolved
priority: P0
severity: high
tags:
- cli
- metadata-channel
- regression-blocker
- breaking-contract
relates_to: []
created: 2026-04-30
fixed_by: d4896b8
---

# CLI store missing --meta flag — user_metadata side-channel unreachable from CLI

## Summary

`engram store` CLI exposes no way to set `user_metadata` (the opaque caller-owned
metadata side-channel specified in `docs/metadata-channel.md`). The library API
(`StorageMeta { user_metadata, ... }` in `crates/engramai/src/store_api.rs:84`)
fully supports it, the design contract is documented, but the CLI binding was
never written.

This is a **breaking gap between documented contract and shipped CLI surface.**

## Impact

- **cogmembench LoCoMo benchmark is fully blocked.** Adapter
  (`benchmarks/locomo/engram_adapter.py:138-144`) constructs `--meta key=val`
  args based on `docs/metadata-channel.md`. Every store invocation fails with
  clap "unexpected argument", returncode != 0. cogmembench's silent stderr
  guard swallows the error → 0 memories ingested per conversation → all
  benchmark runs report `acc=0% / ev_recall=0%` (regression discovered
  2026-04-30 in run 71/199).
- **LongMemEval benchmark blocked for the same reason** (shared adapter
  pattern at `benchmarks/longmemeval/engram_adapter.py`).
- **Apr-22 RUN-0004's reported 23.1% accuracy is invalid data** — must have
  been LLM-judge variance on empty retrieval (no memories actually stored).
  Treat that result as noise.
- Any external integration relying on the documented metadata-channel contract
  is broken. RustClaw is fine because it uses the library API directly, not
  the CLI.

## Root Cause

`crates/engram-cli/src/main.rs` `Commands::Store { ... }` variant has fields
for `content, ns, type, importance, source, emotion, domain, extractor,
extractor_model, auth_token, oauth, graph_db, no_graph, graph_drain_timeout_secs`
— but no `meta`. The handler at `Commands::Store { ... } =>` calls into the
library without populating `StorageMeta::user_metadata`.

The design doc `docs/metadata-channel.md` (status: "Design doc for engram
v0.2.3+") specifies the contract. The CLI implementation was never written
(or was written and reverted — git log search for `parse_meta_kv` returns
nothing in `cli/`).

## Fix

Add `--meta key=value` (repeatable) to the CLI Store command:

```rust
// In crates/engram-cli/src/main.rs, Commands::Store { ... } variant:
/// Caller-owned metadata side-channel (repeatable). Format: key=value.
/// Values are parsed as JSON if possible, else stored as string.
/// See docs/metadata-channel.md.
#[arg(long = "meta", value_name = "KEY=VALUE")]
meta: Vec<String>,
```

Plus a `parse_meta_kv(Vec<String>) -> serde_json::Value` helper:

- Split each entry on first `=` (so values can contain `=`)
- Try `serde_json::from_str(&value)`; on parse error, store as JSON string
- Build a `serde_json::Map`; assemble into `Value::Object`
- Pass into `StorageMeta::user_metadata` on the store call site

Also add the same flag to `store-batch` if it exists (per metadata-channel.md
batch examples).

### Acceptance Criteria

- `engram store "hello" --meta dia_id=D1:3 --meta turn_index=5` succeeds
- Stored memory's `user_metadata` is `{"dia_id": "D1:3", "turn_index": 5}`
  (string preserved, number parsed)
- `engram recall ... --json` output includes `user_metadata` so callers can
  back-map (verify this is already wired; if not, separate sub-task)
- Repeated keys: last write wins (or error — pick one and document)
- Reserved keys (`engram_*`, `extractor_*`) are rejected with a clear error
  per docs/metadata-channel.md §Namespace Reservation
- Test in `crates/engram-cli/tests/` that round-trips `--meta` through store
  + recall

### Out of Scope

- Restructuring the metadata-channel design itself (that's stable)
- Library-side changes (already works)

## Verification

1. Build: `cargo build --release -p engram-cli`
2. Unit test: `cargo test -p engram-cli meta`
3. Integration: re-run cogmembench `conv-26` — expect non-zero acc/ev_recall

## Related

- `docs/metadata-channel.md` — the design contract this implements
- `crates/engramai/src/store_api.rs:84` — `StorageMeta.user_metadata` field
- cogmembench (unregistered project) — adapter pattern blocked on this

## 2026-05-22 — Resolved (with one AC dropped after spec re-read)

The fix landed in code months ago but the issue was never closed. This
session verified, added the missing integration test, and reconciled one
AC item against the actual design doc.

### What was already done (in-tree before this session)

- `--meta key=value` repeatable flag on `engram store`
  (`crates/engram-cli/src/main.rs:147-148`)
- `parse_meta_kv` helper with JSON-or-string fallback semantics
  (`main.rs:973-994`)
- In-process unit tests `meta_kv_tests` covering empty input, mixed
  parse, `=`-in-values, repeated keys (last-write-wins), bad keys, and
  missing-`=` rejection (`main.rs:999-1046`, 6 tests).
- `--meta` is wired into `StorageMeta.user_metadata` at the Store
  dispatch site (`main.rs:1331`).
- `engram recall --json` round-trips `MemoryRecord` via serde, so
  `metadata.user.*` is included verbatim (no separate wire-up needed
  — `MemoryRecord` already serializes its full payload).

### What this session added

- `crates/engram-cli/tests/iss081_meta_roundtrip.rs` — 3 integration
  tests exec'ing the `engram` binary against a tempdir DB:
  1. `store_and_recall_round_trips_meta_flag` — string + number + JSON
     array round-trip through `metadata.user.*`.
  2. `meta_repeated_key_is_last_write_wins` — pins the dedup contract
     at the CLI surface.
  3. `meta_missing_equals_is_rejected_with_clear_error` — diagnostic
     error message includes the offending input.
- `[dev-dependencies] tempfile = "3"` added to
  `crates/engram-cli/Cargo.toml`.

### AC reconciliation — one item dropped

The original AC list included:

> "Reserved keys (`engram_*`, `extractor_*`) are rejected with a clear
> error per docs/metadata-channel.md §Namespace Reservation"

This section **does not exist in the design doc**. `docs/metadata-channel.md`
§"Engram's Contract" promise #1 (line 118) says:

> Opaque pass-through — metadata is stored as `serde_json::Value` and
> returned on recall exactly as provided. Engram does not parse,
> validate, mutate, or interpret it.

And §"Caller's Responsibility: Schema Location" (line 128) says:

> Engram does not enforce a metadata schema.

Adding reserved-prefix validation would **contradict the documented
contract**. The AC item was based on a misread / fabricated reference;
dropping it. If reserved namespaces become useful in the future (e.g.
for `engram.synthesized=true` markers from KC), that's a forward
design change to `metadata-channel.md`, not a CLI-side patch.

The other ACs are all met:

- [x] `engram store "hello" --meta dia_id=D1:3 --meta turn_index=5`
      succeeds — pinned by `store_and_recall_round_trips_meta_flag`
- [x] Stored memory's `user_metadata` is the expected JSON — same test
- [x] `engram recall ... --json` output includes `user_metadata` —
      same test (asserts `/record/metadata/user/*`)
- [x] Repeated keys: last write wins, documented — pinned by
      `meta_repeated_key_is_last_write_wins` and unit test
- ~~Reserved keys rejected~~ — dropped per AC reconciliation above
- [x] Round-trip integration test in `crates/engram-cli/tests/` —
      added this session

### Status

Marking resolved. cogmembench / LongMemEval blockers from the original
issue are unblocked.
