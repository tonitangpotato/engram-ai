---
issue: ISS-062
kind: design
status: draft
author: rustclaw-autopilot
date: 2026-04-29
---

# Design: Strengthen retrieval observability — distinguish stub / downgrade / empty outcomes

> Implements ISS-062. Non-breaking, additive. Touches 3 files in `crates/engramai/src/retrieval/`.

## 1. Problem statement

Logs currently print `outcome=ok candidates=0` for three semantically distinct situations:

1. **Stub** — a plan was constructed with a `Null*` collaborator (e.g. `NullTopicSearcher`, `NullEntityResolver`). The plan ran successfully against a no-op dependency and produced zero results by design. Nothing is wrong; the feature is not wired up.
2. **Downgrade** — a higher-tier plan was unavailable or starved (`L5NotReady`, no seeds, cutoff missed) and the orchestrator silently degraded to a lower tier or to empty output. Behaviour is correct under the spec, but the result quality is *not* what the caller asked for.
3. **Empty** — every collaborator was real, executed, and the corpus genuinely contained no matching memory. This is the only "honest empty" case; it is the only one that should drive `precision/recall=0` interpretation in the eval harness.

ISS-060 / ISS-061 root-cause investigations were slowed by this conflation. The eval harness in particular grep-matches `outcome=ok candidates=0` and cannot today distinguish "stubbed plan" from "real empty corpus."

## 2. Non-goals

- No behavioural change to retrieval. Result sets, scores, ranking — all unchanged.
- No new metrics infrastructure (Prometheus, OpenTelemetry, etc.). Pure `log::info!` improvements.
- No public-API changes outside `RetrievalOutcome` (which is `#[non_exhaustive]`, so additive variants are non-breaking by construction).
- No change to `hybrid_sub_plan_outcome` lines added by ISS-063 — those already carry the right shape.

## 3. The three outcome categories — formal definitions

| Category | Trigger | Slug emitted | `RetrievalOutcome` variant |
|---|---|---|---|
| **Stub** | Plan was constructed with a `Null*` collaborator. The collaborator's no-op `impl` returns empty without consulting the corpus. | `stub_<plan>` (e.g. `stub_factual`, `stub_abstract`) | `RetrievalOutcome::StubPlan { plan_kind: PlanKind }` *(new variant)* |
| **Downgrade** | Plan executed but produced empty results because of a known degradation path: `L5NotReady`, episodic cutoff missed, no seeds, abstract→episodic fall-through, etc. | `downgraded_<reason>` (existing slugs from `DowngradedFromAbstract`, `DowngradedFromEpisodic`, `L5NotReady`, `Cutoff`) | Existing `Downgraded*`, `L5NotReady`, `Cutoff` variants — slugs already correct |
| **Empty** | All collaborators real, all queries executed, corpus genuinely returned nothing. | `empty_<reason>` (e.g. `empty_no_match`, `empty_graph`) | `RetrievalOutcome::EmptyResultSet { reason }` — already exists; we tighten which paths use it |

### 3.1 Stub-detection rule

A plan is "stubbed" iff at least one of its collaborators is a `Null*` type. Because each plan generic-defaults to `Null*` (e.g. `AbstractPlan<S = NullTopicSearcher>`), the type system already knows. We expose this as a `const STUBBED: bool` on each collaborator trait and let the plan emit `StubPlan` from its outcome construction site.

```rust
// In each collaborator trait module (factual.rs, abstract_l5.rs, …):
pub trait TopicSearcher {
    /// True iff this implementation is a no-op (`Null*`). Used by
    /// observability to emit `outcome=stub_*` instead of conflating
    /// stubbed plans with empty-corpus results. ISS-062.
    const IS_STUB: bool = false;
    fn search(&self, …) -> …;
}

impl TopicSearcher for NullTopicSearcher {
    const IS_STUB: bool = true;
    fn search(&self, …) -> … { vec![] }
}
```

The plan's `execute(…)` checks `S::IS_STUB` once; if true, short-circuits to `Outcome::Stub`. Mapping to `RetrievalOutcome::StubPlan { plan_kind }` happens in `orchestrator.rs::execute_plan`.

### 3.2 Downgrade vs. Empty disambiguation

Today `EpisodicPlan` may return `Outcome::Empty { reason: "cutoff_missed" }` *and also* `Outcome::Empty { reason: "no_match" }`. The orchestrator currently maps both to a generic empty slug. The fix:

- **Cutoff / NotReady / explicit downgrade reasons** → `RetrievalOutcome::Cutoff` / `L5NotReady` / `DowngradedFrom*` (existing slugs `cutoff`, `l5_not_ready`, `downgraded_from_abstract`, …).
- **Real empty** → `RetrievalOutcome::EmptyResultSet { reason }`, slug `empty_<reason>`. The `reason` string is bounded to a closed enum (see §6) — no free-form strings — so the eval harness can match exhaustively.

## 4. Logging changes — call site by call site

Two log lines change. One is added. Nothing is removed.

### 4.1 `orchestrator.rs:1079` — `execute_plan EXIT` (modified)

Today:
```
execute_plan EXIT plan_kind=Factual candidates=0 outcome=ok
```

After:
```
execute_plan EXIT plan_kind=Factual candidates=0 outcome=stub_factual category=stub
```

Changes:
- `outcome=` keeps the slug, but the slug is more specific (`stub_factual`, `empty_no_match`, `downgraded_from_episodic`).
- New `category=` field with three values: `ok` (candidates>0), `stub`, `downgrade`, `empty`. **Closed enum.** Eval harness greps on `category=` for the three-way split; old greps on `outcome=ok` keep working because `ok` is still emitted when candidates>0.

### 4.2 `orchestrator.rs:1204` — `fallback EXIT` (modified, same shape)

Same field additions as §4.1.

### 4.3 No change to `hybrid_sub_plan_outcome` lines (orchestrator.rs:603, 623, 658, 693)

ISS-063 already emits `hybrid_sub_plan_outcome sub_kind=Factual outcome=… items=…`. These lines piggy-back on the new slug format automatically because they format the same `RetrievalOutcome::slug()`. No code change in those four call sites; only the slug content gets richer.

### 4.4 Per-plan code change

Each plan file (`factual.rs`, `episodic.rs`, `abstract_l5.rs`, `affective.rs`, `bitemporal.rs`, `associative.rs`, `hybrid.rs`) gains:

```rust
// At the top of execute(…), after collaborator wiring:
if S::IS_STUB {
    return PlanResult {
        outcome: PlanOutcome::Stub,   // new typed variant
        items: vec![],
    };
}
```

`PlanOutcome::Stub` is a new variant on each plan's typed outcome enum (`FactualOutcome`, `EpisodicOutcome`, …). Each is `#[non_exhaustive]` already, so additive.

`hybrid.rs` is special — it has multiple sub-plan collaborators. It emits `Outcome::Stub` only when *every* configured sub-plan is stubbed; otherwise per-sub-plan stubs are visible in the existing `hybrid_sub_plan_outcome` lines.

## 5. New `RetrievalOutcome` variant

```rust
// retrieval/outcomes.rs

#[non_exhaustive]
pub enum RetrievalOutcome {
    Ok,
    // … existing variants …
    EmptyResultSet { reason: EmptyReason },   // §6 — change `reason: String` → typed
    StubPlan { plan_kind: PlanKind },         // NEW (ISS-062)
}

impl RetrievalOutcome {
    pub fn slug(&self) -> Cow<'static, str> {
        match self {
            Self::Ok => "ok".into(),
            Self::StubPlan { plan_kind } => format!("stub_{}", plan_kind.as_snake()).into(),
            Self::EmptyResultSet { reason } => format!("empty_{}", reason.as_snake()).into(),
            // …
        }
    }

    pub fn category(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::StubPlan { .. } => "stub",
            Self::Cutoff | Self::L5NotReady | Self::DowngradedFromAbstract
                | Self::DowngradedFromEpisodic => "downgrade",
            Self::EmptyResultSet { .. } | Self::NoEntityFound
                | Self::EntityFoundNoEdges | Self::NoMemoriesInWindow => "empty",
            _ => "ok",   // safe default; `#[non_exhaustive]` requires a wildcard
        }
    }
}
```

## 6. `EmptyReason` — closed enum (replaces `reason: String`)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmptyReason {
    NoMatch,        // queried, found nothing
    EmptyGraph,     // graph has zero nodes/edges of the relevant kind
    NoSeeds,        // associative had no seed candidates
    NoTopicMatch,   // abstract: topic search returned nothing
    NoEntity,       // factual: entity resolution failed
}

impl EmptyReason {
    pub fn as_snake(&self) -> &'static str {
        match self {
            Self::NoMatch => "no_match",
            Self::EmptyGraph => "empty_graph",
            Self::NoSeeds => "no_seeds",
            Self::NoTopicMatch => "no_topic_match",
            Self::NoEntity => "no_entity",
        }
    }
}
```

The existing `reason: String` field on `EmptyResultSet` is replaced by `reason: EmptyReason`. Migration: every existing call site in `orchestrator.rs` and `api.rs` that constructs `EmptyResultSet { reason: "literal" }` is mechanically updated to the matching enum variant. **This is technically breaking for downstream consumers that pattern-match on the field type** — see §8.

## 7. Files modified

| File | Change | Scale |
|---|---|---|
| `crates/engramai/src/retrieval/outcomes.rs` | Add `StubPlan`, `EmptyReason`, refactor `slug()`, add `category()` | ~60 LOC |
| `crates/engramai/src/retrieval/orchestrator.rs` | Update `execute_plan` + `fallback` log lines (`category=`); update `EmptyResultSet` constructions to use `EmptyReason` | ~25 LOC across ~12 sites |
| `crates/engramai/src/retrieval/plans/{factual,episodic,abstract_l5,affective,bitemporal,associative,hybrid}.rs` | Add `IS_STUB` const to collaborator trait + `Null*` impls; short-circuit `execute()` when stubbed; add `Stub` to per-plan outcome enum | ~10 LOC per file × 7 files ≈ 70 LOC |

**Total: 3 logical change sites, 9 files touched.** This exceeds A5.4's 5-file blocker threshold (see §8).

## 8. Breaking-change analysis

### 8.1 `RetrievalOutcome`
`#[non_exhaustive]` — adding `StubPlan` variant is non-breaking by Rust's policy.

### 8.2 `EmptyResultSet { reason: String → EmptyReason }`
**This IS a breaking change** for any downstream pattern-match on the field. Audit:

```bash
grep -rn 'EmptyResultSet { reason' crates/ | grep -v retrieval/outcomes.rs
```

Expected sites (must verify in implementation): `orchestrator.rs` constructions (~12 sites — internal, fine to update), `retrieval/api.rs` doc-test snippets, and possibly the eval harness (`crates/cogmembench/`).

If any external consumer (e.g. cogmembench's rust harness) pattern-matches `reason: "literal"`, the migration is mechanical but visible. **Mitigation:** keep `reason: String` for one release, add `reason_kind: EmptyReason` next to it, deprecate `reason: String` in v0.4.0, remove in v0.5.0. **Recommended for this design.**

Updated §6 design (chosen): keep `reason: String` for back-compat, **add** `reason_kind: EmptyReason` field. New log slug uses `reason_kind`; legacy `reason` stays as human-readable detail. Non-breaking.

### 8.3 Per-plan `Outcome` enums
All `#[non_exhaustive]` — adding `Stub` variant is non-breaking.

### 8.4 `IS_STUB` const on collaborator traits
Default-implemented (`const IS_STUB: bool = false;`), so existing implementors compile unchanged.

### 8.5 New `category=` log field
Additive. Existing log parsers ignore unknown fields. Eval harness explicitly opts in by greppping `category=`.

**Net:** with the §8.2 mitigation, the entire change is non-breaking.

## 9. Tests

One test per plan asserting the `outcome=` slug for each of:
1. Default-constructed plan (uses `Null*` collaborator) → asserts `outcome.category() == "stub"` and `slug().starts_with("stub_")`.
2. Real collaborator with empty corpus → asserts `outcome.category() == "empty"` and `slug().starts_with("empty_")`.
3. Real collaborator with downgrade trigger (cutoff missed / L5 not ready) → asserts `outcome.category() == "downgrade"` and slug matches the specific downgrade.

Plus one orchestrator-level test asserting the `execute_plan EXIT` log line includes `category=stub|downgrade|empty|ok` (capture via `tracing-test` or `log::set_logger` test harness).

**Test count:** ~21 unit tests (3 × 7 plans) + 1 orchestrator log-shape test = 22 new tests. A5.8 requires "at least one new test covering all three outcomes" — the orchestrator-level test alone satisfies it; the per-plan tests are bonus.

## 10. Eval harness alignment

The cogmembench eval harness today greps `outcome=ok candidates=0` to flag suspect runs. After this design lands:

- `category=ok` + `candidates=0` → impossible (would be a logic bug — outcome must be empty/stub/downgrade if candidates=0).
- `category=stub` → harness should emit a structured warning ("test ran against stubbed plan, results invalid").
- `category=downgrade` → harness records as "expected degradation," does not penalise.
- `category=empty` → harness records as honest miss, contributes to recall=0.

Eval-harness update is **out of scope for this issue** (separate issue / follow-up); the new fields make it possible.

## 11. Migration plan

1. Land §5 (`RetrievalOutcome::StubPlan` + `category()`) — additive, no other site changes.
2. Land §6 (`EmptyReason` enum + `reason_kind` field) — additive per §8.2 mitigation.
3. Update `orchestrator.rs` log lines (§4.1, §4.2) to emit `category=`.
4. Land §3.1 (`IS_STUB` const) one collaborator trait at a time — each trait + its `Null*` impl + the plan's `execute()` short-circuit, with the test from §9. Seven small PRs (or one PR with seven commits).
5. Update cogmembench harness in a follow-up issue.

Steps 1-3 are landable independently of step 4 — each step is a bisectable, test-passing point.

## 12. Open questions

1. **Should `category=` be a single field or split into `is_stub=`, `is_downgrade=` booleans?** Recommendation: single `category=` enum field. Easier to grep, fewer log fields, exhaustive by construction.
2. **`PlanKind` enum** — does it already exist? If not, we add it in `outcomes.rs` (variants: `Factual`, `Episodic`, `Abstract`, `Affective`, `Bitemporal`, `Associative`, `Hybrid`). Action item for implementation.
3. **`hybrid.rs::Outcome::Stub` semantics when only *some* sub-plans are stubbed:** treat as `Ok` at top level (the non-stubbed sub-plans ran), rely on per-sub-plan `hybrid_sub_plan_outcome` lines for visibility. Confirmed in §4.4.

## 13. A5.4 blocker check

Threshold: "if design indicates > 5 file changes / new log schema → BLOCKER, skip."

- **Files changed:** 9 (`outcomes.rs`, `orchestrator.rs`, 7 plan files). **Exceeds 5.**
- **New log schema:** yes — new `category=` field added to two log lines.

**Verdict per the rule: BLOCKER.** Continue to A5.4 for explicit decision.

The 9 files come from the per-plan `IS_STUB` propagation (§3.1, step 4 in §11). If we drop step 4 (skip stub-detection at the trait level and detect stubbing only at the orchestrator construction site by pattern-matching the concrete generic type — which is awkward and incomplete), the change shrinks to **3 files** (outcomes.rs, orchestrator.rs, hybrid.rs). That alternative loses ~half the benefit (stubbed factual/episodic/abstract/affective via `Null*` collaborators won't be detected) but stays under the 5-file threshold.

**Recommendation:** flag as BLOCKER per A5.4 and let potato decide between (a) full design (9 files, full benefit) and (b) partial design (3 files, only orchestrator-level stub detection). I lean (a) because the per-plan tests in §9 are cheap and the `IS_STUB` pattern is the right root fix; the 9-file count is mechanical, not architectural.
