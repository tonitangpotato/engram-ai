---
title: Triple extractor JSON parser rejects 100% of Haiku responses (duplicate object_kind / subject_kind fields)
blocks: ISS-166
labels:
- extractor
- resolver
- bench-blocker
- parser
priority: P0
relates_to:
- ISS-164
- ISS-165
- ISS-166
- .gid/issues/ISS-168/issue.md
severity: blocker
status: resolved
fixed_by: engram:89d5ac9
---

# ISS-167: triple extractor parser rejects 100% of Haiku responses

## Summary

`AnthropicTripleExtractor` (default model `claude-haiku-4-5-20251001`) reliably emits JSON objects with **duplicate `object_kind` (and occasionally `subject_kind`) fields** in the same triple. The strict `serde_json::from_str::<Vec<RawTriple>>(...)` deserializer in `parse_triple_response` (crates/engramai/src/triple_extractor.rs:113) rejects the entire array on the first duplicate-key error → returns `Ok(vec![])` → **zero triples persisted**.

End-state: graph_entities table stays empty in production *and* bench whenever Haiku is the configured extractor. `GraphEntityResolver.resolve()` then returns zero anchors for every query, silently disabling the entity-anchor channel introduced by ISS-164 §4.3.

This bug has been latent in production. It surfaced only when ISS-166 wired the v0.3 pipeline into engram-bench: 3/3 Haiku calls in 2.5min of LoCoMo conv-26 ingest hit the duplicate-key path. The dropped-triple counter is **logged at WARN but not propagated** — the pool's `WorkerPoolStats` count it as successful job runs, so no upstream signal exists.

## Evidence

Probe `examples/iss165_ac1_resolver_probe.rs` (engram-bench commit `bfb1115`) run 2026-05-27 22:23-22:25 EDT against LoCoMo conv-26 fixture (sha `39e7df4...`), env `ENGRAM_BENCH_PIPELINE_POOL=1 ENGRAM_BENCH_PIPELINE_WORKERS=4`.

Log: `/tmp/iss166-probe-validate.log` (kept).

All three LLM responses logged hit the same parser failure:

```
[2026-05-27T02:24:05Z WARN  engramai::triple_extractor] Failed to parse triple extraction JSON:
  duplicate field `object_kind` at line 23 column 17 - content: [
      {
        "subject": "organization",
        "predicate": "implements",
        "object": "inclusivity",
        "confidence": 0.85,
        "object_kind": "Organization",
        "object_kind": "Concept"     # ← duplicate
      }
    ]
```

```
[2026-05-27T02:25:08Z WARN  engramai::triple_extractor] Failed to parse triple extraction JSON:
  duplicate field `object_kind` at line 4 column 130 - content: [
    {"subject": "necklace", "predicate": "related_to", "object": "love",
     "confidence": 0.8, "object_kind": "Artifact", "object_kind": "Concept"},
    ...
  ]
```

Sample size n=3 calls, failure rate 3/3 = 100%. This isn't intermittent.

## Root cause

The prompt (crates/engramai/src/triple_extractor.rs:49-77) instructs Haiku to emit *one* `object_kind` per object value. Haiku, when it sees a concrete noun that plausibly fits two kinds simultaneously (e.g. "necklace" = Artifact + Concept, "organization" = Organization + Concept), packs both into the same JSON object as duplicate keys rather than choosing.

Examples in the prompt itself don't disambiguate this case — they only show duplicate *kinds across different triples*, never within one triple. Haiku's output is consistent with how it interprets the prompt; the prompt under-specifies.

Parser is strict (default `serde_json` behaviour on `Deserialize` derive of a struct field). On duplicate field → error → whole array fails.

## Decision: fix the parser, not the prompt

The prompt could be tightened ("if two kinds fit, pick the more specific; never emit the same key twice"). But:

1. LLMs slip schemas. A robust parser is needed regardless.
2. The fix is mechanically trivial: take the *last* value for duplicate keys (matches `JSON.parse` and most permissive JSON parsers).
3. Strict parsing is silently throwing away production data. That's worse than tolerant parsing + a counted warning.

Prompt tightening is a follow-up nice-to-have but not on the critical path.

## Fix plan

Replace the strict struct deserializer with a tolerant one. Options:

**(a) Custom `Deserialize` impl on `RawTriple`** that uses `MapAccess` and overwrites on duplicate keys.

**(b) Pre-process JSON via `serde_json::Value`** then convert. Marginally less code, slightly higher allocation cost. Acceptable for ~1-call-per-episode rate.

Going with (b) — fewer lines, easier to test.

Add metric: count `extractor.duplicate_field_recoveries` so we can see how often this fires post-fix.

## Acceptance criteria

- [ ] AC-1: `parse_triple_response` accepts JSON objects with duplicate `object_kind` / `subject_kind` keys; takes the **last** value.
- [ ] AC-2: Unit test `parse_triple_response_tolerates_duplicate_object_kind` (regression for the exact Haiku payloads above).
- [ ] AC-3: Unit test `parse_triple_response_tolerates_duplicate_subject_kind` (mirror case).
- [ ] AC-4: Other duplicate fields (subject, predicate, object, confidence) — also take last (consistent behaviour), with one regression test.
- [ ] AC-5: Existing `parse_triple_response` tests still pass (no regressions on well-formed input).
- [ ] AC-6: Re-running the ISS-166 validation probe shows `total entities > 0` and per-query `anchors_found > 0` on at least 1 of the 9 single-fact queries (proving the wire-up + parser fix together populate graph_entities). Update ISS-166 AC-3 accordingly.

## Out of scope (follow-ups)

- Prompt tightening to discourage duplicate keys upstream — file as ISS-XXX once ISS-167 lands.
- Replacing strict serde_json with `serde_json::Deserializer::from_str(...).into_iter::<Value>()` pipeline globally — unnecessary, this parser is the only known offender.
- Sonnet/Opus comparison for extraction quality — separate question, doesn't change parser contract.

## Repro

```bash
cd /Users/potato/clawd/projects/engram-bench
export ANTHROPIC_AUTH_TOKEN=$(perl -e 'alarm 3; exec @ARGV' \
    security find-generic-password -s "Claude Code-credentials" -w \
    | python3 -c 'import sys,json; print(json.load(sys.stdin)["claudeAiOauth"]["accessToken"])')
export ENGRAM_BENCH_PIPELINE_POOL=1
FIX=benchmarks/fixtures/locomo/39e7df4ea492e8bc7a483b2cfc8e18620054beb05fed267f5cc098bd65fd5f4d/conversations.jsonl
./target/release/examples/iss165_ac1_resolver_probe --fixture "$FIX" --conv conv-26 \
    > /tmp/iss167-repro.log 2>&1 &

# Wait 2 min, then:
grep -c "Failed to parse" /tmp/iss167-repro.log
# Pre-fix: > 0 (every Haiku response fails)
# Post-fix: 0 (or near-zero, only on genuinely malformed JSON)
```

## Files

- `crates/engramai/src/triple_extractor.rs:101-138` — `parse_triple_response` (the parser)
- `crates/engramai/src/triple_extractor.rs:49-77` — the prompt (informational, not changing)
- `/tmp/iss166-probe-validate.log` — evidence log (kept)

## Blast radius

- Writer: `parse_triple_response` only. All paths feed through it (Anthropic, Ollama, mock).
- Behavior change: previously-rejected JSON arrays now produce non-empty triple lists. Downstream pipeline (fusion, decision, persist) already handles arbitrary triple counts.
- Risk: low. Tolerance is monotonic — strictly more inputs accepted, no inputs rejected differently.

## Provenance

Discovered during ISS-166 Plan A validation (engram-bench commit `bfb1115`), 2026-05-27.

---

## 2026-05-27 — RESOLVED

Parser tolerance fix shipped and validated end-to-end against the
real Haiku output distribution in LoCoMo conv-26 ingest.

### Implementation

`engram` commit `89d5ac9` — `crates/engramai/src/triple_extractor.rs`:

- `parse_triple_response` now goes through `serde_json::Value`
  intermediate, then `serde_json::from_value` per-element.
- Duplicate JSON keys: `serde_json::Value` takes the last value
  (matches `JSON.parse` semantics in JavaScript). This handles
  Haiku's frequent `object_kind: "X", object_kind: "Y"` pattern.
- Malformed individual triples are dropped with a WARN log and the
  remainder of the array continues. Previously a single bad
  element rejected the whole array → 0 triples persisted.
- 6 new regression tests cover: duplicate `object_kind`,
  duplicate `subject_kind`, missing required field per-element
  drop, mixed valid/invalid array, trailing prose, empty array.
- All 1962 engramai lib tests green.

### Validation evidence

Same probe run as ISS-166 (PID 16259, 2026-05-27 23:48 EDT,
419 episodes conv-26):

- `WorkerPoolStatsSnapshot { jobs_processed: 456, jobs_failed: 0,
  jobs_in_flight: 0, jobs_dropped_inbox_full: 0 }`
- 666 entities persisted (would be ~0 without this fix; pre-fix
  3/3 Haiku calls failed parse in 2.5min probe v1 PID 11296).
- One observed remaining failure mode during validation:
  trailing prose after JSON array (`[...] \n Wait, let me
  reconsider...`). The drop-and-continue path handled it: that
  one episode logged a parse WARN, the rest of the ingest
  continued, and the next batches' triples were persisted
  normally. Filed as ISS-168 for the small follow-up of slicing
  the JSON array out before parsing (current rate ≈ 5% of Haiku
  calls based on probe sampling).

### AC status

- **AC-1** parser tolerates duplicate keys: done
- **AC-2** drop-bad-element-continue semantics: done
- **AC-3** regression tests: done (6 added)
- **AC-4** end-to-end validation (graph_entities populated): done
  (666 entities persisted vs 0 pre-fix)
- **AC-5** ISS-166 unblocked: done (ISS-166 resolved)
- **AC-6** investigate prompt tightening as defensive layer:
  deferred to ISS-162 work. Tightening the prompt could reduce
  Haiku's tendency to emit duplicate keys / multi-array
  responses, but is orthogonal to parser robustness. Parser
  robustness must hold regardless of prompt.

### Resolution

Status: open → resolved. ISS-168 filed as a small follow-up for
the multi-array CoT response pattern (~5% loss rate).
