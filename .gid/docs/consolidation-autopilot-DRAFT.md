# Consolidation Autopilot — Design Draft

**Status**: DRAFT — written 2026-05-06 by RustClaw while potato sleeps. Needs review before any implementation.
**Author**: RustClaw (autopilot session)
**Scope**: This is a master architecture doc, not a feature design. No new GOALs; reuses existing feature designs.
**Output location decision**: keep here (`.gid/docs/`) until potato approves; if approved, individual sub-feature design tweaks land in `.gid/features/<feature>/`.

---

## 1. Problem Statement

Engram has multiple post-write background processes already designed and partially implemented:

- `memory-lifecycle` (consolidation, decay, rebalance — partial; C8/C9 not implemented)
- `multi-signal-hebbian` (write-time link formation — implemented in `synthesis/cluster.rs`)
- `rumination` (online synthesis trigger — draft, `ruminate()` not exposed)
- `knowledge-compiler` (insight promotion — P0 done)
- `supersession` (correction-driven filter — data model done, auto-detect not done)

**Today they only run when something explicitly calls them** — heartbeat, manual `consolidate()`, manual `sleep_cycle()`. When engram is embedded in another agent (rustclaw, cogmembench), nothing schedules them. Result:

- Hebbian links don't form unless retrieval co-recall happens (which itself needs links to work — cold-start)
- Insights only generated on manual triggers
- No temporal decay → memories never lose strength → recency bias inverts
- LoCoMo benchmark substrate has 0 entries in `entities`, `entity_links`, `hebbian_associations` tables (verified RUN-0018 jsonl). Every retrieval is hybrid-only; the entire graph layer is dormant.

**The gap is not new functionality — it's a scheduler that runs the existing functionality on its own.**

## 2. Non-Goals

- Inventing new consolidation algorithms (use what exists in `synthesis/`, `compiler/`, `lifecycle/`)
- Distributed scheduling (single-process is fine — engram embeds in one agent at a time)
- User-facing controls beyond on/off and budget (autopilot's value is being invisible)
- LLM-driven meta-cognition loops (that's `meta-cognition` feature, separate)
- Touching retrieval-time code (autopilot is strictly post-write background)

## 2.5 Substrate Invariants (must hold on v0.4 unified substrate)

Updated 2026-05-14 (T04) to align with `.gid/features/v04-unified-substrate/`. Autopilot does **not** mutate storage directly — it only invokes existing consolidation operations (synthesis, KC, decay, lifecycle). Therefore the substrate guarantees of v0.4 apply transitively to autopilot. Specifically:

- **I-A1 — Dual-write transit.** Every mutator autopilot triggers (Hebbian co-activation, entity link, topic upsert, supersession, decay-soft-delete) goes through a writer path that already dual-writes to `nodes` / `edges` under v0.4 Phase B (ISS-119–126 closed 2026-05-14). Autopilot inherits this without special-case code; if a new consolidation primitive is added that does **not** dual-write, it must not be invoked from autopilot until its Phase B writer audit is closed.

- **I-A2 — No raw SQL writes.** Autopilot code in `engramai/src/autopilot/*` must not contain `INSERT`, `UPDATE`, or `DELETE` statements. All mutations go through `Storage` public methods, which in turn go through the writer queue (`v04-unified-substrate/design.md §6`). This invariant is grep-checkable.

- **I-A3 — Idempotent on the unified substrate.** Running an autopilot cycle twice on a DB at rest must produce zero net new `nodes` / `edges` rows (modulo wallclock timestamp updates on `updated_at` columns). This is the substrate-level expression of G3.

- **I-A4 — Namespace-respecting.** Every autopilot stage operates within a single namespace per invocation, except Hebbian cross-NS coactivation (which is the only legitimate cross-namespace edge writer, per `v04-unified-substrate/requirements.md` GOAL-1.5).

- **I-A5 — Read-switch transparent.** Autopilot reads via the same `Storage` public API as the rest of the system; whether the read path is under `unified_substrate=true` or `=false` (default through Phase D) is irrelevant to autopilot logic. Tests must pass under both flag positions.

- **I-A6 — Backfill compatible.** A Phase C backfill (T19–T26a) running concurrently with autopilot must not cause double-write conflicts on the unified side. The driver invariants (idempotent SQL-set drivers + the resumable T26a triple driver) guarantee `INSERT OR IGNORE` semantics; autopilot's writers via `Storage::add` etc. interleave safely.

- **I-A7 — Counter accounting on background mutations.** Autopilot-triggered Hebbian writes count against the per-driver audit row when invoked during a backfill window (i.e. they show up in `backfill_runs.notes` if instrumented). For pure-online cycles outside any backfill, no `backfill_runs` row is emitted — that table is migration-bookkeeping only.

- **I-A8 — Writer-queue priority lanes.** Autopilot mutations enqueue at `LOW` priority per `v04-unified-substrate/design.md §6.3`. User-facing writes (`Memory::add`) at `NORMAL` or `HIGH` always preempt; this is what makes autopilot "invisible" (G1) at write-load steady-state.

If any future autopilot stage violates these eight invariants, it should be specified in a separate design and not lumped under this doc.

## 3. Goals

- **G1**: Engram-embedded agents get hebbian/entity/insight enrichment **without explicit scheduling code in the host**.
- **G2**: Bounded resource cost — autopilot must cap CPU/wallclock per cycle, never block writes.
- **G3**: Idempotent — running autopilot twice on the same DB yields the same end state (modulo wallclock timestamps).
- **G4**: Observable — every cycle emits a structured report (counts, durations, gates fired).
- **G5**: Disable-able — env var or config flag turns it off entirely (benchmark reproducibility).

## 4. Architecture

### 4.1 Five Stages Per Cycle

A single autopilot cycle runs these stages in order. Each stage is an existing feature; the autopilot is the **scheduler + budget enforcer**.

```
                  ┌──────────────────────────────────┐
                  │  Cycle trigger (timer / N writes)│
                  └────────────────┬─────────────────┘
                                   ▼
             ┌─────────────────────────────────────────┐
             │ Stage 1: Hebbian / entity backfill      │
             │  - existing: synthesis/cluster.rs       │
             │  - new: run on writes since last cycle  │
             └─────────────────┬───────────────────────┘
                               ▼
             ┌─────────────────────────────────────────┐
             │ Stage 2: Synthesis (rumination)         │
             │  - existing: ruminate() (draft)         │
             │  - new: expose + schedule               │
             └─────────────────┬───────────────────────┘
                               ▼
             ┌─────────────────────────────────────────┐
             │ Stage 3: Supersession sweep             │
             │  - existing: supersede() data model     │
             │  - new: detect contradictions on dedup  │
             └─────────────────┬───────────────────────┘
                               ▼
             ┌─────────────────────────────────────────┐
             │ Stage 4: Decay tick                     │
             │  - new: implements memory-lifecycle C8  │
             │  - reads occurred_at (ISS-103)          │
             └─────────────────┬───────────────────────┘
                               ▼
             ┌─────────────────────────────────────────┐
             │ Stage 5: Rebalance / promotion          │
             │  - existing: KC promotion path          │
             │  - new: tier migration on score         │
             └─────────────────────────────────────────┘
```

### 4.2 Trigger Model

Two triggers, OR'd:

- **Time-based**: every N seconds (default 600 = 10 min) since last cycle
- **Write-based**: every M writes (default 100) since last cycle

Both are configurable via `ConsolidationConfig`. First trigger fires the cycle; counters reset.

This avoids both extremes:
- Pure time-based: idle agent wastes cycles; busy agent under-consolidates
- Pure write-based: idle agent never decays old memories

### 4.3 Budget Model

Each cycle has a hard **wallclock budget** (default 30s) and **per-stage budget** (default 6s × 5 stages). Stages that exhaust their budget log a `BudgetCutoff` event and yield to the next stage. **Cycles never run partial-then-blocking-write**: a write request mid-cycle pauses autopilot, write completes, autopilot resumes.

Implementation: a `CycleController` with `Arc<AtomicBool>` write-pending flag. Every loop iteration in stages checks the flag and yields if set.

### 4.4 Cooldown / Backoff

If a cycle does no useful work (0 hebbian links formed, 0 insights generated, 0 decay updates), the next cycle interval doubles, capped at 1 hour. Useful work resets to default interval. Prevents idle agents from polling.

## 5. New Code Surface

Three new modules. Everything else reuses existing code.

### 5.1 `engramai/src/autopilot/mod.rs`

```rust
pub struct Autopilot {
    config: ConsolidationConfig,
    stats: Arc<Mutex<AutopilotStats>>,
    // Handle to the storage; cycles take a brief lock per stage.
    storage: Arc<Mutex<Storage>>,
}

impl Autopilot {
    /// Spawn a background tokio task that runs cycles per the trigger
    /// model. Returns a handle that can be `.shutdown()` cleanly.
    pub fn spawn(storage: Arc<Mutex<Storage>>, config: ConsolidationConfig) -> AutopilotHandle;

    /// Run exactly one cycle synchronously. Used by tests + benchmark
    /// runs that want deterministic consolidation.
    pub fn run_one_cycle(&self) -> CycleReport;
}
```

### 5.2 `engramai/src/autopilot/config.rs`

```rust
pub struct ConsolidationConfig {
    pub enabled: bool,                          // env: ENGRAM_CONSOLIDATE
    pub cycle_interval: Duration,               // default 600s
    pub cycle_writes: u32,                      // default 100
    pub cycle_budget: Duration,                 // default 30s
    pub stage_budget: Duration,                 // default 6s
    pub stages: StageEnableFlags,               // each stage on/off independently
    pub cooldown_max: Duration,                 // default 1h
}

impl ConsolidationConfig {
    pub fn from_env() -> Self;                  // reads all ENGRAM_* vars
    pub fn disabled() -> Self;                  // for benchmarks / tests
}
```

### 5.3 `engramai/src/autopilot/cycle.rs`

The actual cycle runner — five stage functions that each call into existing modules:

```rust
fn stage1_hebbian(storage: &Mutex<Storage>, since: DateTime<Utc>) -> StageReport {
    // Calls synthesis::cluster::form_links_for_writes_since(...)
    // — already exists, just need a "since" overload
}

fn stage2_rumination(...) -> StageReport {
    // Calls ruminate() (currently draft, needs to land first)
}

fn stage3_supersession(...) -> StageReport {
    // Calls a new helper detect_contradictions() in supersession/
    // — uses existing entity_links + dedup signals
}

fn stage4_decay(...) -> StageReport {
    // Implements memory-lifecycle C8. Reads occurred_at, applies
    // ebbinghaus curve, writes back importance. Pure batch update.
}

fn stage5_rebalance(...) -> StageReport {
    // Calls compiler::promotion::run_pass() — already exists.
}
```

## 6. Dependencies — What Has To Land First

Ordered by graph depth:

1. **`rumination` feature**: must expose `Memory::ruminate() -> SynthesisReport`. Currently draft. ETA: small (the function exists internally, just needs a public wrapper).
2. **`memory-lifecycle` C8 (temporal decay)**: not implemented. Needs design. Smallest scope: ebbinghaus curve over `occurred_at` → updates `importance`. ETA: medium.
3. **Supersession auto-detect**: a small helper that inspects entity overlap + content negation patterns to flag candidate supersessions. Does NOT auto-apply — flags only, requires confidence threshold. ETA: medium.
4. **The autopilot module itself**: thin scheduler. ETA: small once 1-3 land.

## 7. Risks & Open Questions

### 7.1 Lock contention

Engram today is single-writer behind one `Mutex<Storage>`. Autopilot stages each take the lock briefly. **Open question**: can a 30s cycle starve a high-write workload?

**Mitigation**: each stage processes in batches of N (default 50) memories with lock-release between batches. Worst-case write latency = 1 batch (~100ms). Needs measurement.

### 7.2 Ordering vs determinism

Stage order matters: hebbian must run before rumination (ruminate uses links), supersession must run before decay (don't decay things about to be removed). If a future stage is added, the order needs to be re-justified.

**Mitigation**: stages are explicit Rust functions in `cycle.rs`, not a registry. Adding a stage is a code change with review.

### 7.3 Benchmark reproducibility

LoCoMo runs in `engram-bench` use `fresh_in_memory_db()` and replay a fixed-order conversation. Autopilot must be **off by default in test/bench code paths** to avoid non-determinism (a cycle firing mid-replay would change retrieval state).

**Mitigation**: `ConsolidationConfig::disabled()` is the default for `MemoryBuilder::for_test()`. Bench driver explicitly passes `disabled()`. Production callers (rustclaw, host agents) call `from_env()` which defaults to enabled but checks `ENGRAM_CONSOLIDATE=0`.

### 7.4 What does "useful work" mean for cooldown

§4.4 says "0 hebbian links formed, 0 insights generated, 0 decay updates" → backoff. But:
- A cycle that decays but doesn't form links — useful?
- A cycle that runs supersession sweep with 0 candidates — useful (it verified there's nothing to do) or wasteful (no state change)?

**Decision needed**: probably "any state-mutating output" counts as useful. Open for review.

### 7.5 Interaction with rustclaw consolidate hook

rustclaw heartbeat already calls `consolidate()` periodically. If autopilot also runs, we get **double consolidation**.

**Mitigation**: rustclaw should detect autopilot is on (via `Memory::autopilot_active()`) and skip its own consolidate hook. Or autopilot config is the single source of truth and rustclaw stops calling consolidate directly.

**Open**: which side owns the decision. Probably engram (autopilot config) is authoritative.

## 8. Acceptance Criteria

This design is "good enough to implement" when:

- [ ] All 5 stage functions can be expressed as 1-line calls into existing modules (or have explicit ETAs for the missing dependencies)
- [ ] Bench reproducibility plan reviewed (no non-determinism in `engram-bench` runs)
- [ ] Lock contention mitigation has a concrete batch-size number (measured, not guessed)
- [ ] potato approves the trigger model (time + writes) vs alternatives (purely write-based, purely time-based, adaptive)
- [ ] ISS-NNN filed for each missing dependency with explicit blocks/depends_on edges to autopilot
- [ ] §2.5 substrate invariants I-A1 through I-A8 verified — each stage mapped to the v0.4 writer path it transits, no raw SQL in `autopilot/*`, namespace-scoping honored, writer-queue priority `LOW` used for all enqueues. (Added 2026-05-14 by T04 of `v04-unified-substrate`.)

## 9. What This Does NOT Specify (Deliberately)

- Exact ebbinghaus parameters for decay (that's `memory-lifecycle` C8 design's job)
- Hebbian link formation thresholds (already in `synthesis/cluster.rs`)
- Insight gate logic (already in `compiler/`)
- Supersession contradiction detection algorithm (separate feature design)

The autopilot is a **conductor**, not a soloist. It schedules existing instruments.

---

## Status & Next Steps

**Tonight (autopilot session, no human review):**
- Wrote this draft. Did NOT touch any source code beyond the K_seed orchestrator fix (separate work).
- Did NOT create new feature directories.
- Did NOT file new ISS issues.

**For potato to do on wakeup:**
- Read this doc, decide: ship as-is for review-design skill / split into per-stage designs / kill it entirely.
- If approved → file ISS for each §6 dependency, build dependency graph, then iterate on §5 module design.
- If killed → delete this file, no harm done, K_seed work stands alone.

**Hard NOT-DO list for tonight:**
- Don't write `engramai/src/autopilot/` skeleton (waiting for review).
- Don't add autopilot graph nodes (review may reshape the architecture).
- Don't touch `rumination`, `memory-lifecycle`, or `supersession` source — those are separate features with their own review cycles.
