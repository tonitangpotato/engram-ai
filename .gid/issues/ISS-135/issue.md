---
id: ISS-135
title: DeferToLlm reaches persist for backfill — implement Conservative/Abort fallback per design §8.1
status: in_progress
priority: P1
severity: blocker
created: 2026-05-22
relates_to: [ISS-044, ISS-132]
blocks: [ISS-044]
labels: [resolution, v03, design-drift, migrate]
---

# Problem

After ISS-132 fixes the embedding-dim mismatch,
`iss044_backfill::test_backfill_completes_against_populated_v02_db`
still fails with `records_failed_permanent: 1`. Diagnostic
(`crates/engramai-migrate/examples/iss132_dump_failures.rs`) shows:

```
graph_extraction_failures row:
  stage           = persist
  error_category  = unresolved_defer
  detail          = EntityResolution::DeferToLlm reached persist for draft
                    'engramai-rs' (index 3, candidate 4beec5f6-...).
                    LLM tie-break must commit a concrete Decision before §3.5.
```

The seeded fixture has three v0.2 memories. `m3` mentions
`engramai-rs`, which the regex extractor matches against the
`*-rs` pattern. Resolution finds a tier-B (ambiguous) similarity
to `m1`'s already-stored `gid-rs` entity → `Decision::DeferToLlm
{ candidate_id }`. The backfill driver never wires an LLM
tie-breaker → `DeferToLlm` arrives at `build_delta` →
`stage_persist.rs:310` (entity arm) emits a `StageFailureRow {
stage: "persist", category: "unresolved_defer" }` and **skips
the entry** (no mention, no entity row).

The same logic exists for edges at `stage_persist.rs:351`
(`EdgeDecision::DeferToLlm`).

# Why this is design drift, not a bug in extraction

`.gid/features/v03-resolution/design.md` §8.1 already specifies
the correct behaviour:

> Resolution (§3.4) may make LLM tie-breaker calls.
> `config.on_tiebreak_failure` controls behavior when the
> tiebreaker fails (timeout, error):
>
> - **`Conservative` (default)** — defaults the decision to
>   `CreateNew` for entities and `Add` for edges. Trace entry
>   flags `tiebreak_failed = true` with `method = Automatic`,
>   `confidence = low`. Preserves the successful classifier +
>   extractor work; accepts the risk of a duplicate entity that
>   the agent can merge later via `agent_curate_entity` (§6.3).
>   Satisfies GUARD-2 via the visible trace entry — this is not
>   silent degradation.

The code currently has **no** `on_tiebreak_failure` config
field, **no** `Conservative` / `Abort` enum, and unconditionally
takes the equivalent of the `Abort` branch (failure row + skip).

Compounding the drift, `stage_persist.rs:117` claims:

```rust
/// - `DeferToLlm { candidate_id }` → equal to `candidate_id` (placeholder;
///   `DeferToLlm` reaching persist is an error path, see ISS-072).
```

— but **ISS-072 is about entity-extractor degenerate defaults**,
not about `DeferToLlm` reaching persist. The comment is a
misleading rationale that justifies the fail-loud behaviour with
a hand-wave to an unrelated issue.

# Root cause

`§3.4 → §3.5` lacks the `on_tiebreak_failure` policy hook. The
backfill driver (`pipeline.rs:826 resolve_for_backfill`)
deliberately does not call LLM (design §6.5: "no LLM
tie-break"). Without the Conservative default, every tier-B
ambiguous draft on backfill becomes a permanent failure. For
real v0.2 → v0.3 migrations on populated DBs, this is a
high-frequency path: any entity surface form similar to one
already in the graph will fail.

# Proposed fix

Implement design §8.1 verbatim.

**Type changes (`crates/engramai/src/resolution/`):**

- New public enum
  `OnTiebreakFailure { Conservative, Abort }` with `Default =
  Conservative`.
- Add field `pub on_tiebreak_failure: OnTiebreakFailure` to
  `PipelineConfig` (pipeline.rs:195). Default = Conservative.

**Audit category (`crates/engramai/src/graph/audit.rs`):**

- New const `CATEGORY_TIEBREAK_FALLBACK: &str = "tiebreak_fallback"`.

**Persist stage (`crates/engramai/src/resolution/stage_persist.rs`):**

- `build_delta` gains a `tiebreak_policy: OnTiebreakFailure`
  parameter (added at end of signature to minimise test
  fixture churn — most call sites pass `Default::default()`).
- `Decision::DeferToLlm` arm:
  - `Conservative`: mint a fresh `Uuid::new_v4()`, emit entity
    upsert as `CreateNew`-equivalent (with
    `identity_confidence = low`), emit mention + alias rows,
    **and** emit one `StageFailureRow { stage: "persist",
    category: "tiebreak_fallback", detail: "..." }` per fallback
    so the trace is visible (GUARD-2).
  - `Abort`: preserve current `unresolved_defer` behaviour.
- `EdgeDecision::DeferToLlm` arm: mirror the entity arm.
  Conservative = emit `Add`-equivalent with low confidence and
  `tiebreak_fallback` audit row; Abort = current behaviour.

**Comment cleanup:**

- `stage_persist.rs:51-53` module doc — rewrite the "no LLM
  tie-break" paragraph to describe the new policy split.
- `stage_persist.rs:117` — replace the misleading "see ISS-072"
  reference with "see ISS-135 / design §8.1".

**Tests:**

- `iss044_backfill::test_backfill_completes_against_populated_v02_db`
  must pass with `records_failed: 0`.
- Add unit tests in `stage_persist_tests.rs`:
  - `defer_to_llm_conservative_mints_new_entity` —
    `OnTiebreakFailure::Conservative` produces an entity upsert
    and a `tiebreak_fallback` audit row, no `unresolved_defer`.
  - `defer_to_llm_abort_emits_failure_row` —
    `OnTiebreakFailure::Abort` preserves the legacy
    `unresolved_defer` row and skips the entry.
  - Same two for `EdgeDecision::DeferToLlm`.

# Acceptance criteria

- [ ] `OnTiebreakFailure` enum exists; `PipelineConfig` carries
      it with `Default = Conservative`.
- [ ] `build_delta` consumes the policy; both `Conservative` and
      `Abort` paths covered for entity and edge `DeferToLlm` arms.
- [ ] `CATEGORY_TIEBREAK_FALLBACK` is emitted on Conservative
      fallback (GUARD-2 visible trace).
- [ ] `iss044_backfill` all 3 tests pass.
- [ ] New unit tests in `stage_persist_tests.rs` cover both
      modes for both decision kinds.
- [ ] Misleading ISS-072 comment removed from
      `stage_persist.rs:117`.
- [ ] Full `engramai` + `engramai-migrate` test suites green; no
      new regressions.
- [ ] ISS-044 unblocked: move from in_review → done.

# Out of scope

- Implementing an actual LLM tie-breaker — `Conservative` is the
  default and is sufficient for backfill. LLM tie-break can be
  added later as a separate ticket; it would slot in between
  `§3.4` and `§3.5` (before persist), not change the persist
  policy split.
- Adding `tiebreak_failed: bool` and `method: ResolutionMethod`
  as **persistent** entity columns. The design text suggests
  these as trace fields, and the `graph_extraction_failures` row
  with `category=tiebreak_fallback` discharges the GUARD-2
  obligation without a schema migration. If retrieval needs
  per-entity provenance later, that's a separate ticket.

# References

- `.gid/features/v03-resolution/design.md` §8.1 — Conservative /
  Abort policy spec
- `.gid/features/v03-resolution/design.md` §6.5 — backfill does
  not call LLM
- `crates/engramai/src/resolution/stage_persist.rs:310,351` —
  current fail-loud arms
- `crates/engramai/src/resolution/pipeline.rs:826
  resolve_for_backfill` — backfill driver call site
- `crates/engramai-migrate/examples/iss132_dump_failures.rs` —
  reproducer that surfaces the `unresolved_defer` audit row
- ISS-132 — the dim-mismatch sibling regression (resolved
  separately, made ISS-135 visible by unblocking the
  apply path)
- ISS-044 — the original v0.2 → v0.3 backfill wiring task, in
  in_review pending this fix
