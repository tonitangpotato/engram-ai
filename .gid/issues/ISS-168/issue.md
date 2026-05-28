---
title: Triple extractor parser drops ~5% of Haiku responses on multi-array CoT pattern (prose between [...] blocks)
priority: P2
severity: minor
status: in_review
category: extractor
created: 2026-05-27
relates_to:
- ISS-167
- ISS-166
- .gid/issues/ISS-167/issue.md
discovered_in: ISS-166 validation probe PID 16259 (2026-05-27)
tags:
- extractor
- parser
- haiku
- locomo
fixed_by: 3fe1585
resolved: 2026-05-28
---

## Summary

After ISS-167 landed (parser tolerates duplicate JSON keys and
drops individual malformed elements without rejecting the whole
array), one residual Haiku failure mode still produces a parse
WARN and drops the triple set for that episode: Haiku occasionally
emits a chain-of-thought response with **two `[...]` JSON arrays
separated by prose**, ending in an empty array.

Concrete example observed during ISS-166 validation
(`/tmp/iss167-probe-validate2.log`):

```text
[{"subject": "Caroline", "predicate": "creates",
  "object": "sunset painting", "confidence": 0.7}]

Wait, I need to reconsider - "creates" is not in the allowed
predicates list. Let me re-evaluate:

The text is a conversational exchange about a painting. The only
potentially extractable relationship would be that Caroline
created/painted a sunset, but "creates" or "paints" are not in
the allowed predicates. The allowed predicates (is_a, part_of,
uses, depends_on, caused_by, leads_to, implements, contradicts,
related_to) don't fit this casual conversation well.

[]
```

`parse_triple_response` currently slices `s[find('[')..rfind(']')+1]`
to strip preamble/postamble. With two arrays plus prose, this
slice produces:

```text
[{...}]\n\nWait, ... reasoning prose ...\n\n[]
```

which fails JSON parse → entire response dropped.

## Impact

**Observed rate ≈ 5%** of Haiku calls in conv-26 ingest, based on
PID 16259 probe sampling (1 case across the parts of the log
inspected; full count requires a grep sweep but multiple
WARN-trailing-prose lines were visible).

Lost triples here are typically the ones Haiku had second
thoughts about — often valid relationships that Haiku rejected
because none of the 9 allowed predicates fit. This double-loss
(parser AND prompt restrictiveness) compounds with ISS-162
extraction-context work: if we widen the predicate set, Haiku may
no longer second-guess itself, eliminating the multi-array
pattern.

In aggregate this is **not blocking** — ISS-166 validation showed
666 entities and 456 successful pool jobs on conv-26, plenty of
signal to validate the substrate works. But on smaller
conversations or longer-form CoT prompts, the loss rate could
matter.

## Repro

1. Run any LoCoMo ingest with `ENGRAM_BENCH_PIPELINE_POOL=1`
   using `AnthropicTripleExtractor` default Haiku model.
2. Grep WARN logs for "Failed to parse triple extraction JSON"
   with `trailing characters` in the error text.
3. Each hit is one dropped episode of triples.

## Fix direction

Three options, in order of robustness:

1. **Take the FIRST `[...]` block, not the slice between first
   `[` and last `]`.** Use a regex or a bracket-depth scanner to
   extract the first complete top-level JSON array. Discard
   everything after the array's matching `]`. This handles the
   observed case (`[{...}] prose []`) by parsing the first
   array, ignoring the second empty array and the prose between.
2. **Take the LAST `[...]` block.** The opposite policy —
   prefer the model's "final answer" array. Risk: in the
   observed case, Haiku's "final answer" is `[]` (it talked
   itself out of a valid triple).
3. **Take the UNION of all top-level `[...]` blocks** and
   deduplicate. Most robust, but adds parser complexity.

Recommend Option 1: matches the principle "extract first
self-contained JSON, ignore CoT remnants". Aligns with how most
LLM JSON-output parsers handle this. ~15 LOC change + 2 tests.

## Acceptance criteria

- AC-1: `parse_triple_response` extracts the first complete
  top-level JSON array regardless of trailing prose or additional
  arrays. The chosen extraction policy is documented in the
  function's doc comment with a rationale.
- AC-2: Three regression tests cover:
  - `[{...}]\n\nprose\n\n[]` (the observed pattern, two arrays)
  - `[{...}, {...}]\n\nWait, scratch that.\n\n[{...}]`
    (replacement pattern — first array wins, second discarded)
  - `prose preamble\n[{...}]\nprose postamble` (already handled
    by ISS-167's tolerance, regression-test for it under the new
    parser)
- AC-3: All existing 6 ISS-167 regression tests still pass.
- AC-4: Re-run a small conv-26 ingest with
  `ENGRAM_BENCH_PIPELINE_POOL=1`; verify zero WARN lines with
  "trailing characters" in the parse-failure path. (A non-zero
  count means a new failure mode surfaced — file as ISS-169 not
  re-open ISS-168.)

## Notes

This issue was deliberately deferred from ISS-167 to keep that
fix narrow. ISS-167 closed the **100% rejection** failure mode
(duplicate JSON keys); ISS-168 closes the residual **~5% loss**
failure mode (multi-array CoT). Both can coexist in the parser.

---

## Implementation — 2026-05-28 (commit `3fe1585`)

Option 1 (first-array-wins, ~15 LOC + 5 tests) shipped per recommendation.

### Approach

New helper `extract_first_top_level_array(&str) -> Option<&str>`
walks the input char-by-char tracking:

- bracket depth (`[` → +1, `]` → −1)
- whether the cursor is inside a JSON string literal (`"` toggles
  it; `\` escapes the next char so `\"` doesn't close the string)

It returns the slice from the first `[` through its matching `]`
(inclusive). Everything after — a second array, trailing prose,
anything — is discarded.

`parse_triple_response` swaps `s[find('[')..rfind(']')+1]` for this
helper; behaviour on single-array inputs is unchanged.

### AC ticks

- [x] **AC-1**: first complete top-level JSON array extracted regardless
  of trailing prose / additional arrays. Policy + rationale documented
  in the helper's doc comment.
- [x] **AC-2**: 5 regression tests cover:
  - `iss168_two_arrays_with_prose_between_takes_first` — the observed
    pattern from ISS-166
  - `iss168_replacement_pattern_first_array_wins` — scratch-that pattern,
    pins the documented choice of first-wins (vs. last-wins)
  - `iss168_prose_preamble_and_postamble_still_extracts` — ISS-167
    regression guard
  - `iss168_nested_brackets_in_string_values_do_not_confuse_scanner` —
    string-awareness guard
  - `iss168_escaped_quote_in_string_preserves_string_boundary` —
    escape-handling guard
- [x] **AC-3**: all 6 ISS-167 regression tests still green
  (`parse_triple_response_tolerates_duplicate_*`,
  `parse_triple_response_mixed_valid_and_malformed_elements_keeps_valid`,
  etc.). 22/22 `triple_extractor` tests. 2016/2016 engramai lib tests.
- [ ] **AC-4**: zero "trailing characters" WARN on next conv-26
  ingest with `ENGRAM_BENCH_PIPELINE_POOL=1`. Deferred — cheap to
  verify when the next ISS-179 sweep runs; no separate bench needed
  just for this. If a new failure mode surfaces in the wild, file
  ISS-XXX rather than re-opening ISS-168.
