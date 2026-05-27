# ISS-165 AC-1 probe result — RustClaw main heartbeat 20:38 EDT 2026-05-26

## Headline

**9/9 NO_ANCHORS** on the stubborn single-fact queries via `GraphEntityResolver::resolve(question)`.

Probe binary: `engram-bench/examples/iss165_ac1_resolver_probe.rs` (built 20:08, last run 20:11→20:26 EDT)
Log: `/tmp/iss165-ac1-probe-v2.log`

Per-query verdicts (all identical):
- q3, q7, q11, q37, q40, q43, q71, q75, q76 → **NO_ANCHORS** (`anchors.len() == 0`)

H1 routing per probe's own decision rule:
> H1 CONFIRMED (NO_ANCHORS + ANCHOR_FOUND_NO_GOLD = 9/9 ≥ 6) → Next lever: ISS-162 (extr...)

## Caveat — possible config drift between probe and bench

ISS-164 Phase 1 A/B bench (STAMP 20260526T213218Z) on the SAME 9 questions retrieved memories that mentioned Caroline (e.g. q3 predicted: "Caroline was 'off to go do some research'"). The bench's entity channel surfaced anchors for these queries during retrieval — that's why predicted answers mention Caroline at all.

But the probe's freshly-built in-memory ingest produces `resolve()` → empty Vec for the same question texts.

**Two non-exclusive explanations:**

1. **Resolver match-rule selectivity**: the bench may surface anchors via a different code path (e.g. by-content match during candidate scoring, not by question-text resolve). The probe specifically calls `resolver.resolve(question_text)` — which may apply stricter thresholds (min_confidence, surface-form match, etc.) than the channel's actual ingestion path.
2. **Ingest path difference**: probe uses `fresh_in_memory_db` (per iss144 template) — entity index may be populated by a different code path than the persistent benchmark store, and may not have indexed "Caroline" / "Melanie" as resolvable anchors.

**Either way, the probe's H1 verdict needs verification before routing to ISS-162.** If (1), then resolver is over-strict and ISS-162 is mis-routed (real bug is in resolver thresholds). If (2), then probe is invalid and AC-1 needs a re-run via the bench harness, not a freestanding example.

## Recommended next step (for the agent picking this up)

- Check `GraphEntityResolver::resolve()` for a min_confidence threshold or surface-form constraint.
- Re-run AC-1 by adding instrumentation to the **bench's** `AssociativePlan::execute()` Step 2b path to dump `(question, resolved_anchors)` for the 9 stubborn questions. The bench retrieval path's resolver call is the canonical one, not the probe's `with_graph_read` wrapper.
- If both probe and bench produce NO_ANCHORS → H1 fully confirmed, route to ISS-162.
- If bench resolves anchors but probe doesn't → probe is invalid, file ISS-166 (or amend ISS-165 AC-1) to use bench-instrumented version.

## Filed because

Trader's last compact summary noted the AC-1 probe as the explicit next step. The probe ran (PID unknown, finished 20:26 EDT) and the result is striking but has a configuration-validity hole. Filing the finding + caveat as an artifact so whichever instance picks this up next doesn't burn cycles re-discovering the drift.

## Author

RustClaw main, 20:38 EDT 2026-05-26. Not normative; potato to ratify the routing decision (ISS-162 next vs probe-revalidation first).
