# Requirements Review: Knowledge Synthesis (R2 — Functional/Purpose Review)

**Document**: `.gid/features/knowledge-synthesis/requirements.md` (v3)  
**Review Date**: 2026-04-09  
**Review Angle**: Product/functional — does this feature serve engram's purpose?  
**Reviewer**: RustClaw

---

## Engram's Purpose

Engram is **memory for AI agents**. Its job: agents store experiences → agents recall relevant context → agents behave more intelligently over time. The users are agent developers (including ourselves — RustClaw, OpenClaw).

The core value proposition: **the more an agent uses engram, the smarter it gets.** If memory is just a growing pile of text, SNR degrades over time and the agent gets *dumber* (more noise in context). Knowledge Synthesis should fix this.

---

## FINDING-1 ✅ Applied [🔴 Critical]: The recall problem is unaddressed

**The single most important question for this feature: how do insights improve recall quality?**

The doc says GUARD-5: "Insights must participate in normal recall." But there's zero specification of HOW. Right now engram's hybrid search returns top-K by score. After synthesis:

- An insight is one memory competing with 8,000+ others
- Source memories are demoted but still exist (GOAL-7)
- The same query that used to return 5 specific facts now might return 1 vague insight + 4 random memories

**The actual user-facing value of synthesis is: when I query "what does potato prefer?", I get a coherent user model instead of 50 fragments.** But nothing in the requirements guarantees this outcome. SC-2 comes closest ("insight appears in top 3 results") but it's a success criterion, not a GOAL with enforceable behavior.

**Suggested fix**: Add a GOAL: "When an insight and its source memories are both candidates for a recall result, the insight must rank higher than any individual source memory for queries matching the cluster topic. Source demotion (GOAL-7) combined with insight importance must ensure this ordering naturally through the existing scoring pipeline — no special-case recall logic."

This makes the recall improvement an explicit, testable requirement instead of an assumed side effect.

---

## FINDING-2 ✅ Applied [🔴 Critical]: Synthesis without retrieval integration = dead feature

Related to FINDING-1 but more fundamental. Consider the actual usage cycle:

1. Agent calls `recall("Rust debugging tips", 5)`
2. Gets back 5 results — are any of them insights? Who knows
3. Agent stuffs them into LLM context
4. LLM sees... what exactly?

There's no requirement for the caller to know whether a result is an insight or a raw memory. There's no requirement for insights to be presented differently. There's no requirement for the agent to prefer insights over raw memories.

In practice, if an agent's prompt says "here are relevant memories:" and dumps 5 results, an insight ("potato is a Rust-first developer who values speed, testing, and first-principles thinking") is dramatically more useful than 5 raw memories. But the agent doesn't know to treat them differently.

**Suggested fix**: This doesn't need a new GOAL — GOAL-9 (API) should include: "The recall API must indicate when a result is a synthesized insight (e.g., via the existing `memory_type` or metadata field). Callers can filter for insights-only or raw-only. The default mixed recall naturally ranks insights higher due to their importance and embedding quality."

---

## FINDING-3 ✅ Applied [🟡 Important]: GOAL-4 gate check is over-engineered for the actual problem

The gate check has 4 signals, 3 classification outputs, and a target of ≤20% LLM rate. This is smart engineering. But step back — **what's the actual cost we're avoiding?**

- A synthesis cycle runs at most 5 LLM calls (GUARD-2)
- It runs on heartbeat (every ~hour) or manual trigger
- That's ~5 Haiku calls per hour max = $0.003/hour = $2/month

The gate check's complexity (entity overlap analysis, temporal spread, existing coverage) will cost more in implementation time and maintenance than the LLM calls it saves. The ≤20% target is solving a problem that costs $2/month.

**I'm NOT saying remove the gate check.** The near-duplicate filter (similarity > 0.92 → skip) and existing coverage check (already covered → skip) are genuinely useful for quality, not cost. But the framing should be: **gate check exists to improve synthesis quality (don't synthesize garbage), not primarily for cost control.**

**Suggested fix**: Reframe GOAL-4's motivation. Change "The target is ≤20% of discovered clusters reaching LLM" to something like: "The gate check's purpose is synthesis quality — filtering out clusters that would produce low-value insights (near-duplicates, too-recent memories, already-covered topics). A secondary benefit is reduced LLM cost." Remove the specific ≤20% target (it's arbitrary and creates pressure to over-filter).

---

## FINDING-4 ✅ Applied [🟡 Important]: Source demotion (GOAL-7) needs more careful thought about what happens to recall

GOAL-7 says: multiply importance by 0.5, move toward Archive. This means after synthesis of a "Rust debugging" cluster:

- 10 specific debugging tips go from importance 0.6 → 0.3, layer Working → Archive
- 1 insight "Rust debugging principles" has importance... what? Not specified.

If the insight gets default importance (0.5), it barely outranks the demoted sources (0.3). If the query is specific ("how to fix borrow checker errors"), a specific source memory might still be more relevant than the general insight — but it's been demoted.

**The demotion factor should be calibrated against the insight's importance, not arbitrary.**

**Suggested fix**: Add to GOAL-7: "The synthesized insight must have importance ≥ max(source importances). Source demotion must reduce source importance to below the insight's importance, ensuring the insight outranks sources in recall for topic-level queries."

---

## FINDING-5 ✅ Applied [🟡 Important]: Missing "what does a good insight look like?" — the LLM prompt is the entire product

GOAL-2 validates format (not too similar to source, not too short). But the QUALITY of the insight — the actual text content — depends entirely on the LLM prompt. And the requirements say nothing about what makes a good insight.

Consider two possible insights from the same cluster about potato's preferences:
- Bad: "potato has various preferences about coding" (technically valid, passes GOAL-2 checks)
- Good: "potato is a first-principles thinker who prioritizes: Rust, speed of iteration, thorough testing, action over discussion. He builds towards financial freedom through AI agent products."

The difference is entirely in the LLM prompt design. This is a design concern, not a requirements concern, BUT the requirements should specify the quality bar.

**Suggested fix**: Add to GOAL-2: "The generated insight must be **actionable** — an agent reading only the insight (without its sources) must be able to make better decisions than an agent reading any single source memory. An insight that merely summarizes ('user has several preferences about X') without capturing the specific pattern is a validation failure."

---

## FINDING-6 ✅ Applied [🟢 Minor]: NON-GOAL-2 (multi-level synthesis) might be a mistake

The doc says "only raw memories → insights, no insight-from-insight." But the highest value of knowledge synthesis is exactly the hierarchical case:

- Level 1: "potato prefers Rust" + "potato prefers action" + "potato prefers testing" → Insight: "potato's engineering values"
- Level 2: "potato's engineering values" + "potato's business goals" + "potato's daily routine" → Schema: "potato's operating model"

The Level 2 schema is worth 10x more than any Level 1 insight for an agent's long-term behavior. Explicitly excluding it as a NON-GOAL is fine for v1 scoping, but the requirements should note this is a **deliberate limitation**, not a design principle.

**Suggested fix**: Reword NON-GOAL-2 to: "This version implements single-level synthesis only (raw memories → insights). Multi-level synthesis (insights → schemas → worldviews) is the planned next evolution and is the primary reason GOAL-3 provenance chains are required. The data model must not prevent multi-level synthesis from being added later."

---

## FINDING-7 ✅ Applied [🟢 Minor]: SC-4 performance target is oddly specific

"End-to-end synthesis cycle completes in <30s for a database of 1,000 memories (excluding LLM latency)" — this is fine but the real performance concern is different. With 8,000+ memories (our actual DB), the cluster discovery phase (GOAL-1) doing pairwise comparison is the bottleneck. 8,000 × 8,000 = 64M comparisons. The SC should target the actual bottleneck.

**Suggested fix**: Change SC-4 to: "Cluster discovery (GOAL-1) completes in <60s for 10,000 memories. Full synthesis cycle (excluding LLM) completes in <120s for 10,000 memories."

---

## ✅ What's Good (things that genuinely serve engram's purpose)

1. **Gate check concept (GOAL-4)** — even if the cost framing is off, the quality filtering is genuinely important. Not every cluster should become an insight.

2. **Source demotion (GOAL-7)** — this is the key mechanism for SNR improvement. Without it, insights just add more stuff to an already noisy DB.

3. **Idempotent synthesis (GOAL-5)** — critical for a system that runs on heartbeat. Without this, you get duplicate insights flooding the DB.

4. **LLM graceful degradation (GOAL-12)** — engram must work without LLM. Good.

5. **NON-GOAL-1 (no real-time)** — correct decision. Synthesis on every write is the trap Mem0/Hindsight fell into.

6. **GUARD-1 (no data loss)** — fundamental. Source memories are evidence; insights are interpretations. Never destroy evidence.

---

## 📊 Summary

| Severity | Count | Findings |
|---|---|---|
| 🔴 Critical | 2 | FINDING-1, FINDING-2 |
| 🟡 Important | 3 | FINDING-3, FINDING-4, FINDING-5 |
| 🟢 Minor | 2 | FINDING-6, FINDING-7 |
| **Total** | **7** | |

### Core Issue

The requirements describe a **knowledge creation engine** but don't close the loop on **knowledge delivery**. The synthesis → recall path is assumed, not specified. FINDINGs 1-2 are critical because without explicit recall integration, synthesis generates insights that nobody ever sees — a technically correct feature that provides no user value.

### Recommendation

Fix FINDING-1 and FINDING-2 before proceeding to design. The rest are improvements that can be applied now or during design review.
