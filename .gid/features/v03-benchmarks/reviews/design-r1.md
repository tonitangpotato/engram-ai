# Design Review — v03-benchmarks r1

> **Target:** `.gid/features/v03-benchmarks/design.md` (783 lines)
> **Requirements:** `.gid/features/v03-benchmarks/requirements.md` (8 GOALs: 5 P0 / 2 P1 / 1 P2)
> **Reviewer:** RustClaw (self-review, no sub-agent — context size demands it)
> **Date:** 2026-04-24
> **Prior reviews:** none (first formal review)

## Scope of this review

This is a **targeted r1 review** — NOT a full 27-check review-design pass. The design has already been through a cross-feature seam-consistency pass (9 seams closed, documented in issue notes). This review focuses on issues that seam-fixing wouldn't catch:

1. Requirements coverage & traceability
2. Internal consistency / contradictions
3. Measurability of gate semantics (can each gate actually be computed?)
4. Engineering integrity (shortcuts, debt, scope drift)
5. Gaps that would block implementation

## Overall assessment

**Quality: high.** Design is unusually mature for an r1 — ownership boundaries are explicit (§1.4), every GOAL has a concrete measurement section, gate semantics are formalized (§4), override path is pipeline-enforced (§4.4 Level 3), and reproducibility schema is concrete enough to implement directly.

**Shipping blockers found: 3 (all 🟡 Important, none 🔴 Critical).** The critical-path claims (P0 gates) are sound. The three flagged issues are gaps, not defects.

---

## Findings

### FINDING-1 🟡 Important — GOAL-5.8 has no failure gate defined

**Location:** requirements.md GOAL-5.8; design.md §4 (gate tables), §13 (traceability table)

**Observation:** GOAL-5.8 ("reproducibility record emitted per run") is listed as **P2 observability** in requirements.md. In design §13 it's traced to §6 + §7.3, but §4.1 (P0 gates), §4.2 (P1 gates), and §4.3 (order / no short-circuit) **never mention GOAL-5.8**. §10.1 summary table doesn't include it either.

**Why this matters:** GUARD-2 ("never silent degrade") says gates fail loud. But there's no gate that fails loud if the reproducibility record itself is missing or malformed. A release could ship with a corrupt/incomplete record and nothing would catch it — which defeats the entire point of GOAL-5.8 (reproducibility-by-contract, not by convention).

**Suggested fix:** Add a **meta-gate** in §4 for GOAL-5.8 with explicit semantics:
- Level: P1 quality gate (not P0 — a missing record doesn't make the build unsafe, only unreproducible).
- Metric: `record.schema_valid && all_required_fields_present && override_fields_present_iff_override_used`.
- Surface: rendered in §10.1 summary table as a separate line.
- Implementation hint: validation runs at the END of every driver, before exit.

Also add a line to the §13 traceability row for GOAL-5.8 pointing to the new §4 subsection.

---

### FINDING-2 🟡 Important — §9.3 anonymization pipeline is underspecified for a one-shot release precondition

**Location:** design.md §9.3 (rustclaw trace anonymization)

**Observation:** §9.3 says "run each episode through the anonymization transformer" and enumerates categories (named entities, URLs, timestamps) — but **does not specify**:

- **The entity-extraction mechanism.** Is it the v0.3 resolution pipeline itself? A separate regex-based stripper? An LLM pass? Each choice has different failure modes (v0.3 pipeline → circular dependency on the feature being benchmarked; regex → misses novel entity forms; LLM → non-deterministic).
- **What constitutes an acceptable miss rate.** The "manual review pass by potato" (step 3) is the only safety net. If the transformer leaks one email address, is that a blocker for the commit? There's no defined tolerance.
- **Failure handling.** If the anonymizer crashes on episode #143, does the whole corpus regenerate? Is episode #143 skipped and the corpus becomes 249/250? This affects the "exactly N=500" invariant in §3.3.

**Why this matters:** This is a **one-time precondition** feeding the cost harness (§3.3, GOAL-5.4 P0 ship gate). If the anonymizer ships and produces a slightly wrong corpus, GOAL-5.4's measurement is contaminated for the entire v0.3 release cycle (per §9.3 "frozen for v0.3"). A one-shot process needs to be nailed down **more** tightly than a recurring one, not less.

**Suggested fix:** Add §9.3.1 "Anonymization mechanism":
- Pick one: I recommend **regex + allow-list** (simpler, deterministic, auditable) over LLM-based. If LLM-based, specify model + temperature=0 + seed and treat any non-determinism as a correctness bug.
- State the acceptable leak tolerance: e.g., "zero PII leaks in potato's manual review → commit; any found leak → regenerate corpus, re-review."
- Specify failure handling: "anonymizer must process all 250 selected episodes or abort. Partial corpuses are not committed." This preserves the N=500 invariant.
- Add a test: "anonymizer is run twice on the same input, output hashes match" — already hinted at in §11 ("Anonymizer idempotence") but not cross-referenced here.

---

### FINDING-3 🟡 Important — §4.3 is missing a concrete ordering and dependency hint

**Location:** design.md §4.3 (implied — I couldn't find §4.3 explicitly in my read; see note below)

**Observation:** §4.1 defines P0 gates, §4.2 defines P1 gates, §4.4 defines failure semantics — but there's a missing §4.3 "Gate evaluation order". The text at §4.3 discusses "no short-circuit" (all gates produce results) but doesn't specify the **data dependency ordering**:

- `test-preservation` (§3.4) depends on migration having been applied.
- `migration-integrity` (§3.6) depends on migration tool being buildable.
- `cost` (§3.3) depends on `ResolutionStats` being exposed (per §12.6 ack from resolution).
- LOCOMO / LongMemEval don't depend on migration at all — they run on fresh in-memory DBs (§3.1 step 1).

For `release-gate` to run "all drivers in §4.3 order", that order must be written down. Currently it's implicit.

**Why this matters:** When implementation begins, the implementer has to invent an order. If they put `test-preservation` before ensuring the migration binary is built, the whole run fails confusingly. An explicit DAG (or topological order) prevents this class of "obvious in retrospect" bug.

**Suggested fix:** Add §4.3 subsection "Evaluation order" with:
- A dependency graph (ASCII DAG is fine):
  ```
  [build engram + engram-bench] → [LOCOMO, LongMemEval, cognitive-regression, cost]
                                → [build migration tool] → [test-preservation, migration-integrity]
  ```
- State: "`release-gate` runs independent gates in parallel where the harness supports it, sequential otherwise. Ordering within a stage is arbitrary; between stages follows the DAG."
- Link this back to §4.3's existing "no short-circuit" rule — all gates produce results, but upstream failures propagate as ERROR (not FAIL) to downstream gates that couldn't even start.

---

### FINDING-4 🟢 Minor — GOAL-5.7 query-equivalence rule has a subtle ambiguity

**Location:** design.md §3.6 "Definition of query equivalence"

**Observation:** The three equivalence rules read:
1. Result count within ±0 for exact-match, ±10% for similarity.
2. Every v0.2 top-3 result present in v0.3 top-10.
3. Type-changed results must have originating record(s) in topic's source list.

Rules 2 and 3 can silently conflict: if v0.2 returned record X in top-3, and v0.3 returned topic T in top-10 where T.source_list includes X, has rule 2 been satisfied? The record X itself is **not** in v0.3's top-10 (only the topic containing it is). Rule 3 says this is acceptable. Rule 2 reads literally as "top-3 result present in top-10" — which fails.

**Why this matters (minor):** An implementer could read rule 2 strictly and fail the gate on a perfectly-acceptable type substitution, OR read rule 3 permissively and miss a real recall regression.

**Suggested fix:** Clarify rule 2: "Every v0.2 top-3 result is either (a) present verbatim in v0.3's top-10, or (b) satisfies rule 3 via a topic in v0.3's top-10 whose source_list includes it."

---

### FINDING-5 🟢 Minor — Reviewer checklist §14 is orphaned after structural changes

**Location:** design.md §14 (end-of-doc reviewer checklist)

**Observation:** The 6-item checklist references §1.4 / §4 / §4.4 / §5 / §9.3 / §12.6. Since the r3 seam fixes added §10 Cross-Feature References to graph-layer (and this doc's §12), any future reviewer reading the checklist won't be prompted to verify §12 / §13 consistency — which are now load-bearing.

**Why this matters (minor):** Low priority; the checklist is for humans, easily updated.

**Suggested fix:** Add two items:
- `[ ] §12 Cross-Feature References — each sibling hand-off has ✅ acknowledged status with a pointer to the sibling's ack section.`
- `[ ] §13 Requirements Traceability Table — every GOAL + GUARD row points to a design section that exists and measures what the row claims.`

---

### FINDING-6 🟢 Minor — §8.2 release-gate duration (4–8h) has no budget breakdown

**Location:** design.md §8.2

**Observation:** "4–8 hours for a full release-gate run (LOCOMO + LongMemEval dominate)" — but there's no per-driver budget. If LOCOMO takes 7h by itself in one run, is that normal or a regression? No way to know.

**Why this matters (minor):** A per-driver p95 latency budget would catch silent perf regressions in the harness itself. Without it, "release-gate is slow today" is an opaque complaint.

**Suggested fix:** Add a table in §8.2:
- LOCOMO: ~3h (N ~2000 queries × ~5s/query incl. LLM)
- LongMemEval: ~3h
- Cost: ~30min (N=500 ingests, no query phase)
- Others: ~15min each
- Buffer: 30min

These are rough targets, committed as soft-budgets. The harness logs actual durations; CI alerts if any driver exceeds target by 50%.

---

## What did NOT surface as issues (spot-checks)

I specifically looked for these known-common design flaws and did **not** find them here:

- ✅ No prose-only acceptance criteria — every GOAL has a concrete metric + threshold.
- ✅ No simplification of the problem — §3.5 explicitly defends "directional, not quality" measurement with reasoning; §3.6 doesn't skip the hard topic-reconciliation case.
- ✅ No hidden runtime dependencies — GUARD-9 compliance is called out in §8.3 and §9.4.
- ✅ No silent-degrade paths — §4.4 formalizes GUARD-2 at three levels, including the override audit trail.
- ✅ Reproducibility is contractual (§6.1 schema), not convention — this is rare and good.
- ✅ Testing strategy for the harness itself (§11) exists — often missing.
- ✅ Ownership boundaries are drawn (§1.4 table) before any mechanism is described.
- ✅ All 8 GOALs have design-section coverage in §13 traceability table.
- ✅ Cross-feature asks (§12.6) all acknowledged in siblings (verified in seam-closure pass).

---

## Recommendation

Apply FINDING-1, 2, 3 before implementation begins. FINDING-4, 5, 6 are polish — can be applied opportunistically or deferred to r2.

**No 🔴 Critical issues. No scope changes needed. No requirements drift.**

Design is implementation-ready after the three Important fixes land.

---

## Applied Changes (2026-04-24, same session)

### FINDING-1 ✅ Applied
- Added §4.2a "Meta-gate: reproducibility record validity (GOAL-5.8)" with P1 level, schema-validation semantics, explicit override-field coupling.
- Updated §10.1 summary table example to show GOAL-5.8 line.
- Updated §13 traceability row for GOAL-5.8 to include §4.2a.

### FINDING-2 ✅ Applied
- Added §9.3.1 "Anonymization mechanism (one-shot precondition specification)".
- Pinned mechanism to deterministic regex + allow-list (not LLM).
- Specified transformer catalog layout (`patterns.toml` / `allowlist.toml` / `delta.toml`).
- Pinned NER model version (spaCy, frozen for v0.3).
- Stated zero-leak tolerance with explicit regen-on-leak procedure.
- Specified all-or-nothing failure handling (no partial corpuses, preserves N=500 invariant).
- Cross-linked to §11 idempotence test.

### FINDING-3 ✅ Applied
- Rewrote §4.3 "Gate evaluation order & dependency DAG".
- Added explicit DAG (stage 0 build → stage 1 independent drivers → stage 2 migration-dependent).
- Added upstream-failure propagation rule: downstream gates blocked by upstream → reported as ERROR (not FAIL, not skipped) with `blocked_by` field.
- Kept within-stage cost ordering as soft preference.
- Parallel execution guidance for stage-1 drivers.

### FINDING-4 ✅ Applied
- Rewrote §3.6 "Definition of query equivalence".
- Rule 2 and rule 3 now explicitly OR-combined: "either (a) present verbatim in top-10 OR (b) satisfies type-substitution via topic whose source_list includes it."
- Added explicit precedence statement.

### FINDING-5 ✅ Applied
- Added two checklist items to §14:
  - §12 Cross-Feature References acknowledgment verification.
  - §13 Requirements Traceability Table pointer-existence verification.

### FINDING-6 ✅ Applied
- Added per-driver soft budget table to §8.2.
- Targets: LOCOMO ~3h, LongMemEval ~3h, cost ~30min, test-preservation ~10min, cognitive-regression ~15min, migration-integrity ~15min, reproducibility-meta <1min, buffer ~30min.
- Stated these are soft budgets (CI observability), not ship gates.

### Summary
- Applied: 6/6
- Skipped: 0/6
- Design grew from 783 → 878 lines (+95 lines of concrete specification).
- All seams still intact (§12 cross-feature references unchanged).
- r2 review not immediately required — changes are additive refinements, no structural rework.
