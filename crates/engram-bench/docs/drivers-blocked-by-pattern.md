# Driver Pattern: `blocked_by` over Silent Pass

> **Audience:** Anyone implementing a new `BenchDriver` (especially the
> remaining `cognitive-regression` and `migration-integrity` drivers).
>
> **Status:** Established by `test_preservation` driver (landed
> 2026-04-26 via `task:bench-impl-driver-test-preservation`). This doc
> codifies the pattern so the next two drivers reuse it instead of
> reinventing.

## TL;DR

When a driver depends on **upstream tooling that may not exist yet**
(e.g. the v0.3 migration CLI), the driver MUST surface the absence as
`blocked_by: <reason>` propagating to a gate `Error`, never silently
PASS. **Missing measurement = never PASS** is GOAL-5.5's guard
principle; this doc shows how to implement it consistently.

## Why this pattern matters

A naive driver has three states: PASS / FAIL / crash. A robust driver
has **four**:

| State | Meaning | Gate result |
|---|---|---|
| PASS | Measurement taken, threshold met | `Pass` |
| FAIL | Measurement taken, threshold violated | `Fail` |
| ERROR (blocked) | Measurement could NOT be taken — upstream blocker | `Error` (with `blocked_by`) |
| Crash | Driver bug | propagates `BenchError` |

The crucial distinction is **PASS vs ERROR**. A driver that returns
PASS when it didn't actually measure anything is worse than a crash —
it's a false positive that lets regressions ship.

Concrete failure mode this pattern prevents:

> v0.3 migration tool isn't built yet → `test_preservation` can't
> migrate fixtures → if we treated "no migration ran" as "no failures
> detected" → release-gate goes green on a build that hasn't actually
> been validated.

## The pattern (3 invariants)

### Invariant 1: Detect upstream tooling, return `Option<String>`

A driver that depends on external tooling exposes a detection helper:

```rust
/// Returns Some(blocked_by_message) if the upstream tool is absent.
/// Returns None if the tool is available (run continues normally).
pub(crate) fn apply_<upstream>_to_fixtures(...) -> Option<String> {
    let tool_available = /* probe PATH, cargo workspace, etc. */;
    if !tool_available {
        return Some("v03-<feature> tool not available (...)".into());
    }
    None
}
```

Reference impl: `test_preservation::apply_migration_to_fixtures()`
(checks `engram-cli` on PATH AND as a cargo workspace member, fails
gracefully if neither).

### Invariant 2: Plumb `blocked_by` through the summary

The driver's summary struct carries an `Option<String> blocked_by`
field, serialized with `skip_serializing_if = "Option::is_none"` so
downstream JSON tooling doesn't have to handle `null`:

```rust
#[derive(Serialize, Deserialize)]
pub struct DriverSummary {
    // ... metrics ...
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub blocked_by: Option<String>,
}
```

The summary is constructed exactly once at run end; `blocked_by` is
set if **any** stage of the run produced a blocker. Multiple blockers
are OK to merge into a single message — the gate just needs SOMETHING
non-None to short-circuit.

### Invariant 3: Translate `blocked_by` → `MetricValue::Missing` in the snapshot

When emitting the `MetricsSnapshot` consumed by gate evaluation:

```rust
if let Some(reason) = &summary.blocked_by {
    snapshot.set_missing("<gate_metric_key>", reason.clone());
} else {
    snapshot.set_number("<gate_metric_key>", measured_value);
}
```

`harness/gates.rs` already handles `MetricValue::Missing` correctly
(line 447): it routes to `GateStatus::Error` with the reason
preserved, so operators see exactly **why** the gate didn't evaluate.

This is the **only** way `blocked_by` is supposed to surface to the
gate — do not invent new gate result variants.

## Application blueprint for the 2 remaining drivers

### `cognitive_regression` (design §3.5, GOAL-5.6)

**What it measures:** Three-feature directional regression — checks
that the v0.3 cognitive properties (presumably ACT-R activation,
Hebbian linking, emotional weighting) move in the correct *direction*
relative to v0.2. Not absolute values; sign/ordering invariants.

**Likely upstream blockers:**
1. v0.3 migration tool — same as `test_preservation`. Need migrated
   v0.2 cognitive fixtures to compare against.
2. v0.3 graph layer — if the cognitive properties are now read from
   graph edges (Hebbian → edges) rather than tables, the graph schema
   must exist.

**Recommended structure:**
```rust
fn run_impl(config: &HarnessConfig) -> Result<RunReport, BenchError> {
    let baseline = load_v02_cognitive_baseline(...)?;          // hard error if missing
    let blocked_by = apply_migration_to_fixtures(&fixture_dir)
        .or_else(|| check_graph_layer_available(...));         // chain blockers
    let outcome = if blocked_by.is_none() {
        run_three_feature_probe(...)?
    } else {
        Outcome::blocked()                                     // no measurement
    };
    finalize_run(config, outcome, &baseline, blocked_by)
}
```

Note `apply_migration_to_fixtures` should be **shared** with
`test_preservation` — extract it to a `drivers/migration_probe.rs`
helper rather than copy-pasting.

### `migration_integrity` (design §3.6, GOAL-5.7)

**What it measures:** v0.2 → v0.3 migration data integrity. Memories
preserved, Hebbian links → graph edges correct, no orphans, no
duplicates.

**The recursive case:** This driver's *subject* is the migration
tool. So "migration tool absent" is itself a hard `Error`, not a
`blocked_by` — there's nothing else to measure.

**Recommended structure:**
```rust
fn run_impl(config: &HarnessConfig) -> Result<RunReport, BenchError> {
    let baseline = load_v02_db_snapshot(...)?;
    let migration_tool = locate_migration_tool(...)
        .ok_or_else(|| BenchError::MissingTooling(
            "v03-migration tool absent — cannot evaluate \
             migration_integrity (this driver's subject is the tool \
             itself)".into()
        ))?;
    // ... run migration, diff before/after, evaluate invariants ...
}
```

Distinction from the other two:
- `test_preservation` and `cognitive_regression` use `blocked_by` →
  gate `Error` (other measurements may still be possible).
- `migration_integrity` uses `BenchError` → driver crash (the tool is
  the SUT; absence is a hard stop, not a temporary block).

## Shared helper extraction (recommended refactor)

Once `cognitive_regression` lands, extract migration-tool detection:

```
crates/engram-bench/src/drivers/
├── mod.rs
├── shared/                          ← NEW
│   ├── mod.rs
│   └── migration_probe.rs           ← apply_migration_to_fixtures
├── test_preservation.rs             ← uses shared::migration_probe
├── cognitive_regression.rs          ← uses shared::migration_probe
└── migration_integrity.rs           ← does NOT use it (the tool IS the subject)
```

Do NOT extract this preemptively — wait until there are 2 callers.
Premature abstraction is a known trap (one driver inventing the trait
that the second driver can't fit).

## Testing the pattern

Each driver MUST have at least these unit tests:

1. **Tool present, all tests pass** → `MetricValue::Number(...)` →
   gate `Pass`.
2. **Tool present, tests fail** → `MetricValue::Number(...)` exceeds
   threshold → gate `Fail`.
3. **Tool absent** → `MetricValue::Missing(reason)` → gate `Error`
   with reason preserved.
4. **Baseline file missing** → `BenchError` (hard crash; baseline is
   non-negotiable).
5. **Exception list empty / present / corrupt** — exception parsing
   is a common silent-fail spot.

Reference: `test_preservation`'s 23 unit tests, especially the
6-branch pass-rate truth table (covers states 1-3 above).

## What this pattern is NOT

- **Not** a way to skip flaky tests. `blocked_by` is for **upstream
  tool absence** (deterministic), not flakiness. A flaky test should
  fail loudly and be added to the `exceptions.toml` with
  justification.
- **Not** a way to defer measurement. If a driver could measure but
  the operator forgot to pass a flag → that's a usage error
  (`BenchError::InvalidConfig`), not `blocked_by`.
- **Not** a substitute for proper dependency declaration. Drivers
  still need to declare their dependencies in the design doc and
  build plan; `blocked_by` is the runtime safety net, not the
  primary contract.

## References

- Driver trait: `crates/engram-bench/src/harness/mod.rs:215`
  (`BenchDriver`)
- Gate evaluation: `crates/engram-bench/src/harness/gates.rs:425`
  (`evaluate_gate`), line 447 handles `MetricValue::Missing`.
- Reference impl: `crates/engram-bench/src/drivers/test_preservation.rs`
  (especially `apply_migration_to_fixtures` at line 614 and the
  `blocked_by` plumbing at lines 669-704).
- Design source: `.gid-v03-context/v03-benchmarks-design.md` §3.4
  (test_preservation), §3.5 (cognitive_regression), §3.6
  (migration_integrity); §5 (release-gate semantics).
- Build plan: `.gid-v03-context/v03-benchmarks-build-plan.md` T3
  (driver task table) and T4 (dependency edges).
