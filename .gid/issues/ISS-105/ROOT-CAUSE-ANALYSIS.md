# ISS-105 — Root Cause Analysis

**Date:** 2026-05-06
**Status:** analysis (pre-implementation)
**Supersedes:** the original `issue.md` framing ("4 sub-plans hardcode K")

---

## 1. The shallow framing (what we thought yesterday)

> "Hybrid has 4 sub-plans (Factual / Episodic / Abstract / Affective). Each
> one hardcodes its own K. Fix: make them all read `query.limit`."

This framing motivated the original `issue.md`. **It is wrong in two
ways:**

1. **2 of the 4 sub-plans are actually fine.** Episodic already reads
   `query.limit` (`episodic.rs:527`); Affective already derives its
   K from `query.limit` via `min(3 × requested_k, 60)`
   (`budget.rs:242`).
2. **"Make them all read `query.limit`" misses the point.** The K in
   each sub-plan is **not the same kind of K**. A uniform interface
   (`sub_plan_k(query.limit)`) would paper over real semantic
   differences and create a worse design than what's there now.

The right framing requires understanding what each sub-plan's K
*means*, then asking: what does the user's `query.limit = K` imply for
each of those internal Ks?

---

## 2. What §7.3 actually says

From `.gid/features/v03-retrieval/design.md` §7.3 (verbatim):

```
- max_anchors: 5 (Factual) — cap on entities resolved from query tokens
- max_hops: 1 — cap on edge traversal depth
- max_edges_visited: 500 — aggregate cap across all anchors
- max_candidates: 1000 — cap on memories passed to rerank
- K_seed: 10 (Associative) — seed recall size
- K_pool: 100 (Associative) — candidate pool after edge-hop expansion
- K_seed_affective: 3 * requested_k, capped at 60 (Affective)
```

**Key observations:**

- `max_candidates: 1000` is the **fuse-input total ceiling**. It's
  decoupled from `query.limit`.
- Only `K_seed_affective` is defined as a function of `requested_k`.
  Everything else is a fixed constant.
- The constant `10` for `K_seed` was implicitly chosen for "default
  query asks for top-10". When `query.limit = 50` arrives, this
  constant **silently undersamples**.

---

## 3. Per-sub-plan K semantics (the truth)

| Sub-plan | What its "K" actually controls | Reads `query.limit`? | Status |
|---|---|---|---|
| **Factual** | `max_anchors × memory_limit_per_entity` — a 2D fanout budget: "how many entities × how many memories per entity" | ❌ Hardcoded `5 × 50 = 250 ceiling` | **Broken at high K** |
| **Episodic** | Time-window memories returned — bounded by the window itself, then truncated to `query.limit` | ✅ `episodic.rs:527` reads `query.limit` directly | **Correct** |
| **Abstract** | `k_topics` — number of seed topics before traceability expansion. Topics fan out via L5 traceability, so each topic produces multiple memories. | ❌ Hardcoded `DEFAULT_K_TOPICS = 10` | **Broken at high K** |
| **Affective** | `k_seed = min(3 × requested_k, 60)` — explicit overfetch ratio | ✅ `budget.rs:242 + affective.rs:411` | **Correct** |
| **Associative** (top-level, not sub-plan) | `K_seed = 10` (fixed); `K_pool = 100` | ❌ Was hardcoded; ISS-104 patched it to read `query.limit` directly | **Patched but inconsistent with §4.5** |

**The shape of the problem:** 2 sub-plans use `query.limit`, 2 don't,
and Associative was hot-patched in ISS-104 with a *different* formula
than Affective uses. There is no consistent rule.

---

## 4. The first-principles question: what should K mean?

User contract:

> `query.limit = K` means **"return K relevant memories"**.

Internal consequence:

> For fuse_rrf to *select* K good results (rather than just *merge*
> available candidates), each sub-plan must contribute **more than K**
> candidates. The ratio above K is the **overfetch ratio α**.

**Why overfetch is required, not optional:**

If each sub-plan contributes exactly K candidates and fuse outputs K,
fuse is doing **set union with deduplication, not selection**. The
quality of the top-K is bounded by the quality of each sub-plan's
top-K — fuse adds no signal.

If each sub-plan contributes α·K candidates (α > 1), fuse can:

1. Cross-reference (memories that surface in multiple sub-plans rank
   higher in RRF — a real signal).
2. Filter (memories that surface only in one sub-plan with a weak
   score get dropped).
3. Re-rank (the top-K of fuse output is genuinely a selection from a
   larger pool, not a coincidence).

**This is exactly what `K_seed_affective = 3 × requested_k` encodes.**
The Affective sub-plan was designed with this principle in mind. It's
the only one that was.

---

## 5. Can α = 3 be generalized?

§4.5 (Affective) chose α = 3 as a design constant. The question:
**does the same α apply to Associative, Abstract, and Factual?**

### 5.1 Associative

Associative does:
1. Hybrid recall → `K_seed` candidates
2. Edge-hop expansion → up to `K_pool = 100` candidates
3. Fuse with other sub-plans

The `K_pool` cap (100) is the real ceiling here. Setting
`K_seed = α × requested_k = 3 × 50 = 150` would exceed `K_pool` —
which means the formula must be `min(α × requested_k, K_pool)`.

**For α = 3:**
- `query.limit = 10` → `K_seed = 30` (well under `K_pool = 100`) ✅
- `query.limit = 50` → `K_seed = 100` (saturates `K_pool`) ⚠️
- `query.limit = 100` → `K_seed = 100` (saturated) ⚠️

This is consistent — `K_pool` already caps the worst case.

**Verdict:** α = 3 is fine for Associative, but `K_pool` may need to
grow (separately) if we want to support genuinely large K. For now,
`K_pool = 100` is a known limit.

### 5.2 Abstract

Abstract does:
1. Topic-vector recall → `k_topics` seed topics
2. L5 traceability expansion → each topic yields ~M memories
3. Fuse

The expansion factor M (memories per topic) is data-dependent — in
practice 3-10. So setting `k_topics = α × requested_k = 3 × 50 = 150`
would yield ~450-1500 memories before fuse — way over `max_candidates
= 1000`.

**This is the case where α = 3 doesn't directly fit.** Abstract has a
**multiplicative expansion** that Affective and Associative don't
have. The right formula is:

```
k_topics = min(α × requested_k / E, k_topics_max)
```

where E = expected expansion factor per topic (≈ 5).

**For α = 3, E = 5:**
- `query.limit = 10` → `k_topics = 6` (slightly less than current 10 — fine, current 10 was over-budget)
- `query.limit = 50` → `k_topics = 30`
- `query.limit = 100` → `k_topics = 60`

**Verdict:** α = 3 generalizes if we account for Abstract's expansion.
The formula becomes `k_topics = min(α·K/E, cap)` where E ≈ 5.

### 5.3 Factual

Factual does:
1. Resolve N entities from query tokens (capped at `max_anchors = 5`)
2. For each entity, retrieve up to `memory_limit_per_entity = 50` memories
3. Fuse (max candidates = `N × memory_limit_per_entity`)

The K here is **two-dimensional**. Two cases:

- **Wide query** (many entities mentioned, e.g. "what did Alice and
  Bob say last month?"): N ≈ 5, `mem_per_entity` matters for fanout.
- **Narrow query** (one entity, e.g. "tell me about Bob"): N = 1,
  `mem_per_entity` *is* the K.

**The right factorization:**

- `max_anchors = 5` is **structural** — it's about query parsing, not
  result count. Don't change it.
- `mem_per_entity` is the per-entity recall budget. It should scale
  with `query.limit`.

```
mem_per_entity = min(α × requested_k / max_anchors_actual, mem_cap)
```

where `max_anchors_actual` is the actual number of anchors found (1-5).

**For α = 3:**
- 1 anchor, K = 10 → `mem_per_entity = 30`
- 1 anchor, K = 50 → `mem_per_entity = 150`
- 5 anchors, K = 50 → `mem_per_entity = 30`

This makes the user-facing K invariant to entity count, which is
correct — user asked for 50 results, doesn't care if they came from 1
entity or 5.

**Verdict:** α = 3 generalizes once we recognize Factual's K is
2D. `max_anchors` stays 5; `mem_per_entity` scales with `α·K /
max_anchors`.

### 5.4 Episodic — already correct, don't touch

Episodic recalls memories within a parsed time window. The window
size comes from the time expression in the query ("last week", "in
2024"), not from K. Within the window, results are truncated to
`query.limit`.

This is the **only sub-plan whose K semantics matches user intent
directly**: K = "how many results to return". No overfetch needed
because there's no fusion ranking happening before truncation —
Episodic is its own pipeline.

**Verdict:** Don't change. (And don't apply α=3 — it would over-recall
without benefit.)

### 5.5 Affective — already correct, already conformant

```
K_seed_affective = min(3 × requested_k, 60)
```

This is the template. α = 3, hard cap = 60.

**Verdict:** Don't change.

---

## 6. The unified rule

After this analysis, the rule is:

> **All sub-plans that contribute to fuse_rrf must overfetch by α = 3
> over `query.limit`, modulated by their own structural factors
> (expansion ratio, anchor count, hard caps).**

Per-sub-plan formulas:

```
Associative:  K_seed       = min(3 × query.limit, K_pool=100)
Abstract:     k_topics     = min(3 × query.limit / E≈5, k_topics_max)
Factual:      mem_per_ent  = min(3 × query.limit / max_anchors_actual, mem_cap)
Affective:    K_seed_aff   = min(3 × query.limit, 60)            ← unchanged
Episodic:     limit        = query.limit                          ← unchanged
```

**This is not "调参 to LoCoMo".** α = 3 comes from the §4.5 design
decision (Affective). We are **generalizing an existing design
principle**, not curve-fitting to benchmark.

---

## 7. Why this is root fix, not patch

A patch would be: "make Factual / Abstract read `query.limit` somehow,
make the LoCoMo number go up." That's what ISS-105's old framing
implied.

The root fix is:

1. **Recognize** that the design already had a coherent overfetch
   principle (Affective §4.5).
2. **Extend** that principle to the other sub-plans, accounting for
   their structural differences (expansion factor, dimensionality).
3. **Update** design.md §7.3 to make the principle explicit and apply
   uniformly.
4. **Implement** the uniform principle.
5. **Verify** on LoCoMo (a regression check, not a tuning target).

If LoCoMo doesn't move after this, the answer is **not** "try α = 4".
The answer is "the α = 3 principle doesn't generalize as expected —
go back to step 1 and find out why". That's the discipline that
separates principled engineering from benchmark hacking.

---

## 8. Risks and unknowns

### 8.1 The expansion factor E for Abstract

I estimated E ≈ 5 (memories per topic from L5 traceability). This is
a guess. If E is actually 20, then `k_topics = α·K/E` may be too small.

**Mitigation:** Measure E empirically once, then bake it into the
formula as a constant. This is **measuring an internal property of
the system**, not tuning to a benchmark — totally legitimate.

### 8.2 K_pool = 100 is a hard ceiling for Associative at large K

For `query.limit ≥ 34` (since `3 × 34 = 102 > 100`), Associative
saturates at 100 candidates. This means the α = 3 promise breaks down
at high K for this sub-plan.

**Mitigation:** Either accept the saturation (K_pool = 100 is by
design for cost reasons), or raise K_pool — that's a separate
decision, not part of ISS-105.

### 8.3 max_candidates = 1000 is the fuse-stage hard ceiling

Sum of all sub-plan candidates at high K:
- Associative: ≤ 100
- Abstract: ≤ k_topics_max × E ≈ 60 × 5 = 300
- Factual: ≤ 5 × 150 = 750 (at K=50)
- Affective: ≤ 60
- Episodic: ≤ query.limit (e.g. 50)

Total at K=50: 100 + 300 + 750 + 60 + 50 = **1260** > 1000.

**Mitigation:** This is a real interaction. Either raise
`max_candidates`, or accept that Factual gets clipped at the
fuse-input stage. Investigate before implementing.

### 8.4 Two correct sub-plans don't conform to the formula

Episodic uses `α = 1` (no overfetch); Affective uses `α = 3` with cap
60. The unified rule says "α = 3 everywhere" — but Episodic's α = 1
is *correct* (it's not contributing to ranking competition).

**Resolution:** The rule isn't really "α = 3 everywhere". It's "α = 3
for any sub-plan whose output is ranked against other sub-plans in
fuse". Episodic does join fuse, but its semantics (time-bounded) make
overfetch wasteful. **Document this exception explicitly** in §7.3.

---

## 9. Proposed execution

1. **Update `design.md` §7.3** — replace constants with formulas, document
   the α = 3 principle and Episodic exception. (Design change, needs
   review from potato before code.)
2. **Empirically measure E** for Abstract (one-shot benchmark, not a
   tuning loop).
3. **Investigate `max_candidates` interaction** (§8.3) — either raise to
   2000 or document the clipping.
4. **Implement** the formulas in `orchestrator.rs` and per-plan modules.
5. **Run RUN-0024 K=15** as regression check on LoCoMo. Expectation:
   J-score moves up modestly (because the α = 3 principle now applies
   uniformly). If it doesn't move, return to first principles — do
   not adjust α.
6. **Run RUN-0025 K=50** to validate that high-K behavior now works
   end-to-end (the original goal of ISS-104).

---

## 10. What this analysis changes about ISS-105

Old framing:
> Fix 4 hardcoded sub-plans by making them read `query.limit`.

New framing:
> Generalize the §4.5 overfetch principle (α = 3) to all
> fuse-contributing sub-plans, with per-plan modulation for
> structural factors. Keep Episodic and Affective unchanged. Update
> design.md §7.3 to document the unified rule. Implement, then verify
> on LoCoMo as regression check (not tuning target).

The scope is roughly the same in lines-of-code, but the **mental
model is fundamentally different**, and the design.md update is part
of the deliverable, not an afterthought.

---

*End of analysis. Awaiting potato review before implementation.*
