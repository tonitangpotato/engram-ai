# A4 + A5 Analysis: Interoceptive Gating & Budget Backlog

**Context**: r1 findings A4 (interoceptive gating is philosophically wrong) + A5 (budget/backlog mechanism is unnecessary complexity). These are tightly coupled — both target §4.5.
**Date**: 2026-04-24
**Status**: Analysis complete — recommendation nuanced, partial disagreement with r1

---

## What r1 Says

### A4
- **Claim**: Using interoceptive stress/load to gate Stage 3/4/5 (extraction) is wrong
- **Reasons**: (1) misreads interoception as a throttle; (2) contradicts neuroscience (stress → stronger encoding in humans); (3) reverse-causality anti-pattern (hard sessions = most valuable = least indexed); (4) conflates resources with affect
- **Prescription**: Delete §4.5 gating. Interoception has **zero role on the write path**.

### A5
- **Claim**: The budget-check + skip + backlog state machine is unnecessary complexity for a problem that doesn't exist
- **Reasons**: Normal dialog never saturates; prompt blow-up is a prompt-design problem, not runtime; bulk import is the importer's job; rate-limits are infra-layer
- **Prescription**: Delete the backlog concept. Write path either completes or doesn't enter Stage 3 at all (based on **content**, not runtime state).

---

## Evidence Verification

### DESIGN §4.5 says exactly what r1 quotes
> *"when `interoceptive.stress` is high or `interoceptive.load` is saturated, the pipeline **degrades gracefully**: Skip Stage 3/4/5 LLM calls; just admit to L2, extract later during consolidation. Mark the episode with `pending_extraction = true`. Consolidation cycle picks up the backlog when load drops."*

Accurate quote. Also referenced in §6 consolidation step 4 ("Pending extraction — episodes skipped due to interoceptive load").

### Current code has zero implementation
Grep across the codebase for `pending_extraction`, `interoceptive gat`, `budget gat`, `skip Stage`, `operational_load`:
- `pending_extraction`: **0 hits** in any Rust file
- `OperationalLoad`: exists as a `SignalSource` enum variant, but no consumer uses it to gate anything in the ingest path (ingest path itself doesn't exist in v0.2 — only `store`/`recall`)

**Neither A4 nor A5 is a breaking-change risk.** Both are pure DESIGN-level deletions.

---

## Where r1 Is Right

- **A5 is 100% correct**. The budget/backlog state machine is cognitive overhead for a problem that isn't measured anywhere, isn't triggered by any concrete scenario in the doc, and would introduce 3+ new moving parts (budget signal, skip decision, backlog queue, pending_extraction column, consolidation catch-up job). Classic speculative-complexity anti-pattern. potato's rule: "no technical debt" — don't add infrastructure for imaginary problems.
- **A4's core intuition** — that interoceptive state shouldn't determine whether memories get indexed — is sound for the **affect portion** of interoception.
- The reverse-causality point holds for affective signals: stressful sessions = most valuable = should be *more* indexed, not less.

---

## Where r1 Is Partially Wrong (Nuance A4 Missed)

r1 says "Resource pressure (token budget, queue depth, latency SLOs) ≠ affective state" — and uses this to argue gating should be driven by resources, not emotions.

**But engram's interoceptive framework deliberately blurs this line already.** Looking at `crates/engramai/src/interoceptive/types.rs`:

```rust
pub enum SignalSource {
    // Engram-internal (affect/cognition)
    Anomaly, Accumulator, Feedback, Confidence, Alignment,
    
    // Runtime-sourced (resources)
    OperationalLoad,    // token budget, rate pressure
    ExecutionStress,    // loop depth, retries, tool failures
    CognitiveFlow,      // task completion, latency, coherence
    ResourcePressure,   // memory util, disk I/O, queue depth
    VoiceEmotion,
}
```

All 10 sources feed one `InteroceptiveState`. The framework itself treats resource signals and affective signals as siblings inside the same fusion.

This means §4.5's `interoceptive.stress` and `interoceptive.load` are actually **composite signals** — `load` is already largely resource-driven (OperationalLoad + ResourcePressure). `stress` is more affect-driven but isn't cleanly separated.

So r1's dichotomy "resources ≠ affect, gate on resources" is architecturally hard to enforce without either:
- (a) Splitting interoceptive signals back into separate `ResourceState` vs `AffectState`
- (b) Bypassing interoception entirely and reading raw runtime telemetry

**This is a real finding r1 didn't surface**: engram's interoceptive framework *itself* conflates resources and affect. §4.5 is a symptom; the upstream cause is the signal taxonomy.

---

## Refined Position

### On A5: Delete the backlog mechanism. Full agreement with r1.
No measured problem, no concrete scenario, adds state machine complexity. Content-based decision to enter Stage 3 (importance, novelty) is sufficient.

### On A4: r1's prescription is too absolute — "zero role on write path" is wrong. Correct position is more nuanced:

1. **Delete affect-driven gating**. If affective stress is high (anomaly spike, negative valence burst, low alignment), the memory encoding path should **not** be throttled. If anything, those episodes deserve *more* extraction (neuroscientific basis r1 cites is correct).

2. **Resource-driven throttling is legitimate but shouldn't live inside "interoceptive gating"**. If the token budget is genuinely exhausted, you stop calling the LLM — but that's an infra concern (retry/queue at the LLM client level), not a design-layer feature. Write path should fail cleanly, not invent a "backlog later" state.

3. **Long-term structural fix** (not v0.3 blocker): split the interoceptive signal taxonomy. `ResourceState` (ops/exec/resource/flow) should be a separate pipeline from `AffectState` (anomaly/accumulator/confidence/alignment/feedback). The current unified enum is a latent design problem that §4.5 made visible.

### On the "graceful degradation" use case
r1 dismisses this, but there is **one legitimate variant**: if an LLM call fails (rate limit, timeout, provider error), the episode should still land in L2 — just without graph extraction. That's not "graceful degradation based on interoception"; that's **error handling**. DESIGN should mention this, but frame it correctly:

> "If Stage 3/4/5 LLM calls fail or time out, the episode is admitted to L2 with `extraction_failed=true` (or similar error metadata). Extraction is **not** automatically retried in consolidation — failures are surfaced for operator review. This preserves the agent loop but does not hide failures."

This is fundamentally different from §4.5's "preemptively skip based on signals" — it's **reactive error handling**, not **proactive throttling**.

---

## DESIGN Changes

### §4.5 — Rewrite (don't delete entirely)

**Old** (~15 lines, budget gating based on interoceptive state):
> "Skip Stage 3/4/5 LLM calls when stress/load high; mark pending_extraction=true; consolidation cycle picks up backlog."

**New** (~15 lines, error-handling only):
> ### 4.5 Extraction failure handling
>
> Graph extraction (Stages 3/4/5) depends on LLM calls which can fail (rate limits, timeouts, provider errors). When this happens:
>
> - Episode is admitted to L2 with `extraction_error: Option<ExtractionError>` set
> - Entity/edge updates are skipped for that episode
> - The error is surfaced via logs and optionally via an `extraction_errors` metric
> - **Failures are not auto-retried in consolidation** — repeated LLM failures usually indicate infrastructure issues that need operator attention, not silent retry. An operator-facing `engramai reextract --failed` command can drive manual retry.
>
> **Explicitly not gated on**:
> - Interoceptive affect signals (stress, anomaly, valence) — high-affect episodes often deserve *more* indexing, not less
> - Operational load signals — if the LLM client itself is throttling/failing, that's handled at the infra layer
>
> The write path either completes all 6 stages synchronously or fails cleanly. No internal backlog state machine.

### §6 — Remove consolidation step 4 ("Pending extraction")
Replace with a one-line mention of operator-driven reextraction if needed.

### §9 roadmap
- **Phase 2**: remove "Interoceptive gating for graceful degradation" bullet. Replace with "Extraction error handling (reactive, not preemptive)".
- Saves ~2–3 days of Phase 2 work.

### §3.2 (optional add)
Add `extraction_error: Option<ExtractionError>` field to `MemoryRecord` — or push to `metadata` JSON. Probably metadata is fine; keeps schema lean.

---

## Effort Impact

| Change | Effort |
|---|---|
| DESIGN §4.5 rewrite | 30 min |
| DESIGN §6 step 4 removal | 10 min |
| §9 roadmap adjustment | 5 min |
| Phase 2 scope reduction | -2 to -3 days |
| Phase 4 scope reduction | -1 day (consolidation no longer has catch-up logic) |

**Net effect on v0.3 roadmap: ~3–4 days saved** (no backlog machinery to build/test).

No code changes today since nothing is implemented.

---

## Related — Interoceptive Signal Taxonomy Issue (New Finding)

**Surfaced by this analysis, not in r1**: the fact that engram's `SignalSource` enum mixes resource signals (OperationalLoad, ResourcePressure) with affect signals (Anomaly, Accumulator) in one pipeline is a latent design smell. §4.5 made it visible by naively gating on the fusion.

**Recommendation**: NOT a v0.3 blocker, but file as a future issue:
- Split `SignalSource` into `AffectSource` and `ResourceSource`
- Keep fusion only at display/reporting level (for the unified "interoceptive snapshot")
- Consumers that need to make decisions should read from the appropriate sub-pipeline, not the blended state

File as **ISS-030 (new)**: "Split interoceptive signal taxonomy — affect vs resource" once ISS tracking is back online. For now, noted here.

---

## Open Sub-Questions (for potato)

- Accept refined A4 position (keep reactive error handling, delete proactive gating)? Or go full r1 ("zero role on write path, not even error handling")?
- File the signal taxonomy split as a tracked future issue?
- For failed extractions — store error in `metadata` JSON, or add first-class `extraction_error: Option<ExtractionError>` field on `MemoryRecord`?

---

## Status

- [x] A4 evidence verified — DESIGN §4.5 says what r1 quoted
- [x] A5 evidence verified — no concrete scenario justifies backlog mechanism
- [x] Current code verified — zero implementation of either (no breaking change risk)
- [x] Surfaced nuance r1 missed: interoceptive framework itself conflates resources + affect
- [x] Refined prescription written (error handling vs r1's "delete everything")
- [x] Roadmap impact computed (~3–4 days saved)
- [ ] potato decision on refined A4 vs r1's absolute version
- [ ] potato decision on signal taxonomy split (file as issue?)
- [ ] DESIGN §4.5 / §6 / §9 rewrites (batched with other r1 findings)
