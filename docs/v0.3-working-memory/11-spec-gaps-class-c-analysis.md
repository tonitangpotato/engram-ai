# Class C Analysis: Specification Gaps

**Context**: r1 Class C findings — specification gaps to close before implementation. C4 was superseded by A4/A5 analysis.
**Date**: 2026-04-24

---

## C1 — Entity.activation and Entity.valence update rules missing

### r1's Claim
§3.3 defines `Entity.activation: f64` and `Entity.valence: f64` but doesn't specify:
- When does activation decay? Same curve as MemoryRecord?
- How is valence learned from mentions?
- Does valence regress when mentioning memories decay?

### Verification

Read §3.3 — confirmed. Struct definition lists fields but no update-rule subsection. The only hint is `valence: f64, // affective tag (learned from mentions)` which says nothing concrete.

### Recommendation — Add §3.3.1 subsection

Proposed spec:

```
### 3.3.1 Entity state update rules

**Activation (ACT-R, same curve as MemoryRecord)**:
- Initialized to 1.0 on first mention
- Boosted by +0.5 on each subsequent mention (clamped to ≤ 2.0)
- Decays with same dual-trace ODE as MemoryRecord (shared μ1/μ2 params)
- Running in the same consolidation cycle as memory activation

**Valence/arousal (learned, EMA with decay)**:
- On each mention m with affect (valence_m, arousal_m):
  entity.valence = α · valence_m + (1-α) · entity.valence
  entity.arousal = α · arousal_m + (1-α) · entity.arousal
  with α = 0.2 (slow drift, stable identity)
- When a mentioning memory is superseded or expired, affect IS NOT retroactively removed
  (entity identity carries the affective history it accumulated; retrospective rewrite
   is left to explicit retro-evolution in §6 step 5)
- On retro-evolution: entity valence is recomputed from currently-valid mentions only

**Importance**:
- Not updated in write path
- Recomputed in consolidation as: 0.4·mention_count_normalized + 0.3·max_pin_rate
  + 0.3·|valence|·arousal_amplitude

**Somatic fingerprint**:
- Updated on each mention (weighted average of memory's 11-dim affect vector mapped
  to the 8-dim somatic space)
- Weight = mention's memory.importance
```

**Effort**: 1 hour to write + review. No code impact (Phase 1+ work).

### Status
- [x] C1 is a real gap
- [ ] Potato to approve proposed rules (especially α=0.2 and no-retro-remove policy)

---

## C2 — Multi-signal fusion weights not specified

### r1's Claim
§4.3 claims fusion of (embedding + alias + graph-context + temporal proximity) but no weights, no same-entity threshold, no tie-breaker. Phase 2 first week will be wasted guessing.

### Verification

§4.3 (checked by reading §4 structure):
- Lists the signals
- Says "§8.3 tuning plan" explicitly calls these guesses and promises post-launch tuning
- No initial values given anywhere

Confirmed. Real gap.

### Recommendation — Add initial values

r1 proposed `w_embed=0.5, w_alias=0.3, w_context=0.15, w_temporal=0.05, threshold ≥0.72`. These are defensible starting points. Two adjustments:

1. **Tie-breaker**: when two candidate entities score within 0.05 of each other, prefer the one with more recent `last_seen`. If still tied, prefer the one with higher `importance`. Explicit tie-breaking prevents non-deterministic merges.

2. **Alias weight breakdown**: "alias match" is ambiguous — exact string match vs fuzzy match vs Levenshtein < 2. Specify:
   - Exact canonical_name or alias hit: contributes 1.0 to alias signal
   - Fuzzy match (Levenshtein ≤ 2 on normalized form): contributes 0.6
   - Common alias pattern (e.g., "Dr. X" matching "X"): contributes 0.8
   - Multiple alias hits do not stack above 1.0

**Effort**: 2 hours to draft rules + sanity-check on mental model. No code impact (Phase 2 work).

### Status
- [x] C2 is a real gap
- [ ] Approve weights + threshold + tie-breaker policy

---

## C3 — Edge bi-temporality verification

### r1's Claim (speculative — r1 didn't have §3.4 excerpt)
*"Verify §3.4 defines both `valid_time` and `transaction_time`. If only one, G1 temporal guarantee can't be delivered."*

### Verification

Read §3.4 — **both are defined**:
```rust
pub valid_at: Option<DateTime<Utc>>,     // when the fact became true (real-world)
pub invalid_at: Option<DateTime<Utc>>,   // when it stopped being true
pub asserted_at: DateTime<Utc>,          // when engram learned it
```

`valid_at` + `invalid_at` = real-world validity interval (valid-time)  
`asserted_at` = when engram recorded it (transaction-time)

Both present. G1 is deliverable.

### Status
- [x] **C3 resolved — no change needed**. DESIGN §3.4 already meets the bi-temporal requirement. r1 flagged this as "verify"; verification passed.

One minor improvement: §3.4 should add a one-line comment clarifying the bi-temporal model so future readers don't have to reverse-engineer it:

> *"These three fields form a bi-temporal model: (`valid_at`, `invalid_at`) is valid-time (when the fact is true in the real world), `asserted_at` is transaction-time (when engram learned of it). Queries like `query_at(fact, t)` use valid-time; audit queries like 'when did we learn X?' use `asserted_at`."*

Trivial addition, ~3 lines.

---

## C4 — Interoceptive gating rules

**Superseded by A4/A5 analysis.** The gating mechanism is being deleted entirely (replaced with reactive error handling, not proactive signal-based skip). C4's "spell out the rules" question becomes moot — there are no rules because there is no gating.

See `09-interoceptive-gating-a4-a5-analysis.md` for details.

### Status
- [x] C4 resolved via A4/A5 deletion of §4.5 gating logic

---

## C5 — Query classification: one LLM call per recall breaks latency

### r1's Claim
§5.1 says "one cheap LLM call OR heuristic" — ambiguous. If LLM, every recall adds 200–500ms. If heuristic, show the heuristic. Recommended: heuristic first, LLM fallback.

### Verification

§5.1 text:
> *"Heuristic first (regex + keyword + dimension hits), LLM fallback only when heuristic is unsure."*

**§5.1 already says heuristic-first with LLM fallback.** r1 may have misread "one cheap LLM call OR heuristic" in §5.1's section header/intro without reading the following bullets.

But r1's deeper point stands: **the heuristic itself isn't specified.** "regex + keyword + dimension hits" is gestural. Which regexes? Which keywords map to which intent? What dimension thresholds?

### Recommendation — Add §5.1.1 heuristic specification

Draft:

```
### 5.1.1 Heuristic query classifier (default path)

The classifier produces an intent + confidence in O(1) without LLM calls.
Rules evaluated in order; first match with confidence ≥ 0.7 wins:

1. Temporal markers → Episodic:
   - Regex: /\b(yesterday|last (week|month)|on (Mon|Tue|...))\b/
   - Confidence: 0.9

2. Question words → Factual:
   - Regex: /^(who|what|when|where) (is|are|was|were) /
   - Keywords: "how old", "located", "works at"
   - Confidence: 0.85

3. Affect words → Affective:
   - Keywords from somatic lexicon: stuck, blocked, frustrated, excited, anxious
   - Dimension hit: query embedded into 11-dim affect space, max component > 0.3
   - Confidence: 0.75

4. Summary markers → Abstract:
   - Regex: /\b(summarize|what have we|main points about)\b/
   - Confidence: 0.80

5. Hybrid/fallthrough:
   - Any query not matching above with confidence ≥ 0.7
   - Triggers LLM fallback OR returns multi-intent with Factual default
   - Confidence: 0.5

LLM fallback is invoked only when all heuristic rules return confidence < 0.7.
Expected hit rate from heuristic alone: ~70-80% on typical agent traffic
(to be measured on LOCOMO).
```

**Effort**: 2–3 hours to refine rules + sanity-check on recent rustclaw query logs. Some code impact in Phase 3 but nothing Phase 1 blocking.

### Status
- [x] C5 heuristic mention present in §5.1 but under-specified
- [ ] Approve proposed §5.1.1 rules (or refine based on actual query log sample)

---

## Combined Recommendation

| Finding | Action | Effort | Phase |
|---|---|---|---|
| C1 | Add §3.3.1 Entity update rules | 1 hr | Phase 0 |
| C2 | Add fusion weights + tie-breaker | 2 hrs | Phase 0 |
| C3 | Resolved, add 3-line clarifying comment | 5 min | Phase 0 |
| C4 | Resolved via A4/A5 deletion | — | — |
| C5 | Add §5.1.1 heuristic spec | 2–3 hrs | Phase 0 |

Total Phase 0 addition for Class C: **~6 hours of doc writing**. Well inside Phase 0's 1-week budget.

---

## Open Sub-Questions (for potato)

- Approve C1 rules (especially α=0.2 EMA, no-retro-remove policy)?
- Approve C2 weights + threshold + tie-breaker policy?
- Approve C5 heuristic rules as starting point (before LOCOMO-based tuning)?

---

## Status

- [x] C1, C2, C5 are real gaps, drafted fixes
- [x] C3 resolved by reading §3.4 (bi-temporal already present)
- [x] C4 resolved by A4/A5 analysis (gating being deleted)
- [ ] Potato approval on proposed specifications
- [ ] Apply spec additions (batched with other DESIGN edits)
