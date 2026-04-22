# Task-Plan Review r1 — ISS-020 P0

## Summary

The 7-task breakdown (umbrella + p0-0..p0-6) maps cleanly onto investigation §5 P0.0–P0.6 with correct 1:1 coverage and a dependency graph that matches the investigation's ASCII chart (p0-0 ∥ p0-1; p0-4 ∥ p0-5 after p0-3). Most applied findings are preserved in the task bodies, but a few concrete guidance items from the review are implicit rather than called out in the task descriptions and risk being lost when a coder picks up the task in isolation.

## FINDING-1: p0-1 task body doesn't lock in `TypeWeights` cardinality
- Severity: minor
- Task affected: iss-020-p0-1-types
- Issue: The task says "Define Dimensions/TypeWeights/Confidence in src/compiler/types.rs" but does not name the 7 fields (factual, episodic, procedural, relational, emotional, opinion, causal). FINDING-2 of the review corrected 5→7 floats precisely because this is easy to get wrong; a coder reading only the task title could re-introduce the bug.
- Suggested fix: Add to the task body: "TypeWeights has exactly 7 f64 fields mirroring MemoryType variants: factual, episodic, procedural, relational, emotional, opinion, causal. Confidence is a 3-level enum (match existing extractor enum). Include `from_metadata_json` constructor with missing-field → None semantics (investigation §5 P0.1)."

## FINDING-2: p0-0 task body doesn't specify the serialization shape for confidence
- Severity: minor
- Task affected: iss-020-p0-0-persist
- Issue: Investigation §5 P0.0 specifies `fact.confidence` should be persisted "as the string form of `Confidence`" and `fact.valence` as `f64`, alongside the existing 10 Option fields inside `memory.metadata.dimensions`. The task summary ("Persist valence+confidence in src/memory.rs:1320-1345 write path") doesn't pin the serialization shape or the exact sub-path, which matters because p0-3's JSON parser has to agree with it.
- Suggested fix: Add to the task body: "Write `valence: f64` and `confidence: String` (string form of the Confidence enum) into `memory.metadata.dimensions` (same object as the 10 existing Option fields, NOT as siblings). Include round-trip unit tests through SQLite per §5 P0.0 exit criteria."

## FINDING-3: p0-4 task body underspecifies the token-budget guard
- Severity: important
- Task affected: iss-020-p0-4-prompt
- Issue: FINDING-5 of the review added three concrete mitigations to §4.3: (a) compact one-line format as default, (b) `prompt_detail_level: {minimal|standard|full}` config with `standard` as default, (c) when projected input tokens > budget (default 120k), drop lowest-importance memories first and record truncation in topic metadata. The task mentions "compact one-line prompt + PromptDetailLevel config + token-budget guard" but doesn't pin the 120k default, the drop-by-importance rule, or the truncation-metadata requirement. These are the kind of details a coder will guess differently without the pointer.
- Suggested fix: Expand task body to reference investigation §4.3 token-budget table explicitly, and call out: default budget 120k tokens; eviction policy = lowest-importance first; record truncation count in topic metadata; `standard` (compact one-line) is the default detail level; `minimal` must reproduce today's exact format byte-for-byte for back-compat.

## FINDING-4: p0-5 task body doesn't capture the "flag candidates, let synthesis LLM decide" escape hatch
- Severity: important
- Task affected: iss-020-p0-5-conflict
- Issue: FINDING-8 / §6 Q6 explicitly accepts shipping P0.5 with known false positives by having it *flag* contradiction candidates and letting the topic-page synthesis LLM make the final "contradiction vs evolution" call. FINDING-9 / Q6 also specifies the temporal-comparison fallback: parseable-dates preferred; else order by `memory.created_at`. The task text ("dimensional_conflict() using domain+participants+stance; temporal succession guard") is correct in skeleton but loses both nuances — a coder might implement a hard-reject rule and/or attempt string comparison on free-form `temporal` phrases.
- Suggested fix: Add to task body: "Output conflict *candidates* (not hard verdicts) — let topic-synthesis LLM resolve contradiction-vs-evolution. Temporal comparison: use parseable dates when both sides have them; otherwise fall back to `memory.created_at` ordering. Do NOT string-compare free-form `temporal` phrases. Use participants + location to scope context per §6 Q6."

## FINDING-5: p0-3 task body mentions the `updated_at` stopgap but not the verified constraint
- Severity: minor
- Task affected: iss-020-p0-3-convert
- Issue: FINDING-4 of the review verified that `MemoryRecord` (src/types.rs:162) has NO `updated_at` field — only `created_at` and `access_times`. This is what forces option (b) (stopgap). The task says "updated_at stopgap (from access_times)" which is right but doesn't tell the coder *why* they can't just use `m.updated_at`. Without this, a coder may try the "obvious" fix, discover the field missing, and ask for clarification.
- Suggested fix: Add one line: "Note: `MemoryRecord` has no `updated_at` field (verified at src/types.rs:162). Use `m.access_times.last().copied().unwrap_or(m.created_at)` as stopgap. Adding `updated_at` to `MemoryRecord` is explicitly deferred to a separate ticket."

## FINDING-6: p0-6 "verify" task doesn't enumerate the back-compat invariant test
- Severity: minor
- Task affected: iss-020-p0-6-verify
- Issue: Investigation §5 P0 exit criteria and §7.2 state a hard invariant: "A KC run over a DB of 100% legacy memories must produce the same output as today." The task says "back-compat invariant" but doesn't name the concrete test (compile a legacy-only corpus through enriched pipeline, byte-compare prompt output against `minimal` mode). Also §5 P0.6 specifies three concrete test classes: (1) `Dimensions::from_metadata_json` present/absent/malformed, (2) prompt generation snapshot tests, (3) conflict detection with synthetic stance-opposition pairs.
- Suggested fix: Expand task body to enumerate the three test classes from §5 P0.6 and the legacy-corpus equivalence assertion from §7.2. VERIFY_REPORT.md should include a section "Back-compat invariant: PASS/FAIL" with the specific test name.

## FINDING-7: General-domain caveat (FINDING-8 in review) is correctly scoped out
- Severity: informational (non-issue)
- Task affected: none
- Issue: The `general` domain clustering caveat lives in §4.5 which is P1 (clustering). The task breakdown is P0-only and correctly does not include it. No action needed; flagging this as explicitly-checked per the audit prompt.

## Scope leakage check
None detected. All tasks stay within P0 scope. Clustering/domain pre-filter (§4.5), confidence propagation (§P1.3), type_weights-driven synthesis style (§P1.4), and dimensional_coverage metric (§4.6/§P2.1) are all correctly absent from P0 tasks.

## Finding-preservation matrix
| Review finding | Applied in investigation | Preserved in task breakdown? |
|---|---|---|
| FINDING-1 (valence/conf not persisted) | §1.1, §3, §4.1, §4.2, §5 P0.0 | ✅ p0-0 exists and is independent |
| FINDING-2 (TypeWeights = 7 floats) | §3 table | ⚠️ p0-1 body doesn't name the 7 fields (see FINDING-1 above) |
| FINDING-3 (last_extraction_emotions misleading) | §4.2 rewritten | ✅ N/A — p0-3 just parses persisted metadata |
| FINDING-4 (updated_at stopgap) | §4.2 sub-note | ⚠️ p0-3 mentions it but lacks the src/types.rs:162 verification pointer (see FINDING-5 above) |
| FINDING-5 (token budget + compact + PromptDetailLevel) | §4.3 rewritten | ⚠️ p0-4 names all three but underspecifies defaults (see FINDING-3 above) |
| FINDING-6 (general domain caveat) | §4.5 caveat | ✅ Correctly out of P0 scope (clustering is P1) |
| FINDING-7 (P0 dependency graph) | §5 ASCII graph | ✅ Edge graph matches exactly |
| FINDING-8 (temporal is free-form string) | §6 Q6 rewritten | ⚠️ p0-5 loses the "flag candidate, let LLM decide" nuance (see FINDING-4 above) |
| FINDING-9 (stale coordination items) | §7.7 | ✅ N/A — coordination, not implementation |

## Dependency-graph verification
Investigation §5 graph:
```
P0.0                           (independent)
P0.1 → P0.2 → P0.3 → { P0.4, P0.5 }
P0.6                           (alongside)
```
Task edges:
- p0-0: independent ✅
- p0-1: independent ✅
- p0-2 deps p0-1 ✅
- p0-3 deps p0-2, p0-0 ✅ (p0-0 dep is stronger than investigation — investigation says P0.4 picks up confidence only after P0.0, implying p0-3 could technically land before p0-0 and carry `valence=None`/`confidence=None`. Making p0-3 depend on p0-0 is acceptable tightening and avoids a temporarily-inert field; not a bug.)
- p0-4 deps p0-3 ✅
- p0-5 deps p0-3 ✅ (parallel with p0-4 ✅)
- p0-6 deps p0-4, p0-5 ✅

Graph matches. p0-0 ∥ p0-1 ✅. p0-4 ∥ p0-5 after p0-3 ✅.

## Verdict

- Proceed with implementation? **yes-with-caveats** — apply the body-text expansions from FINDING-1..FINDING-6 before a coder picks up the tasks. None of the findings invalidate the breakdown; they're all "the task title is right but a coder in isolation will miss the nuance applied in review r1."
- Recommended execution order:
  1. **p0-0** and **p0-1** in parallel (both independent; p0-0 unblocks real data for p0-3's parser tests)
  2. **p0-2** (after p0-1)
  3. **p0-3** (after p0-0 + p0-2)
  4. **p0-4** and **p0-5** in parallel (after p0-3)
  5. **p0-6** (after p0-4 + p0-5) — though per §5, test scaffolding can grow alongside each earlier task
