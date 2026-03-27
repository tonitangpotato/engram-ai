# Engram Future Improvements

> Analysis from 2026-03-11. Current architecture is pragmatically sound for agent memory, but the cognitive science labels overstate the implementation depth.

---

## Current State: Honest Assessment

| Claimed Mechanism | Actual Implementation |
|---|---|
| ACT-R activation | `base_activation + Σ(weight × spreading)` — a weighted sum formula |
| Hebbian learning | Co-recall counter +1 — two memories recalled together get strengthened |
| Consolidation | Sort by activation, merge similar memories |
| Emotional Bus | Manually passed -1.0 to 1.0 float (valence only, no arousal) |

**This works.** Agents can store, recall, frequently-used memories surface higher, related memories associate. As an engineering product it's solid.

**But it's not what the labels imply.** Real ACT-R has sub-symbolic computation, production rules, and goal buffers. Real Hebbian learning involves continuous weight updates and competitive learning. The current implementation captures the *spirit* but not the *depth*.

---

## Improvement Areas (Prioritized)

### 1. Fine-tune Embedding Model (High Impact, Medium Effort)
**Problem:** Recall quality depends on embedding similarity. Generic embeddings (e.g., `all-MiniLM-L6-v2`) don't understand agent-specific semantics.

**Solution:** Fine-tune the embedding model on agent interaction history.
- Training data: pairs of (query, successfully-recalled memory)
- Negative examples: queries that returned irrelevant results
- Could use contrastive learning (SimCSE-style)
- Even 1000 interaction pairs could significantly improve domain-specific recall

**Expected improvement:** Better precision on recall, fewer irrelevant results, understanding of user-specific terminology.

### 2. Learn ACT-R Parameters (High Impact, Low Effort)
**Problem:** Decay rate, spreading activation weights, and importance thresholds are hand-tuned constants.

**Solution:** Use agent's actual recall success/failure data to learn optimal parameters.
- Log every recall: what was queried, what was returned, was it used (reward signal)
- Bayesian optimization or simple grid search over parameter space
- Parameters to learn: `decay_rate`, `spreading_weight`, `importance_boost_factor`, `recency_weight`
- Could run as periodic background job during consolidation

**Expected improvement:** Memory retrieval better calibrated to actual usage patterns.

### 3. Associative Prediction (Medium Impact, Medium Effort)  
**Problem:** Hebbian learning is just a co-occurrence counter. It doesn't predict *which* memories will be needed given context.

**Solution:** Train a small model to predict memory relevance given context.
- Input: current conversation context (embedding)
- Output: probability distribution over stored memories
- Architecture: lightweight MLP or attention layer on top of embeddings
- Training: use historical recall patterns as labels

**Expected improvement:** Proactive memory surfacing — "you might need this" before the agent asks.

### 4. Richer Emotional Model (Low Impact for now)
**Problem:** Single valence dimension (-1 to 1) is too simplistic. No arousal, no emotional state transitions, no emotion-cognition interaction.

**Options (increasing complexity):**
- **Russell's Circumplex:** Add arousal dimension → 2D (valence × arousal)
- **Plutchik's Wheel:** 8 basic emotions with intensity levels
- **Appraisal Theory:** Emotion = f(relevance, certainty, control, agency)
- **Full affect dynamics:** ODE-based emotional state evolution (could use Liquid NN here!)

**Recommendation:** Russell's 2D model is the sweet spot — simple to implement, meaningfully richer than 1D valence.

### 5. Memory Layering (Medium Impact, High Effort)
**Problem:** All memories are stored flat. No distinction between working memory, episodic, semantic, procedural.

**Current state:** Type tags exist (`episodic`, `semantic`, `procedural`, `emotional`) but they don't affect retrieval or consolidation differently.

**Solution:** Different retrieval/consolidation rules per type:
- Episodic: fast decay, high specificity
- Semantic: slow decay, generalized (facts extracted from episodes)
- Procedural: no decay, activated by context matching
- This is closer to real ACT-R's module-based architecture

---

## What NOT to Change

- **One-shot learning** — This is a feature, not a limitation. Agents need instant memory, not batch training.
- **Interpretability** — Being able to read stored memories and debug weights is essential for trust.
- **File-based daily logs** — Complementary to Engram, not replaceable. Raw logs for manual review, Engram for semantic recall.
- **SQLite backend** — Simple, fast, zero-config. Don't over-engineer the storage layer.

---

## Connection to Liquid Neural Networks

If the Liquid Causal Graph project succeeds, there's a natural integration point:
- Engram's association network (memories as nodes, Hebbian weights as edges) could be modeled as a small LNN
- Each memory = a liquid neuron, associations = ODE connections
- Recall = signal propagation through the liquid memory graph
- This would give *continuous, time-aware* memory dynamics instead of discrete counter updates

But this is speculative and depends on proving LNN value in the causal graph first.

---

*Last updated: 2026-03-11*
