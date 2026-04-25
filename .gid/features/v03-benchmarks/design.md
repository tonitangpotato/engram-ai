# Design: Engram v0.3 — Benchmarks & Ship Gates

> **Feature:** v03-benchmarks
> **Requirements:** `.gid/features/v03-benchmarks/requirements.md` (GOAL-5.1 … GOAL-5.8)
> **Master design:** `docs/DESIGN-v0.3.md` §11 (success criteria), §1/G3 (cost measurement), §7.3 (backward compatibility), §8.3 (fusion-weight tuning)
> **Sibling designs:** `v03-resolution/design.md`, `v03-retrieval/design.md`, `v03-graph-layer/design.md`, `v03-migration/design.md`

This design specifies the measurement harness and numeric ship gates for the v0.3 release. It does **not** specify the pipeline mechanics being measured — those belong to the sibling v0.3 features.

---

## 1. Scope & Non-Scope

### 1.1 In scope

- The measurement harness (drivers, fixtures, scoring, reproducibility records).
- Numeric pass/fail thresholds for every P0 ship gate and P1 quality gate in `requirements.md`.
- The CLI and Rust API used to run benchmarks locally, in CI, and during release qualification.
- Gate evaluation semantics: which failures block release, which are justified-failure acceptable, and how a failure is surfaced (GUARD-2).
- Baseline capture workflow (what numbers must be committed before v0.3 development begins).

### 1.2 Out of scope

- **Pipeline correctness at the per-stage level.** Whether resolution produces the right entity, whether retrieval fuses scores correctly, whether migration preserves a given row — those are owned by the sibling features. This feature only consumes their public APIs and measures aggregate outcomes.
- **Fusion weight tuning methodology.** `DESIGN-v0.3.md §8.3` defines the tuning process. This feature measures outcomes under frozen weights only.
- **Continuous production observability.** GUARD-12 covers rolling-window counters in deployed systems; this feature covers pre-release gates. The two share one counter source (GOAL-2.11) but have different surfaces.
- **Fuzz / stress / property testing beyond LOCOMO and LongMemEval.** Useful, not a v0.3.0 release gate.
- **Multi-version regression chains.** Only v0.2 vs v0.3 comparison is required; v0.1 history is not rebenchmarked.
- **Live dashboards / telemetry UIs.** Output is text (stdout, JSON, committed records). Visualization is future work.

### 1.3 Non-goals that look like goals

Two temptations to explicitly reject:

- **"Make the benchmark harness production-ready observability."** The harness is a gate-runner, not a monitoring system. Reusing the counter source is fine; reusing the harness for prod telemetry is not.
- **"Score every cognitive feature individually."** GOAL-5.6 specifies a *directional* regression (does the feature still affect ranking?), not a quality score per feature. Quality scoring per cognitive feature is an open research problem and not a release gate.

### 1.4 Ownership boundary with siblings

| Concern                              | Owner                        | This feature's role               |
| ------------------------------------ | ---------------------------- | --------------------------------- |
| Per-episode LLM call counters        | v03-resolution (GOAL-2.11)   | Consumer (aggregates over N=500)  |
| Query API driving LOCOMO/LongMemEval | v03-retrieval                | Consumer (drives via public API)  |
| Migration tool                       | v03-migration                | Consumer (invokes, checks output) |
| Graph store used during benchmarks   | v03-graph-layer              | Consumer (read-only observation)  |
| Gate thresholds & decision rules     | v03-benchmarks (this)        | Owner                             |
| Reproducibility record schema        | v03-benchmarks (this)        | Owner                             |
| Baseline capture workflow            | v03-benchmarks (this)        | Owner                             |

If any sibling changes a public API that a driver depends on, this feature's driver code updates; the gate thresholds do not.

---

## 2. Requirements Coverage

Every GOAL in `requirements.md` has a section below that defines its measurement:

- **GOAL-5.1** (LOCOMO overall ≥ 68.5%) → §3.1, §4.1
- **GOAL-5.2** (LOCOMO temporal ≥ Graphiti) → §3.1, §4.1
- **GOAL-5.3** (LongMemEval ≥ v0.2 + 15pp) → §3.2, §4.1, §5.1
- **GOAL-5.4** (≤ 3 LLM calls / episode over N=500) → §3.3, §4.1
- **GOAL-5.5** (100% v0.2 tests pass post-migration) → §3.4, §4.1, §5.2
- **GOAL-5.6** (cognitive-feature directional regression) → §3.5, §4.2
- **GOAL-5.7** (migration on rustclaw DB, ≥20-query equivalence) → §3.6, §4.2
- **GOAL-5.8** (reproducibility record per run) → §6, §7.3

GUARDs relevant to this feature (defined in master requirements):

- **GUARD-2** (never silent degrade) → §4.4 (failure semantics)
- **GUARD-9** (no new runtime dep) → §9.4 (fixtures as test-only artifacts)
- **GUARD-12** (LLM call budget observability) → §3.3 (shares counter source with prod observability)

The full traceability table is in §13.

---

## 3. Benchmark Suites & Drivers

A **driver** is a Rust binary (under `crates/engram-bench/`) that: loads fixtures, builds an engramai `Memory` instance, drives it through the suite, collects scores, and emits a reproducibility record. Drivers share a common harness (§7.2); they differ in dataset loader, scoring rule, and output schema.

### 3.1 LOCOMO driver

**Binary:** `engram-bench locomo`

**Input:**
- LOCOMO dataset at a pinned commit SHA (`fixtures/locomo/<sha>/`).
- v0.3 build with default fusion weights (frozen per `DESIGN-v0.3.md §8.3`).

**Procedure:**
1. For each LOCOMO conversation, construct a fresh engramai `Memory` instance with an in-memory SQLite backend.
2. Replay the conversation episodes via the standard `ingest(episode)` API (owned by v03-resolution).
3. For each query in the conversation's question set, call `Memory::graph_query(...)` (owned by v03-retrieval §6.2), capture the typed `RetrievalOutcome`, and extract the answer string per LOCOMO's scoring conventions.
4. Score each answer against the gold label using LOCOMO's official scorer (vendored as a library, see §9.1).
5. Aggregate scores by category and overall. Temporal category is reported separately per GOAL-5.2.

**Output:**
- `locomo_summary.json`: `{overall: f64, by_category: {temporal: f64, …}, n_queries: usize}`
- `locomo_per_query.jsonl`: one line per query with `{id, category, predicted, gold, score, latency_ms}`
- Reproducibility record (§6).

**Gate bindings:**
- GOAL-5.1 consumes `overall`
- GOAL-5.2 consumes `by_category.temporal` (compared to a constant `GRAPHITI_TEMPORAL_BASELINE` committed in `baselines/external.toml`, §5.3)

**Determinism:**
- Fusion weights frozen via `FusionConfig::locked()` — cross-ref `v03-retrieval/design.md §5.4`.
- Temperature=0 for any LLM calls; model identifiers pinned in the reproducibility record.
- Query order: file order (deterministic).

### 3.2 LongMemEval driver

**Binary:** `engram-bench longmemeval`

**Input:**
- LongMemEval dataset at pinned SHA (`fixtures/longmemeval/<sha>/`).
- v0.3 build with default fusion weights.
- The v0.2 baseline number captured per §5.1 (read from `baselines/v02.toml`).

**Procedure:** analogous to §3.1, using LongMemEval's scoring conventions. The v0.2 baseline is *not* rerun during a v0.3 benchmark run — it is captured once (§5.1) and committed.

**Output:**
- `longmemeval_summary.json`: `{overall: f64, v02_baseline: f64, delta_pp: f64}`
- `longmemeval_per_query.jsonl`
- Reproducibility record.

**Gate binding:**
- GOAL-5.3 consumes `delta_pp` (≥ 15.0 required for release).

### 3.3 Cost harness (N=500 episode corpus)

**Binary:** `engram-bench cost`

**Corpus composition:**
- The N=500 corpus is fixed and committed. Composition: 250 episodes sampled deterministically (seeded RNG) from the LOCOMO test set, and 250 episodes sampled from an anonymized rustclaw production trace (see §9.3 for anonymization).
- Both halves together reach exactly N=500. The sampling seed and selection indices are committed so the corpus is bit-identical across runs.

**Procedure:**
1. For each episode, call `ingest(episode)` on a fresh engramai `Memory` instance configured with the counter reset to zero.
2. After ingest returns, read the per-stage LLM call counters exposed by GOAL-2.11 (owned by v03-resolution). Accumulate into a running sum.
3. After all 500 episodes, compute `average_llm_calls = total_calls / 500`.

**Output:**
- `cost_summary.json`: `{n_episodes: 500, total_calls: usize, average: f64, by_stage: {extraction: f64, resolution: f64, …}}`
- `cost_per_episode.jsonl`: `{episode_id, calls_by_stage, total_calls}`
- Reproducibility record.

**Gate binding:**
- GOAL-5.4 consumes `average` (≤ 3.0 required).

**Counter source:** the same counters exposed by v03-resolution for GUARD-12 production observability. The cost harness is GUARD-12's ship-gate consumer; the production consumer is a separate concern. No duplication of measurement logic.

**Determinism caveat:** if any resolution stage involves a non-deterministic LLM call, the counter *count* is still deterministic (one call per stage per trigger condition) even when the *content* varies. Temperature=0 keeps content stable; the count is robust regardless.

### 3.4 Test-preservation harness (v0.2 suite replay)

**Binary:** none — this is a `cargo test` invocation wrapped by a CI script.

**Procedure:**
1. Check out v0.2.2 tag, extract test source files and test-time fixtures into a workspace-temporary directory.
2. Apply the migration tool (owned by v03-migration) to the v0.2 fixture DBs in place.
3. Run `cargo test` against v0.3 source, with the v0.2 test sources imported as an additional test target (`tests/v02_preservation.rs` — a wrapper that includes the extracted tests).
4. Any test that fails is surfaced in the output and either (a) must be annotated in `v02_exceptions.toml` with a documented rationale, or (b) blocks release.

**v0.2 count freeze:** The exact count (currently "~280") is resolved and committed as `baselines/v02_test_count.toml` before v0.3 development begins (§5.2). The harness asserts the actual test count matches this number — a drift (e.g., someone deletes a test) is itself a failure.

**Output:**
- `test_preservation_summary.json`: `{total: usize, passed: usize, failed: usize, exceptions: [...]}`
- `test_preservation_failures.log`: stderr for each failure.

**Gate binding:**
- GOAL-5.5 passes iff `failed - exceptions.len() == 0` AND `total == frozen_count`.

### 3.5 Cognitive-feature regression harness

**Binary:** `engram-bench cognitive-regression`

**Scope:** three features — interoceptive (self-state modulating retrieval), metacognition (confidence affecting filtering), affect (mood-congruent recall).

**Procedure (per feature):**
1. Prepare two engramai `Memory` instances seeded with identical episode data.
2. On instance A, set the feature's state to value S1 (e.g., high-positive affect).
3. On instance B, set the feature's state to value S2 (e.g., high-negative affect).
4. Run an identical query on both. Collect the top-K ranked results.
5. Compute a directional metric:
    - **interoceptive/affect**: Jaccard distance between top-K IDs (A vs B). Must exceed a threshold (default 0.2 — i.e., at least 2/10 items differ).
    - **metacognition**: number of filtered-out items (A vs B) must differ under different confidence settings.

**Gate binding:**
- GOAL-5.6 passes iff all three features show the expected directional difference on a fixed test set (committed under `fixtures/cognitive_regression/`).

**Why directional, not quality:** measuring whether mood-congruent recall is "good" requires a labeled dataset that doesn't exist. Measuring whether mood *changes the output* is robust and catches the regression of interest ("feature got silently disconnected from ranking").

### 3.6 Migration data-integrity harness

**Binary:** `engram-bench migration-integrity`

**Input:**
- A copy of rustclaw's production `engram-memory.db` (the live DB, snapshot copied — not modified in place).
- A fixed query set of ≥ 20 queries (`fixtures/migration_queries.toml`) that work on the pre-migration DB.

**Procedure:**
1. Take a checksum of the pre-migration DB.
2. Run each of the ≥ 20 queries against the pre-migration DB via the v0.2 API; record each result set.
3. Apply the migration tool (v03-migration) to produce a v0.3 DB.
4. Run each query against the v0.3 DB via the v0.3 API; record each result set.
5. Assert no MemoryRecord / Hebbian link / Knowledge Compiler topic was lost (cross-ref GOAL-4.1, owned by v03-migration — this harness consumes the counts the migration tool reports in its CLI progress output).
6. Assert each query's v0.3 result set is "equivalent" to the v0.2 result set.

**Definition of query equivalence:** a structured comparison, not string match —
- Result *count* within ±0 for exact-match queries, ±10% for similarity queries.
- Every v0.2 top-3 result is either (a) present verbatim in the v0.3 top-10 (recall preservation, not rank preservation), OR (b) satisfies the type-substitution rule below via a topic in v0.3's top-10 whose `source_list` includes it.
- **Type-substitution rule:** if a result changed *type* (e.g., v0.3 returned a knowledge-topic where v0.2 returned a raw record), the originating record(s) must appear within the topic's `source_list`. A topic substitution for a v0.2 top-3 record is equivalent iff the record is in the topic's `source_list` AND the topic appears in v0.3's top-10.

**Precedence:** rules are checked in order. A v0.2 top-3 record satisfies equivalence if **either** rule (a) or rule (b) holds — not both required. This prevents a strict reading of "must appear in top-10" from failing perfectly-acceptable topic substitutions, and prevents a permissive reading from masking real recall regressions.

**Output:**
- `migration_integrity_summary.json`: `{pre_checksum, post_checksum, records: {pre, post, lost: 0?}, links: {pre, post, lost: 0?}, topics: {pre, post, lost: 0?}, queries: {total, equivalent, divergent: [...]}}`

**Gate binding:**
- GOAL-5.7 passes iff `lost == 0` everywhere AND `divergent == []`.

---

## 4. Gate Definitions & Decision Rules

A gate is a `(metric, threshold, comparator)` triple plus a failure semantics. This section enumerates each gate formally.

### 4.1 P0 ship gates

P0 gates **block release if failed** — no justification accepted short of explicit manual override with signed-off rationale (see §4.4).

| GOAL     | Metric                                   | Comparator | Threshold                                  | Source           |
| -------- | ---------------------------------------- | ---------- | ------------------------------------------ | ---------------- |
| GOAL-5.1 | `locomo.overall`                         | `≥`        | `0.685`                                    | §3.1 (constant)  |
| GOAL-5.2 | `locomo.by_category.temporal`            | `≥`        | `GRAPHITI_TEMPORAL_BASELINE` (§5.3)        | §3.1             |
| GOAL-5.3 | `longmemeval.delta_pp`                   | `≥`        | `15.0` pp                                  | §3.2             |
| GOAL-5.4 | `cost.average`                           | `≤`        | `3.0` LLM calls / episode                  | §3.3             |
| GOAL-5.5 | `tests.failed - tests.exceptions.len()` AND `tests.total == frozen_count` | `== 0` AND equality | — | §3.4 |

A release-qualification run that returns `P0_GATES_PASS = false` **must not** produce a release artifact. The CI release pipeline aborts before artifact signing.

### 4.2 P1 quality gates

P1 gates are *strongly* required, but accept documented failure under written rationale (e.g., "LOCOMO dataset bug in category X; filed upstream; ship with noted caveat").

| GOAL     | Metric                                                             | Comparator    | Threshold                  | Source |
| -------- | ------------------------------------------------------------------ | ------------- | -------------------------- | ------ |
| GOAL-5.6 | All 3 cognitive features show directional difference              | `∀ features`  | threshold per feature (§3.5) | §3.5   |
| GOAL-5.7 | `migration.lost == 0` AND `migration.divergent == []`             | equality      | —                          | §3.6   |

A P1 failure with accepted rationale generates a **release note entry** in the artifact — the caveat is surfaced to users, not hidden.

### 4.2a Meta-gate: reproducibility record validity (GOAL-5.8)

GOAL-5.8 requires that every run emits a complete, schema-valid reproducibility record (§6.1). A corrupt or incomplete record silently breaks the reproducibility contract — GUARD-2 requires a gate that catches this loudly.

| GOAL     | Metric                                                             | Comparator | Threshold | Source |
| -------- | ------------------------------------------------------------------ | ---------- | --------- | ------ |
| GOAL-5.8 | `record.schema_valid && all_required_fields_present && override_fields_present_iff_override_used` | `== true` | — | §6, §7.3 |

**Level:** P1 quality gate. Rationale: a missing/corrupt record does NOT make the build unsafe at runtime (the binary itself is fine); it makes the release unreproducible. That is a serious quality defect worth failing on, but not a P0 "the software is broken" condition.

**Execution:** this meta-gate runs **last**, after every driver has emitted its section of the record and the harness has assembled the final TOML. Validation checks:
1. Schema conformance (TOML parses; all required top-level tables present: `[meta]`, `[build]`, `[dataset]`, per-driver sections, `[gates]`).
2. Every gate in §4.1 / §4.2 / §4.2a is represented in `[gates]` with `(metric, threshold, comparator, measured, status)` populated.
3. If and only if `--override-gate` was used: `[override]` table is present with `gate`, `rationale_file`, `rationale_sha`, `operator` all populated; rationale file exists and its SHA matches.
4. No field contains a sentinel value (e.g., `"TODO"`, empty string where a value was required).

**Surface:** rendered in §10.1 summary table as its own line, same format as other gates. On failure, the stdout includes `[FAIL-P1] GOAL-5.8 reproducibility record` and names the first missing/malformed field.

**Why this exists as a separate meta-gate and not a side-effect check:** side-effect checks ("oh, the record didn't emit properly, we'll log a warning") silently degrade by design — GUARD-2 forbids that. The meta-gate makes the reproducibility-by-contract promise enforceable, not advisory.

### 4.3 Gate evaluation order & dependency DAG

Gates have two kinds of ordering: a **data-dependency DAG** (you cannot measure X before building Y) and a **cost-based preference** (within a DAG stage, run cheap gates first for fast feedback).

**Dependency DAG** (mandatory; violating this produces ERRORs, not meaningful FAILs):

```
[stage 0: build]
    build engram (rustc)
    build engram-bench (rustc)
    build migration tool (rustc)  ← required for stages touching migration
        │
        ├─→ [stage 1: independent drivers]
        │       §3.1 LOCOMO            (fresh in-mem DB)
        │       §3.2 LongMemEval       (fresh in-mem DB)
        │       §3.5 cognitive-regression (fresh in-mem DB, 3 features)
        │       §3.3 cost harness      (fresh in-mem DB; reads ResolutionStats per §12.1)
        │
        └─→ [stage 2: migration-dependent drivers]
                §3.4 test-preservation (runs v0.2 test suite against migrated DB)
                §3.6 migration-integrity (runs pre/post-migration queries)
```

**Within-stage cost ordering** (soft preference, fast-fail feedback):

1. Cheap first: §3.4 test-preservation, §3.5 cognitive-regression (minutes).
2. Medium: §3.3 cost harness (N=500 ingest is minutes, not hours).
3. Expensive last: §3.1 LOCOMO, §3.2 LongMemEval (may take hours; see §8.2 per-driver budgets).

**Parallel execution:** the harness SHOULD run stage-1 drivers in parallel when system resources permit (independent fresh DBs, no shared fixtures beyond read-only inputs). Stage-2 drivers may also parallelize with each other (different queries, same migrated DB). Stages are sequential.

**Short-circuit policy: never.** Every gate that *can* run, does run. All gates produce results; the summary report (§10.1) prints all of them. A failing P0 gate does not skip remaining gates — we want the full picture before deciding how far from release we are.

**Upstream failure propagation:** if an upstream stage blocks a downstream gate (e.g., migration tool build fails → §3.4 and §3.6 cannot execute), the downstream gates are reported as `ERROR` with a clear `blocked_by: <upstream-stage>` field in the gate result — NOT `FAIL` (they weren't measured) and NOT skipped silently (GUARD-2). This distinction matters: FAIL means "we measured and the result was bad"; ERROR means "we could not measure."

**Why no short-circuit:** when a human reads "LOCOMO failed", they want to know simultaneously whether cost also failed (two problems) or not (one problem). Stopping at the first failure hides that.

### 4.4 Failure semantics (GUARD-2: never silent degrade)

The harness enforces GUARD-2 at three levels:

**Level 1 — driver errors must fail loud.** If a driver crashes (panic, fixture missing, scorer error), the gate is reported `ERROR`, not `FAIL` and not skipped. `ERROR ≠ PASS` — an errored P0 gate blocks release exactly like a failed P0 gate. No accidental pass via "we couldn't measure it."

**Level 2 — threshold violations are visible.** The summary table (§10.1) prints the metric, threshold, and status on one line per gate. A P0 failure is highlighted (colorized in TTY, marked `[FAIL-P0]` in non-TTY).

**Level 3 — no silent downgrade.** Downgrading a P0 to P1 requires a commit-level change to `requirements.md` — not a runtime flag. Downgrading P1 to P2 likewise. The harness has **no runtime knob** that can soften a gate threshold. Thresholds live in code constants and requirements.md; changing them is a PR visible in git history.

**Manual override:** a release manager can pass `--override-gate=GOAL-5.X --rationale=<file.md>` to produce a release despite a P0 failure. This requires:
- The rationale file must exist and contain a non-empty rationale.
- The rationale is embedded verbatim in the release artifact (release notes, reproducibility record, git tag message).
- The override event is logged to `.gid/releases/overrides.log` (committed).

No release can override a P0 gate without these artifacts. "Accidentally shipped despite failing" is made impossible by the pipeline.

---

## 5. Baseline Capture (Pre-v0.3 Preconditions)

Three baselines **must be committed to the repository before v0.3 implementation begins**. Without them, the deltas in GOAL-5.3 and GOAL-5.5 are not measurable and the gates are not enforceable.

### 5.1 v0.2 LongMemEval baseline

**Precondition for GOAL-5.3.**

**Procedure (one-time, on v0.2.2 tag):**
1. Check out tag `v0.2.2`.
2. Build the v0.2 benchmark harness (a simplified predecessor of this design, sufficient to run LongMemEval).
3. Run LongMemEval against v0.2 with the same dataset SHA that v0.3 will use.
4. Record the overall score and per-category sub-scores.
5. Commit to `baselines/v02.toml`:
    ```toml
    [longmemeval]
    dataset_sha = "…"
    v02_tag = "v0.2.2"
    overall = 0.xxx
    by_category = { factual = 0.xxx, episodic = 0.xxx, … }
    captured_at = "2026-MM-DD"
    captured_by = "potato"
    ```

**Invariant:** once committed, `baselines/v02.toml` is immutable. Any change is a PR with explicit rationale ("we found a bug in the v0.2 harness" is the only acceptable rationale; "we want a lower bar" is not).

**Why this matters:** without a pinned baseline, GOAL-5.3 can be trivially satisfied by choosing a bad v0.2 rerun on the day of release. Pinning it pre-development prevents gate-goalposting.

### 5.2 v0.2 test count freeze

**Precondition for GOAL-5.5.**

**Procedure:**
1. On v0.2.2 tag, run `cargo test --no-run` to list all test functions.
2. Count them deterministically (sort, unique, count).
3. Commit to `baselines/v02_test_count.toml`:
    ```toml
    [tests]
    v02_tag = "v0.2.2"
    total = 280   # resolved exact number
    list_sha = "sha256 of sorted test-name list"
    captured_at = "2026-MM-DD"
    ```

The `list_sha` lets the harness detect drift without committing a 280-line file.

### 5.3 External baseline numbers (mem0, Graphiti)

**Precondition for GOAL-5.1, GOAL-5.2.**

External baselines come from published papers / repos, not rerun locally. Commit the numbers with provenance:

```toml
# baselines/external.toml
[locomo.mem0]
overall = 0.685
source = "mem0 paper, Table 3, 2024-XX-XX version"
url = "https://…"

[locomo.graphiti]
temporal = 0.xxx                      # resolved before v0.3 ships; placeholder flagged by harness if missing
source = "Graphiti paper, Table Y"
url = "https://…"
```

**Harness behavior:** if any value is `null` or missing at release-qualification time, the corresponding gate reports `ERROR` (not `PASS`), blocking release per §4.4 Level 1.

---

## 6. Reproducibility Record Schema

Per GOAL-5.8, every benchmark run (every driver invocation) emits a reproducibility record. This section specifies its schema, layout, and replay workflow.

### 6.1 Record contents

```toml
# reproducibility.toml
[run]
driver = "locomo"              # or "longmemeval" | "cost" | "test-preservation" | "cognitive-regression" | "migration-integrity"
started_at = "2026-MM-DDTHH:MM:SSZ"
finished_at = "2026-MM-DDTHH:MM:SSZ"
status = "pass" | "fail" | "error"

[build]
engram_commit_sha = "…"
engram_version = "0.3.0-rc.1"
cargo_profile = "release"
rustc_version = "1.…"
host_triple = "aarch64-apple-darwin"

[dataset]
locomo_sha = "…"               # populated per driver
longmemeval_sha = "…"
cost_corpus_seed = 42
cost_corpus_selection_sha = "…"

[fusion]
weights = { factual = 0.x, episodic = 0.y, … }  # captured from FusionConfig::locked()
frozen = true

[models]
embedding_model = "…"
rerank_model = "…"
llm_model = "…"
llm_temperature = 0.0

[result]
# driver-specific summary, e.g., for locomo:
locomo_overall = 0.xxx
locomo_by_category = { temporal = 0.xxx, … }

[gates]
# evaluated gate outcomes for this run
"GOAL-5.1" = { metric = 0.xxx, threshold = 0.685, status = "pass" }
"GOAL-5.2" = { metric = 0.xxx, threshold = 0.xxx, status = "pass" }

[override]
# present only if --override-gate was used
gate = "GOAL-5.X"
rationale_file = "…"
rationale_sha = "…"
operator = "…"
```

**Invariant:** every field except `[override]` is always present. Missing values are represented as `null` (and cause `status = "error"` if the missing field is gate-relevant).

### 6.2 On-disk layout

```
benchmarks/
  runs/
    2026-MM-DDTHH-MM-SSZ_<driver>_<short-sha>/
      reproducibility.toml
      <driver>_summary.json
      <driver>_per_query.jsonl       # if applicable
      stderr.log
      stdout.log
  baselines/
    v02.toml
    v02_test_count.toml
    external.toml
```

**Committed vs not:**
- `benchmarks/baselines/` is always committed.
- `benchmarks/runs/` is committed **only for release-qualification runs** (tagged releases and RCs). Development runs stay local — otherwise the repo grows without bound.
- A `.gitattributes` rule marks `runs/**/per_query.jsonl` as `diff=off binary` to avoid diff noise.

### 6.3 Replay workflow

To reproduce a historical benchmark run:

1. Read `reproducibility.toml`. Extract `build.engram_commit_sha`, `build.rustc_version`, `dataset.*_sha`, `fusion.weights`, `models.*`.
2. Check out `engram_commit_sha` in a fresh workspace.
3. Ensure rustc matches (or accept drift with a documented delta).
4. Download datasets matching the SHAs from `fixtures/` (or fetch from the pinned release URLs).
5. Run: `engram-bench <driver> --from-record <path-to-reproducibility.toml>`.
6. The harness validates every precondition before running, emits a new record, and diffs the `[result]` section against the historical one.

Expected outcome: bit-identical results under determinism guarantees (§3.1 determinism subsection) within rounding tolerance of 1e-6 on scores. Divergence is a regression event reported by the CI replay job (§8.2).

---

## 7. Harness API & CLI

### 7.1 `engram-bench` CLI surface

All benchmark drivers live in one binary, `engram-bench`, under `crates/engram-bench/`. One binary with subcommands keeps the shared harness code (fixture loading, record emission, gate evaluation) in one crate — no duplicated infrastructure.

```
engram-bench <driver> [flags]

Drivers:
  locomo                    Run LOCOMO suite                        (GOAL-5.1, 5.2)
  longmemeval               Run LongMemEval suite                   (GOAL-5.3)
  cost                      Run cost harness (N=500)                (GOAL-5.4)
  test-preservation         Run v0.2 test replay                    (GOAL-5.5)
  cognitive-regression      Run cognitive-feature directional tests (GOAL-5.6)
  migration-integrity       Run migration data-integrity suite      (GOAL-5.7)
  release-gate              Run ALL drivers in §4.3 order and emit combined summary

Flags (shared):
  --fixtures-dir <path>     Override fixture root (default: benchmarks/fixtures/)
  --output-dir <path>       Run record destination  (default: benchmarks/runs/<timestamp>_<driver>_<sha>/)
  --format <summary|json>   Stdout format (default: summary)
  --from-record <path>      Replay from reproducibility.toml
  --override-gate GOAL-N.X  Manual P0 override (requires --rationale)
  --rationale <path>        Rationale file (required with --override-gate)

Flags (driver-specific):
  locomo     --categories=<list>  Restrict to specific categories (debug only, never in release-gate)
  cost       --n=<usize>          Override N=500 (debug only)
  …
```

**Exit codes:**
- `0` — all gates the driver evaluated passed.
- `1` — at least one gate failed.
- `2` — driver error (fixture missing, crash, unrecoverable).
- `3` — override used (signals to CI: do not auto-release, human must acknowledge).

`release-gate` aggregates: exit `0` iff all P0 pass and all P1 pass-or-have-rationale; `1` on any P0 fail; `2` on any driver error; `3` on any override.

### 7.2 Rust API (for CI integration)

For CI steps that need to inspect results programmatically (not just parse stdout), the harness exposes:

```rust
// In crates/engram-bench/src/lib.rs

pub struct GateResult {
    pub goal: String,            // e.g. "GOAL-5.1"
    pub metric: f64,
    pub threshold: f64,
    pub comparator: Comparator,  // Ge, Le, Eq, …
    pub status: GateStatus,      // Pass | Fail | Error
    pub priority: Priority,      // P0 | P1 | P2
    pub message: Option<String>, // human-readable if Fail/Error
}

pub struct RunReport {
    pub driver: Driver,
    pub record_path: PathBuf,
    pub gates: Vec<GateResult>,
    pub summary_json: serde_json::Value,
}

pub trait BenchDriver {
    fn name(&self) -> Driver;
    fn run(&self, config: &HarnessConfig) -> Result<RunReport, BenchError>;
}

pub fn run_release_gate(config: &HarnessConfig) -> Result<Vec<RunReport>, BenchError>;
pub fn aggregate_release_decision(reports: &[RunReport]) -> ReleaseDecision;
```

`ReleaseDecision`:
```rust
pub enum ReleaseDecision {
    Ship,                        // all P0 pass, all P1 pass or justified
    Block { failed_p0: Vec<String> },
    ConditionalShip { overridden: Vec<Override>, p1_rationales: Vec<Rationale> },
}
```

The Rust API is used by the release-qualification CI script (§8.2) to make the ship/no-ship decision in one place, rather than having the shell parse exit codes from six drivers.

### 7.3 Output formats (summary, per-query, reproducibility record)

Three output artifacts per driver run:

1. **Stdout summary** — one-screen human-readable table (see §10.1). Written even on error so a failing driver still produces usable output.
2. **Per-query JSONL** (`<driver>_per_query.jsonl`) — one JSON object per query, with gold/predicted/score. Used for drill-down (§10.2) and for diffing across runs.
3. **Reproducibility record** (`reproducibility.toml`) — schema per §6.1.

All three are always emitted. A driver that fails mid-run emits partial artifacts plus an error record — "we got halfway and crashed" is useful information, silently not-writing is not.

---

## 8. CI Integration

### 8.1 Which suites run on every commit

On every push / PR to `main` and release branches:

- `engram-bench test-preservation` — runs the v0.2 test replay.
- `engram-bench cognitive-regression` — runs the three-feature directional test.
- A **smoke subset** of LOCOMO (~50 queries) and LongMemEval (~50 queries) — catches gross regressions without the full multi-hour run.

These together should take < 10 minutes on a standard CI runner. Exceeding that budget means we split the smoke subset further.

**Failure behavior:** blocks merge to `main`. P0 gate failures block merge; the smoke subset is treated as a P1-style gate (failure → review required, not auto-block).

### 8.2 Which suites run on release-qualification only

On `vX.Y.Z-rc.N` tags and the final `vX.Y.Z` tag:

- `engram-bench release-gate` — full suite, all drivers.
- The run is pinned to specific runner hardware (recorded in `build.host_triple`) for determinism.
- Output artifacts are committed to `benchmarks/runs/` (§6.2 "committed").
- A replay job (`engram-bench --from-record ...`) runs against the committed record 24 hours later; a regression beyond tolerance fails the replay and triggers an alert.

**Duration budget:** 4–8 hours for a full release-gate run (LOCOMO + LongMemEval dominate). This is acceptable because release qualification happens rarely and has a release engineer in the loop.

**Per-driver soft budgets** (committed as targets; the harness logs actual durations and CI alerts if any driver exceeds its target by 50%):

| Driver                     | Target | Notes                                                           |
| -------------------------- | ------ | --------------------------------------------------------------- |
| §3.1 LOCOMO                | ~3h    | N ≈ 2000 queries × ~5s/query incl. LLM scorer                   |
| §3.2 LongMemEval           | ~3h    | N ≈ 500 long-context queries × ~20s/query                       |
| §3.3 cost harness          | ~30min | N = 500 ingests, no query phase; dominated by resolution latency |
| §3.4 test-preservation     | ~10min | Runs the v0.2 cargo-test suite against migrated DB              |
| §3.5 cognitive-regression  | ~15min | 3 features × small query set                                    |
| §3.6 migration-integrity   | ~15min | 20+ queries pre/post migration                                  |
| §4.2a reproducibility meta | <1min  | Schema validation only                                          |
| Buffer (rustc, I/O, setup) | ~30min | Build time + fixture download + assembly                        |

**Purpose of per-driver budgets:** catches silent perf regressions in the harness itself. Without them, "release-gate took 9h this run" is opaque — with them, the log pinpoints which driver doubled. The soft budgets are not ship gates; they are CI observability.

### 8.3 Cache & fixture management

Fixtures (LOCOMO, LongMemEval, cost corpus seed, migration queries) are:

1. **Not committed to git as raw files** (sizes too large, licensing constraints).
2. Downloaded from pinned URLs to `benchmarks/fixtures/<dataset>/<sha>/` on first use.
3. Verified against a SHA-256 checksum before use.
4. Cached across CI runs via the CI cache layer keyed on dataset SHA.
5. Cleaned up via `engram-bench clean-fixtures` (removes stale SHAs beyond the last 2).

**GUARD-9 compliance:** fixtures are test-only artifacts. The crate's `[dependencies]` section in `Cargo.toml` gains no new entries; `[dev-dependencies]` may gain TOML/JSON parsers for fixture files, which are already in the workspace. Dataset downloads use `std::process::Command("curl", …)` or a minimal HTTP client already present. No new runtime dependency is introduced.

---

## 9. Datasets & Fixtures

### 9.1 LOCOMO acquisition & version pinning

- **Source:** official LOCOMO repository (URL pinned in `benchmarks/fixtures/locomo/source.toml`).
- **Pin format:** git commit SHA of the LOCOMO repo at the time of adoption. All runs reference this SHA.
- **Scorer:** LOCOMO ships an official scoring script. We vendor it as a Rust crate wrapper in `crates/engram-bench/src/scorers/locomo.rs` — a thin port that matches the published scorer's behavior bit-for-bit on a fixture of 50 known queries (unit-tested). If LOCOMO publishes a scorer change, we update the vendored scorer AND rerun the v0.3 benchmark AND update the reproducibility records.
- **Version upgrade policy:** a LOCOMO dataset-SHA change during a v0.3 release cycle is a PR that must rerun all affected benchmarks. The SHA change is visible in git history, preventing silent dataset shifts that would invalidate gate comparisons.

### 9.2 LongMemEval acquisition & version pinning

Same policy as §9.1, against the LongMemEval repo. Scoring is LongMemEval's own scorer, vendored identically.

### 9.3 rustclaw production trace (anonymization, sampling)

The cost corpus (§3.3) includes 250 episodes from rustclaw's live usage. These contain potato's personal data and MUST be anonymized before use as a benchmark fixture.

**Anonymization pipeline (one-time, committed output):**
1. Sample 250 episodes from rustclaw's `engram-memory.db` using a seeded RNG (seed committed).
2. Run each episode through the anonymization transformer:
    - Named entities (people, places, URLs, tokens) → replaced with placeholders `<PERSON_1>`, `<URL_7>`, etc.
    - Timestamps → offset by a fixed delta (committed) so temporal structure is preserved but absolute dates are shifted.
    - Message content preserved at structure level; surface content replaced.
3. Manual review pass by potato — commit the approved fixture.
4. Commit to `benchmarks/fixtures/rustclaw_trace/<sha>/` alongside the anonymization log.

**Why anonymize rather than exclude?** The whole point of including rustclaw's trace is that it tests the system on realistic messy episode data that LOCOMO doesn't fully represent. Excluding it leaves a distributional hole in the cost measurement.

**Why commit rather than regenerate?** Deterministic sampling + deterministic anonymization = same fixture every time, but committing it prevents the "it changed and no one noticed" failure mode.

**Update policy:** the trace is captured once pre-v0.3 and frozen for the v0.3 release cycle. A future release (v0.4+) can refresh it; v0.3 must not, to keep gate comparisons stable.

### 9.3.1 Anonymization mechanism (one-shot precondition specification)

Because the anonymized rustclaw trace feeds GOAL-5.4 (P0 ship gate) and is frozen for the entire v0.3 release cycle, the mechanism must be fully specified. A slightly-wrong corpus contaminates the cost measurement for the whole release — so this one-shot process is pinned down **more** tightly than a recurring one, not less.

**Mechanism: deterministic regex + allow-list.** Not LLM-based. Rationale:
- Deterministic (same input → byte-identical output; required for the idempotence test in §11).
- Auditable (the regex catalog and allow-list are text files committed to the repo; a human can verify coverage).
- No dependency on the system under test (an LLM-based anonymizer that relied on engram's own pipeline would create a circular dependency — the feature under benchmark cannot be a prerequisite of the benchmark fixture).

**Transformer catalog** (committed to `benchmarks/fixtures/rustclaw_trace/anonymizer/`):
- `patterns.toml`: the regex catalog with typed replacements (emails → `<EMAIL_N>`, URLs → `<URL_N>`, named-entity tokens from a frozen spaCy NER pass → `<PERSON_N>`/`<ORG_N>`/`<PLACE_N>`, absolute timestamps → shifted by committed delta).
- `allowlist.toml`: tokens that look like PII but are not (e.g., "RustClaw" the project name, "engram" the crate name, "potato" the agent's own alias — committed by potato's review decision).
- `delta.toml`: the fixed timestamp offset applied to every timestamp in the corpus, committed once and never changed for v0.3.

**NER model pinning:** the spaCy model is pinned to a specific version (`en_core_web_lg==3.7.1` or similar; recorded in the anonymizer's `requirements.txt`). Upgrading the model is a v0.4+ concern — v0.3 freezes the NER to preserve reproducibility.

**Acceptable leak tolerance: zero.** Procedure:
1. Automated pass produces anonymized corpus + a human-readable diff (`diff.txt`: every original span and its replacement).
2. potato reviews `diff.txt` end-to-end.
3. Any PII leak found → regenerate the entire corpus, re-review. Partial patches ("just fix this one span") are forbidden because they create ad-hoc state outside the committed transformer catalog.
4. Corpus is committed only after clean review.

**Failure handling: all-or-nothing.** The anonymizer processes all 250 sampled episodes or aborts with non-zero exit. Partial corpuses (e.g., "244 processed, 6 crashed") are never committed, because that would silently break the `N == 500` invariant (§3.3: 250 synthetic + 250 rustclaw = 500 total). If the anonymizer crashes on any episode:
1. Log the crash with the offending episode ID and input span.
2. Fix the transformer (add missing pattern / extend allow-list / etc.).
3. Rerun the full anonymization pass. Intermediate partial outputs are discarded.

**Idempotence test** (cross-linked from §11): running the anonymizer twice on the same input + same catalogs + same delta produces byte-identical output. This test runs in CI on a small fixture so a catalog regression is caught immediately.

**Why so strict for a one-shot process?** Recurring processes have the "we'll fix it next run" safety net. This one doesn't — whatever ships with v0.3 is locked in. Strictness up-front is the cost of skipping the retry safety net.

### 9.4 Fixture storage & GUARD-9 compliance

- Fixtures are **test-only** — imported via `dev-dependencies` or `build.rs`, never from the main crate.
- Fixture downloads do not add runtime dependencies to the published crate (the `engram-bench` crate is separately published or not-published; it does not appear in the dependency graph of `engramai` itself).
- A `cargo build -p engramai` succeeds without any fixture present. Only `cargo bench` / `cargo run -p engram-bench` require fixtures.

This boundary means users installing `engramai` from crates.io get zero benchmark machinery in their binary; developers running benchmarks get the full harness.

---

## 10. Observability & Reporting

### 10.1 Summary table (single-line gate status)

Stdout summary format (printed by every driver and by `release-gate`):

```
Engram v0.3 Release Gate Summary
================================
Build:    engram @ a1b2c3d (v0.3.0-rc.1)   rustc 1.XX   aarch64-apple-darwin
Started:  2026-MM-DDTHH:MM:SSZ   Finished: 2026-MM-DDTHH:MM:SSZ   Duration: 5h42m

P0 Ship Gates
─────────────
  [PASS] GOAL-5.1  LOCOMO overall               0.712 ≥ 0.685
  [PASS] GOAL-5.2  LOCOMO temporal              0.698 ≥ 0.650  (Graphiti)
  [PASS] GOAL-5.3  LongMemEval delta          +17.4pp ≥ +15.0pp  (v0.2: 0.540 → v0.3: 0.714)
  [FAIL] GOAL-5.4  Cost per episode              3.21 ≤ 3.00    ← exceeds budget by 7%
  [PASS] GOAL-5.5  v0.2 tests preserved        280/280  (0 exceptions)

P1 Quality Gates
────────────────
  [PASS] GOAL-5.6  Cognitive-feature regression    3/3 pass
  [PASS] GOAL-5.7  Migration integrity             0 lost, 20/20 queries equivalent
  [PASS] GOAL-5.8  Reproducibility record         schema valid, 7/7 required sections present

Decision: BLOCK  (1 P0 gate failed: GOAL-5.4)
Artifacts: benchmarks/runs/2026-MM-DDTHH-MM-SSZ_release-gate_a1b2c3d/
```

**Colorization:** `[PASS]` green, `[FAIL]` red, `[ERROR]` bright red, only when stdout is a TTY; plain text otherwise. No ANSI codes leak into committed reproducibility records.

### 10.2 Per-gate drill-down

`engram-bench <driver> --format json` emits the full report structured. A convenience command:

```
engram-bench explain GOAL-5.4 --run benchmarks/runs/<latest>/
```

prints:
- The metric definition.
- The threshold and comparator.
- The raw measurement (with substructure — e.g., cost broken down by stage).
- The top-10 episodes contributing most to the measurement (e.g., highest per-episode LLM calls).
- The delta vs the previous release-qualification run, if one exists.

This is the tool a developer uses when a gate fails and they need to understand *why*.

### 10.3 Regression alerting

On release-qualification runs:

1. The CI job emits a summary to a release-engineering channel (Telegram/Slack — channel-agnostic; specified in CI config, not here).
2. Any P0 fail triggers a high-priority alert with the summary table and the artifact path.
3. The 24-hour replay job (§8.2) compares its results to the original; divergence beyond tolerance triggers a lower-priority alert ("benchmark flakiness or environment drift — investigate").

**Scope boundary:** this section specifies the signals the harness emits. The delivery channels and escalation rules belong to CI/infra config, not to this design.

---

## 11. Testing Strategy (testing the tests)

The benchmark harness is itself code. It must be tested — otherwise gate failures might be harness bugs, not system regressions, and we'd lose faith in the gates.

**Unit tests (in `crates/engram-bench/`):**

- **Scorer parity tests.** For LOCOMO and LongMemEval scorers: a fixture of ~50 known `(predicted, gold, expected_score)` triples. The vendored scorer must match the expected score bit-for-bit. Any scorer behavior change (upstream or ours) breaks this test.
- **Gate evaluation logic.** Given synthetic `RunReport` inputs, assert the correct `ReleaseDecision` is produced. Cover: all-pass, one P0 fail, one P1 fail, override, error.
- **Reproducibility record roundtrip.** Write a record, read it back, assert structural equality. Covers the TOML schema.
- **Corpus sampler determinism.** Given a fixed seed and corpus, the cost harness must select the same 500 episodes across runs (assert equality of selection indices).
- **Anonymizer idempotence.** Running the anonymizer twice on the same input produces the same output.

**Integration tests:**

- **Mini-LOCOMO smoke.** A 10-query subset with hand-crafted expected outputs. End-to-end: fixture → driver → score → gate evaluation. Catches plumbing bugs without the full run cost.
- **Error-path smoke.** Missing fixture, corrupt fixture, panicking scorer — assert that each produces `status = "error"`, non-zero exit code, and visible stderr output (GUARD-2 compliance).
- **Override plumbing.** Manual override + missing rationale → rejected. Override + valid rationale → accepted, rationale embedded in record.

**Self-consistency test:**

A harness self-test called by CI: run the harness against a trivial mock `Memory` that produces known outputs, assert the gates behave as expected. This is the "the harness itself compiles and runs" smoke test.

**Non-goals for this section:**

- We do **not** benchmark the benchmark harness for performance. Slow gate evaluation is annoying, not correctness-threatening.
- We do **not** fuzz the scorer inputs. LOCOMO's scorer is vendored-as-spec; fuzzing is upstream's concern.

---

## 12. Cross-Feature References

This section is the canonical list of hand-offs between this feature and its siblings.

### 12.1 Consumed from v03-resolution

- **GOAL-2.11 counter API.** Cost harness (§3.3) reads per-stage LLM call counters from `ResolutionStats` (the struct exposed by v03-resolution). The benchmark asserts: each counter is monotonically increasing within one episode, resets to zero between episodes, and aggregates correctly. If v03-resolution changes the counter field names, this driver updates; gate threshold (≤3) does not.
- **`ingest(episode)` API.** Driven identically in every harness that loads data.
- **`resolve_for_backfill` (from migration design §12).** Not consumed here — that's migration → resolution. No direct benchmark dependency.

### 12.2 Consumed from v03-retrieval

- **`Memory::graph_query(...)` / `GraphQuery` API** (v03-retrieval §6.2). Every gate that scores retrieval quality drives this API. (The earlier draft referred to this as `Engram::query` — pre-canonical naming; normalized per v03-retrieval §6.1 GUARD-11 note.)
- **Typed `RetrievalOutcome`** (v03-retrieval §6.4). Drivers use the typed variant to extract predicted answers without string-guessing the internal format.
- **`FusionConfig::locked()`** (v03-retrieval §5.4). Used to pin fusion weights per §3.1 determinism guarantee.

### 12.3 Consumed from v03-migration

- **Migration CLI** (v03-migration §9). Test-preservation harness (§3.4) and migration-integrity harness (§3.6) both invoke this.
- **Migration progress/statistics output** (v03-migration §9). The integrity harness parses counts (records/links/topics pre- and post-) from this output to assert zero loss.
- **Topic reconciliation output** (v03-migration §6). Used by the integrity harness to validate that a pre-migration query returning a raw record is satisfied post-migration by a knowledge topic whose source list includes that record.

### 12.4 Consumed from v03-graph-layer

- Read-only consumer via the retrieval API. No direct graph-layer API is called from benchmarks.

### 12.5 Provided to siblings

Nothing. This feature is a leaf in the dependency graph — it consumes public APIs and produces reports.

### 12.6 New requests to siblings (review required)

These are API/observability asks that this design places on siblings. Each needs acknowledgement in the sibling design's cross-feature section.

- **→ v03-resolution:** expose `ResolutionStats` as a public type reachable from the end of `ingest()` (either via return value or via a `&mut ResolutionStats` parameter). The benchmark needs to read it without SQL introspection. **Status: ✅ acknowledged in v03-resolution §6.4 (r3). Access path is `Memory::ingest_with_stats(content) -> Result<(MemoryId, ResolutionStats), _>`. Field names pinned as public contract.**
- **→ v03-retrieval:** confirm that `FusionConfig::locked()` exists and produces a reproducible, inspectable weight set that can be embedded in the reproducibility record. **Status: ✅ acknowledged in v03-retrieval §5.4 (r3). `FusionConfig::locked()` is the canonical frozen constructor; `FusionConfig` derives `PartialEq` + `Serialize` so benchmarks embed and re-verify exact equality.**
- **→ v03-migration:** expose migration summary counts (records/links/topics, pre/post) as structured output (JSON on `--format=json`). The integrity harness parses this output rather than re-counting via SQL. **Status: ✅ acknowledged in v03-migration §9.4 (r3). Schema version 1.0, stable field names, stdout-only final object in JSON mode.**

---

## 13. Requirements Traceability Table

Every GOAL in `requirements.md` traces to the design section(s) that specify its measurement, threshold, and evaluation.

| GOAL     | Prio | Metric                                         | Threshold              | Design section(s)  |
| -------- | ---- | ---------------------------------------------- | ---------------------- | ------------------ |
| GOAL-5.1 | P0   | `locomo.overall`                               | ≥ 0.685                | §3.1, §4.1, §10.1  |
| GOAL-5.2 | P0   | `locomo.by_category.temporal`                  | ≥ Graphiti baseline    | §3.1, §4.1, §5.3   |
| GOAL-5.3 | P0   | `longmemeval.delta_pp`                         | ≥ +15.0 pp             | §3.2, §4.1, §5.1   |
| GOAL-5.4 | P0   | `cost.average` over N=500                      | ≤ 3.0 calls/episode    | §3.3, §4.1         |
| GOAL-5.5 | P0   | v0.2 tests pass post-migration                 | 100% (exceptions noted) | §3.4, §4.1, §5.2   |
| GOAL-5.6 | P1   | Cognitive-feature directional regression       | 3/3 features pass      | §3.5, §4.2         |
| GOAL-5.7 | P1   | Migration integrity on rustclaw DB, ≥20 queries | 0 lost, all equivalent | §3.6, §4.2         |
| GOAL-5.8 | P2   | Reproducibility record per run                 | present, complete      | §6, §7.3, §4.2a    |

| GUARD     | Severity | Enforcement section(s)  |
| --------- | -------- | ----------------------- |
| GUARD-2   | hard     | §4.4                    |
| GUARD-9   | hard     | §9.4                    |
| GUARD-12  | soft     | §3.3 (shares counter source, separate surface) |

---

**End of design.** Reviewer checklist:

- [ ] §1.4 ownership boundary matches your mental model (this feature is a consumer, not a producer of pipeline behavior).
- [ ] §4 thresholds all match `requirements.md` — no drift.
- [ ] §4.4 override semantics are acceptable (can a release manager *accidentally* ship a P0 failure? should be "no").
- [ ] §5 baseline capture happens before v0.3 implementation starts, not during.
- [ ] §9.3 rustclaw trace anonymization is acceptable to potato.
- [ ] §12.6 three hand-off asks are actionable (each sibling design will add a cross-reference line).
- [ ] §12 Cross-Feature References — each sibling hand-off has ✅ acknowledged status with a pointer to the sibling's ack section.
- [ ] §13 Requirements Traceability Table — every GOAL + GUARD row points to a design section that exists and measures what the row claims.

