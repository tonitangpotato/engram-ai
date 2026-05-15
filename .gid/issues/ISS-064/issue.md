---
id: ISS-064
title: Namespace mismatch in graph_query is silently swallowed (returns empty instead of warning)
status: done
severity: medium
priority: P2
labels:
- retrieval
- observability
- usability
relates_to:
- ISS-056
- ISS-063
discovered: 2026-04-28
fixed_by: be42a57
---

# Namespace mismatch silently returns empty

## TL;DR

`Memory::graph_query(GraphQuery::new(q).with_namespace("foo"))` returns
an empty `RetrievalResponse` with `outcome=ok candidates=0` (or
`empty_result_set` post-ISS-063) when `"foo"` doesn't match any
namespace in the substrate. There is **no warning, no error, no log
line** distinguishing "namespace does not exist" from "namespace exists
but corpus is genuinely empty".

This silently caused **RUN-0001 (LoCoMo conv-26 smoke) to report
0/25 hits @ 5** — every query targeted `--ns conv26` but the substrate
ingest used `--ns locomo-conv26-iss058`. Once the namespace was
corrected (RUN-0002) the same fix produced 14/25 hits @5. The bug
has been latent since ISS-056 (namespace propagation through
GraphQuery) landed.

## Why it matters

Namespace is a string. Typos and naming mismatches between ingest
and retrieval scripts are the single most common operational error.
Silent zero-result responses look indistinguishable from "the model
doesn't know that fact" and burn hours of debugging on the wrong
hypothesis (we filed ISS-060, ISS-061, and started on ISS-063 design
notes before discovering the real cause was a `--ns` typo).

## Repro

```bash
# Ingest into namespace "foo"
engramai ingest --ns foo memories.txt

# Query namespace "fo" (typo)
cargo run --example locomo_conv26_retrieval -- --ns fo ...
# Result: 0 hits, outcome=ok candidates=0, no warning anywhere
```

## Acceptance

Pick one (or both):

1. **Fail-fast in the orchestrator**: when `GraphQuery::namespace = Some(ns)` and
   the namespace has 0 memories AND 0 graph_entities, return a new
   `RetrievalOutcome::NamespaceNotFound { namespace }` variant with
   the namespace string. Distinguishable from `EmptyResultSet`.

2. **Operational warning**: log a `WARN` line at the dispatch layer
   when the requested namespace has 0 memories. Emit it once per
   `Memory::graph_query` invocation; don't spam.

Test on an empty graph + non-existent namespace: the response /
log makes the mismatch immediately discoverable.

## Out of scope

- Namespace governance, allowlists, validation at ingest time
  (separate concern — could be ISS-065 if needed).
- Auto-suggesting "did you mean X?" via fuzzy matching on existing
  namespaces. Nice-to-have; not required to close.

## Related

- ISS-056 (namespace propagation through GraphQuery — landed but
  didn't add validation)
- ISS-063 (downgrade-to-fallback contract — exposed this when
  RUN-0002 corrected the namespace and recall jumped 0% → 56%)
- The smoke run report at
  `.gid/issues/_smoke-locomo-2026-04-28/RUN-0002.md` documents the
  full discovery trail.

## Closure (2026-05-15)

**Status: done.** Code shipped commit `be42a57` (2026-04-29) —
adds tracing::warn! + a `NamespaceNotFound`-style `reason` field
(`reason: "namespace_not_found"`) when an explicit namespace
exists but has no memories AND no graph entities. AC option 1
satisfied (fail-fast with distinguishable reason).

Verified 2026-05-15: code present at
`crates/engramai/src/retrieval/api.rs:414-442`. Regression test
binary `crates/engramai/tests/iss064_namespace_mismatch_warn_test.rs`
ships 3 tests — all pass:

- `iss064_query_nonexistent_namespace_returns_empty_and_warns`
- `iss064_explicit_default_on_empty_store_warns`
- `iss064_implicit_default_namespace_does_not_warn` (negative
  test: implicit-default path stays quiet)

`fixed_by: be42a57`. Status flipped open → done.
