# ISS-030: Signal Taxonomy Re-architecture — SelfState/OtherState × Raw/Interpreted

**Status:** superseded by v0.3 DESIGN §3.7 (Cognitive state model — Telemetry / Affect / Empathy)
**Severity:** resolved at design level; implementation tracked as part of v0.3 §3.7 rollout
**Milestone:** v0.3 (moved up from v0.4 — folded into §3.7 rather than deferred)
**Related:**
- v0.3 DESIGN §3.7 — definitive architecture
- v0.3 DESIGN review r1 (findings A4, A5 surfaced the symptom)
- `docs/v0.3-working-memory/09-interoceptive-gating-a4-a5-analysis.md` (initial analysis)
**Filed:** 2026-04-24
**Superseded:** 2026-04-24 (same day — §3.7 written in response)

## Resolution

§3.7 supersedes this ticket's two-axis proposal (self/other × raw/interpreted) with a simpler **three-layer** model whose semantics turned out cleaner in review:

- **Telemetry** = raw self-sensing (body signals)
- **Affect** = interpreted self-state (constructed emotion, metacognition)
- **Empathy** = other-directed perception (social signals)

The self/other axis survives (Affect vs Empathy), but the raw/interpreted axis collapsed into "Telemetry feeds Affect via explicit subscription" rather than being materialized as a second schema dimension. This is both simpler and matches the neuroscience better (Barrett 2017: emotion = interoception + context, not a separate raw/cooked layer).

Enforcement is stronger than originally planned: boundary rule #2 in §3.7 makes the one-way Telemetry → Affect edge a compile-time invariant via `cargo-deny` workspace rules, not a coding discipline.

Follow-up work carried forward to v0.4:
- Schema inducer for emergent Affect/Empathy dimensions beyond the fixed 8-dim fingerprint
- Opt-in `contagion_coefficient` config (Empathy → Affect weighted pull)
- Entity.empathy_signature field (other-directed affect storage)

---

## Historical analysis (preserved for record)

## TL;DR

The current `SignalSource` flat enum mixes **two orthogonal dimensions** that should be separated:

1. **Signal direction axis**: self (感知自己) vs other (感知他人/empathy)
2. **Abstraction level axis**: raw sensing vs interpreted feeling

This produces **four quadrants**, but the code collapses them into one flat 10-variant enum plus a single `DomainState` struct, causing:
- `VoiceEmotion` (an **empathy** signal — reading the user) is classified as "interoceptive" and mixed into the agent's **self-perception** computation
- `SomaticMarker` docstring says "empathy valence" but code uses self-valence (concept confusion committed in source)
- `compute_global_arousal` in hub.rs averages arousal across all sources regardless of category, producing a meaningless "is the agent aroused because the user is agitated, or because token budget is low, or because SOUL-misalignment score is high?"
- Downstream decision paths cannot distinguish system-level signals (throttle/retry) from cognitive-level signals (encoding depth, consolidation priority) from social-level signals (response tone, pacing)

## Concept Correction — Not Three Pipelines, Two Axes

Earlier analysis (09-a4-a5) proposed a 3-way split: Emotion / Interoception / Empathy. This was wrong — Emotion and Interoception are not sibling categories. Per modern affective neuroscience (Lisa Feldman Barrett, constructed emotion theory):

> **emotion = interoception + context + conceptualization**

Emotion is the **interpreted** layer on top of **raw** interoception. They live on the same "self-perception" axis, just at different abstraction levels.

Empathy is genuinely independent — it reads **external** signals (other), not internal ones.

## Target Architecture

```
                       Signals
                          │
              ┌───────────┴───────────┐
              │                       │
          Self Axis               Other Axis
       (感知自己)               (感知他人/empathy)
              │                       │
       ┌──────┴──────┐         ┌──────┴──────┐
       Raw      Interpreted   Raw      Interpreted
  (sensing)     (feeling)   (sensing)   (feeling)
```

Proposed Rust types:

```rust
pub struct InteroceptiveState {   // keep the name; semantics = SELF only
    // Raw layer (sensing)
    pub anomaly_level: f64,
    pub operational_load: f64,
    pub execution_stress: f64,
    pub cognitive_flow: f64,
    pub resource_pressure: f64,
    pub feedback_success_rate: f64,
    // Interpreted layer (feeling)
    pub valence: f64,
    pub arousal: f64,
    pub confidence: f64,
    pub alignment: f64,
}

pub struct EmpathyState {
    // Raw layer (external perception)
    pub voice_arousal_raw: f64,
    pub text_sentiment_raw: f64,
    // Interpreted layer
    pub user_valence: f64,
    pub user_arousal: f64,
    pub user_engagement: f64,
}
```

Signal sources split similarly:

```rust
pub enum SelfSignalSource {
    // Raw
    Anomaly, OperationalLoad, ExecutionStress, CognitiveFlow, ResourcePressure, Feedback,
    // Semi-raw/semi-interpreted
    Accumulator,
    // Interpreted
    Confidence, Alignment,
}

pub enum OtherSignalSource {
    VoiceEmotion,   // v0.3
    TextEmotion,    // future
}
```

## Consumer Mapping (Why the Split Matters)

| Decision kind | Reads from | Rationale |
|---|---|---|
| System-level (throttle, fail-fast, retry, surface alert) | `InteroceptiveState` raw fields | Don't need a "feeling" label, just need "load high / anomaly high" |
| Cognitive (memory encoding depth, consolidation priority, SOUL alignment) | `InteroceptiveState` interpreted fields | Need the high-order label "valence negative / confidence low" |
| Social (response tone, topic sensitivity, pacing) | `EmpathyState` | Reading the user, not the agent |

Current code cannot enforce this — everything reads from one blended state.

## Why Deferred to v0.4

1. **v0.3 has no consumers that rely on the broken fusion.** §4.5 rewrite removes the only planned one (interoceptive gating on write path). Display layer uses the blended `InteroceptiveState` for UI only, not for decisions.
2. **This is a design sprint, not a refactor.** It needs:
   - Written proposal (this ticket is the starter, not the final design)
   - Review rounds (naming, migration strategy, external API impact)
   - Migration plan (deprecated aliases, downstream update path)
   - Then code
3. **v0.3 ship timeline is tight.** Doing this rushed would produce a fourth rewrite within a month. Worth doing right.

## v0.3 DESIGN Treatment

DESIGN-v0.3.md documents this as **known debt** in the relevant section (likely §4.5 or §10 future work):

> **Known debt**: `SignalSource` currently flattens two orthogonal axes (self-vs-other direction; raw-vs-interpreted abstraction level). VoiceEmotion is wrongly grouped with self-sensing signals; semantic decision paths cannot cleanly distinguish system-level from cognitive-level from social-level consumers. Re-architecture tracked in ISS-030, targeted v0.4. No v0.3 feature depends on this separation.

## Out of Scope for This Ticket

- Implementation in v0.3 (explicitly deferred)
- Deciding final names (`SelfState` vs `InteroceptiveState` vs `AgentState`) — subject to review
- Migration of existing `SomaticMarker` semantics (the "empathy valence" docstring/code mismatch gets fixed as part of this work, but isn't solved here)

## Next Steps (v0.4 design sprint)

1. Write formal proposal doc expanding this ticket
2. Circulate for review
3. Decide migration strategy (deprecated aliases, transition release, etc.)
4. Build migration plan with timeline
5. Execute
