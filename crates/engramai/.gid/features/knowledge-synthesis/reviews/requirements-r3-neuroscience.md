# Requirements Review: Knowledge Synthesis (R3 — Neuroscience & Function Alignment)

**Document**: `.gid/features/knowledge-synthesis/requirements.md` (v4)  
**Review Date**: 2026-04-09  
**Review Angle**: Does this feature faithfully model systems consolidation? Does engram's neuroscience stack remain coherent?  
**Reviewer**: RustClaw

---

## Engram's Neuroscience Stack (Current)

| Layer | Brain Model | Implementation |
|---|---|---|
| Encoding | Hippocampal fast-binding | `memory.add()` → Working layer |
| Retrieval | ACT-R activation (frequency × recency power law + spreading activation) | `recall()` → hybrid scoring |
| Synaptic consolidation | Murre-Chessa dual-trace (r₁ hippocampal → r₂ neocortical) | `consolidate()` → decay + transfer |
| Forgetting | Ebbinghaus R=e^(-t/S) + stability growth via spacing | `should_forget()`, `effective_strength()` |
| Association | Hebbian co-activation (fire together → wire together) | `record_coactivation()` → link strength |
| Emotional modulation | Amygdala priority (importance modulates consolidation rate) | `effective_alpha = alpha * (0.2 + importance²)` |
| Working memory | Miller's 7±2 capacity limit | `SessionWorkingMemory` |

This feature adds **Systems Consolidation** — the missing layer between synaptic consolidation (trace-level) and retrieval.

---

## FINDING-1 [🔴 Critical]: Sleep replay model is backwards — real brains replay, then cluster; this clusters, then synthesizes ✅ Applied

In the brain, systems consolidation works like this:

1. **Hippocampal replay** — during sleep, the hippocampus replays recent experiences (Sharp Wave Ripples). This isn't selective — it replays broadly.
2. **Neocortical pattern detection** — the neocortex, receiving replayed signals, detects recurring patterns across episodes. This is where schemas emerge.
3. **Schema assimilation** — new memories that fit existing schemas are rapidly integrated; those that don't remain hippocampus-dependent longer.

The requirements model it as: discover clusters first (using static similarity), then synthesize. This is actually closer to **semantic memory organization** (Tulving) than sleep replay. The distinction matters because:

- Real replay is **temporally ordered** — it replays recent memories more, which is why recent experiences consolidate faster. The requirements have no recency bias in cluster discovery (GOAL-1).
- Real replay is **repeated** — the same memory gets replayed many times across sleep cycles, gradually strengthening the neocortical representation. One synthesis pass doesn't capture this.
- Real pattern detection uses **activation overlap**, not just embedding similarity — two memories that activated the same neural population are clustered, even if their content differs. Engram has this signal (Hebbian co-activation) but GOAL-1 treats it as one of three equal criteria rather than the primary one.

**Suggested fix**: 
- Add recency weighting to cluster discovery: memories accessed/created in the last N consolidation cycles should have priority for cluster consideration (this mirrors the hippocampal recency bias in replay)
- Make Hebbian co-activation the primary clustering signal (it's the closest analog to "activated the same neural population"), with embedding similarity as secondary confirmation
- This doesn't require architectural changes — just reweight the criteria in GOAL-1

---

## FINDING-2 [🟡 Important]: Consolidation and synthesis are disconnected systems — should be one "sleep cycle" ✅ Applied

Right now `consolidate()` does:
1. Decay both traces (r₁, r₂)
2. Transfer working → core (α · r₁ · dt)
3. Interleaved replay of archive memories
4. Layer rebalancing

Knowledge synthesis (this feature) would be a separate call. But in the brain, these happen during the **same sleep cycle**. The replay that strengthens traces IS the same replay that discovers patterns. Separating them means:

- A memory could be synthesized into an insight but its traces don't reflect that it was "replayed"
- Consolidation might archive a memory that synthesis would have needed
- The agent has to call two separate functions — `consolidate()` and `synthesize()` — when the brain does one thing: sleep

**Suggested fix**: Add to requirements: "Synthesis should be integratable into the existing consolidation cycle as an optional phase. `consolidate()` continues to work standalone (trace-level only). A new `consolidate_with_synthesis()` (or a config flag) runs both in sequence: trace consolidation first, then knowledge synthesis on the freshly-consolidated state. This mirrors the brain's single sleep cycle."

This is NOT saying merge the implementations — it's saying the API should present them as one cognitive event when desired.

---

## FINDING-3 [🟡 Important]: Insight importance is set too rigidly — should use the same importance mechanisms as regular memories ✅ Applied

GOAL-7 says "insight importance ≥ max(source importances before demotion)". But in the brain, a schema's strength comes from how many traces activate it and how often it's accessed — the same ACT-R dynamics as regular memories.

Setting importance as a floor at creation time is fine for initial ranking. But after that, the insight should **behave like any other memory**:
- Accessed during recall → importance/activation increases (ACT-R power law)
- Not accessed → decays naturally (Ebbinghaus)
- Can be consolidated further (Murre-Chessa)

The risk: if insight importance is artificially maintained above sources, it creates a "zombie schema" — a synthesized insight that's no longer relevant but can't fade because its importance is pinned. In the brain, unused schemas DO weaken.

**Suggested fix**: Clarify in GOAL-7 or add to GOAL-14: "After creation, insights participate in ALL existing memory dynamics: ACT-R activation (frequency/recency of recall access), Ebbinghaus forgetting (decay if not accessed), Murre-Chessa trace transfer, and Hebbian co-activation. The initial importance floor ensures the insight starts with an advantage; natural dynamics determine whether it keeps that advantage over time. No special-casing of insight decay or activation."

---

## FINDING-4 [🟡 Important]: Hebbian links between insight and sources are not specified ✅ Applied

When the brain forms a schema, it doesn't just create an isolated new representation — the schema becomes **bidirectionally linked** to its constituent memories. Recalling the schema activates the memories; recalling a memory activates the schema. This is exactly what Hebbian links are for.

The requirements specify provenance chains (GOAL-3) as metadata relationships, but say nothing about Hebbian links between insight and source memories. These serve different purposes:
- Provenance (GOAL-3) = administrative record ("this insight came from these memories")
- Hebbian links = cognitive association ("activating this insight also activates these memories")

In practice: if an agent recalls an insight about "potato's engineering values", spreading activation through Hebbian links should also boost the specific memories ("potato said don't simplify", "potato prefers action over discussion"). This gives the agent both the high-level schema AND the specific evidence.

**Suggested fix**: Add a requirement: "After insight creation, Hebbian co-activation links must be established between the insight and each source memory. Link strength should be proportional to the source memory's contribution to the insight (e.g., embedding similarity between source and insight). These links participate in normal spreading activation during recall, enabling schema-to-evidence traversal."

---

## FINDING-5 [🟢 Minor]: GOAL-6 emotional modulation is shallow — amygdala does more than priority ordering ✅ Applied

GOAL-6 says emotional clusters get processed first. That's the weakest form of emotional modulation. In the brain, emotional significance affects:

1. **Priority** — yes, emotional memories consolidate first ✅ (GOAL-6 covers this)
2. **Consolidation rate** — emotional memories transfer hippocampal → neocortical faster. Already modeled in existing consolidation (`effective_alpha = alpha * (0.2 + importance²)`). But synthesis doesn't have an analog — emotional clusters don't get "deeper" synthesis.
3. **Resistance to decay** — emotional memories decay slower. Already modeled (importance modulates Ebbinghaus stability). But are emotional INSIGHTS more resistant to decay than non-emotional ones?

The current spec is fine for v1 — priority ordering is the minimum viable emotional modulation. But note this as a future extension area.

**Suggested fix**: No change needed for v1. Add a note to GOAL-6: "Future extension: emotional significance could also modulate insight quality requirements (lower validation thresholds for high-emotion clusters, since emotional coherence is a legitimate form of pattern), insight initial importance (emotional insights start stronger), and insight decay resistance."

---

## FINDING-6 [🟢 Minor]: "Within cluster" similarity (GOAL-1) should use spreading activation, not just static embedding ✅ Applied

GOAL-1 uses three static signals: embedding cosine similarity, entity overlap, and Hebbian link strength. These are computed from stored data. But ACT-R spreading activation — the mechanism that actually drives recall — is **context-dependent**. Two memories might have low embedding similarity but high co-activation in the right context.

Since synthesis runs offline (not in a specific recall context), there's no "current context" to drive spreading activation. This is a genuine limitation of batch processing vs. the brain's context-rich replay.

**Suggested fix**: No immediate action — this is an inherent limitation of batch synthesis. But add to NON-GOAL-2 or as a note: "Context-dependent clustering (where cluster membership depends on the recall context) is out of scope. The brain's replay is context-rich; our batch synthesis is context-free. This is a fundamental limitation of offline synthesis."

---

## ✅ What Aligns Well With Neuroscience

1. **Complementary Learning Systems mapping (GOAL-4)** — the fast hippocampal path (raw storage) vs slow neocortical path (synthesis) is exactly CLS theory. The gate check acting as a "is this worth slow-path processing?" filter is biologically plausible.

2. **Source demotion without deletion (GOAL-7)** — mirrors the brain's behavior: when a schema forms, the original episodes don't vanish, they become harder to access independently (childhood memories exist but are rarely recalled without a cue). GUARD-1 enforces this correctly.

3. **Idempotent synthesis (GOAL-5)** — reconsolidation! In the brain, retrieving a memory makes it labile again, allowing updating. Re-synthesis when a cluster grows mirrors this: the schema is "reconsolidated" with new evidence.

4. **Provenance chains (GOAL-3)** — while the brain doesn't maintain explicit provenance (source amnesia is a real phenomenon), for an AI system this is a crucial addition. The brain's lack of provenance is a bug, not a feature.

5. **LLM as neocortex analog** — the LLM performing pattern extraction from raw memories is a reasonable stand-in for neocortical pattern completion. The gate check filtering what reaches the LLM mirrors the hippocampal gate that determines which memories get replayed.

---

## 📊 Summary

| Severity | Count | Findings |
|---|---|---|
| 🔴 Critical | 1 | FINDING-1 |
| 🟡 Important | 3 | FINDING-2, FINDING-3, FINDING-4 |
| 🟢 Minor | 2 | FINDING-5, FINDING-6 |
| **Total** | **6** | |

### Core Assessment

The feature is **fundamentally sound** as a systems consolidation implementation. The CLS mapping, source demotion, idempotent synthesis, and LLM-as-neocortex analogy all hold up. The critical gap is that cluster discovery doesn't mirror how replay actually works (recency-biased, Hebbian-primary, temporally ordered). The important gaps are: synthesis should be presentable as part of the consolidation cycle (not a disconnected system), insights should participate in ALL existing cognitive dynamics (not just recall), and Hebbian links between insight and sources are missing.

None of these require architectural changes — they're refinements that make the neuroscience mapping more faithful while keeping the same implementation structure.
