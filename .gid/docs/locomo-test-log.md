# LoCoMo Test Log

> **➡️ Protocol lives at [`locomo-protocol.md`](./locomo-protocol.md).** Read that first.
> This file is the append-only run log it references.

> Append-only log of every LoCoMo benchmark / smoke test run.
> One entry per run. Most recent at top.

## Purpose

Centralised record of LoCoMo test runs across issues, so we can:
- Compare runs across commits / configs / prompts
- Reproduce any past run from `commit + command`
- Track movement vs. baselines (v0.2 = 77.9% full LoCoMo, conv-26 hit@5 progression)

## Conventions

Each entry has the same shape. Skip a field with `n/a` rather than omitting it.

```
## RUN-NNNN — {short-title}  ({YYYY-MM-DD HH:MM TZ})

**Issue / context**: ISS-XXX (and any sibling tickets)
**Goal**: one sentence — what this run was supposed to tell us
**Hypothesis**: what we expected to see, and why

### Setup
- **Repo**: engram @ `<commit-sha>` (`<branch>`)
- **Workspace dirty?**: yes/no — list uncommitted changes if yes
- **Dataset**: LoCoMo `<conv-id>`, `<sessions>` sessions, `<queries>` queries
- **Model(s)**:
  - Extractor: `<model + version>`
  - Embedder: `<model + dim>`
  - QA judge: `<model>` (if applicable)
- **DB layout**:
  - main: `<path>`
  - graph: `<path>` (or "same file")
- **Config flags**:  any non-default knobs (namespace, top_k, etc.)

### Method
1. Step-by-step — what was actually run, in order.
2. Include any pre-step (regenerate memories? reuse existing DB?).

### Files (inputs / outputs)
- Source script: `<path>`
- Generated DB(s): `<path>` (size, row counts)
- Logs: `<path>`
- Result artifact: `<path>` (json / csv / md)

### Commands
```sh
# exact commands, copy-pasteable
```

### Output
- **Headline metric**: hit@5 = X/N (Y%), accuracy = Z%, etc.
- **Per-category breakdown** (if applicable): single-hop / multi-hop / temporal / adversarial
- **Latency**: ingest = Xs, retrieval = Yms p50/p95
- **Errors / warnings**: count + sample message

### Observations
- What surprised us
- What confirmed the hypothesis
- What didn't fit and needs follow-up

### Next actions
- [ ] Concrete follow-ups (link to issues / tickets)
```

## Baselines (reference)

For quick comparison without scrolling. Update only when a baseline is re-measured.

- **v0.2 full LoCoMo**: 1986 questions, 77.9% accuracy
  - temporal 86.3% • single-hop 81.x% • multi-hop 70.x% • adversarial 63.9%
  - Source: ISS-019 / earlier benchmark run (date n/a — pre-2026-04-22 monorepo consolidation)
- **conv-26 hit@5 progression** (v0.3 path, 25 queries):
  - 0/25 (0%) — first E2E run, discovered ISS-044 (backfill not wired) — commit `0fe6156` (2026-04-26)
  - 12/25 (48%) — ISS-049 Phase 4 acceptance after ISS-044 fix — commit `7a3f27a` (2026-04-27)
  - 14/25 (56%) — post-ISS-058 split-brain fix + post-ISS-063 fallback contract; 3 Factual misses traced to ingest-gap (D1:12, D2:15, D3:16), 2 to retrieval-rank — commits `986ca65` → RUN-0002/0003/0004 (2026-04-28..29)
  - **expected next**: post-ISS-068 ingest fix (`6f5821a`) — RUN-0005 confirmed ingest gap closed; full retrieval re-run pending (RUN-0006)

## Runs

<!-- Add new runs ABOVE this line. Most recent first. -->

## RUN-0006 — Per-LoCoMo-category breakdown over RUN-0005 substrate (2026-04-29 14:25 -04:00)

**Issue / context**: PROTOCOL-V2-PLAN follow-up. RUN-0005's headline `12/25 = 48%` was deemed too coarse — LoCoMo's 5 question categories have very different shapes (cat=5 Adversarial gold is "unanswerable", so retrieval hit is meaningless). Added per-LoCoMo-category instrumentation to `crates/engramai/examples/locomo_conv26_retrieval.rs` (uncommitted at run time) and re-ran retrieval over byte-identical RUN-0005 substrate.

**Goal**: (1) reproduce 12/25 as sanity check; (2) get per-category cohort metrics; (3) get a "cleaner" headline that excludes Adversarial noise.

**Build / commit**: `925800b` clean tree + uncommitted diff to `locomo_conv26_retrieval.rs` (per-category counters + cleaner output).

**Substrate**: `.gid/eval-runs/RUN-0006-substrate/` — `cp` of `RUN-0005-substrate/locomo-conv26-iss068.{db,graph.db}` (md5 verified identical). No re-ingest.

**Method**:

```sh
ENGRAM=/Users/potato/clawd/projects/engram
SUB=$ENGRAM/.gid/eval-runs/RUN-0006-substrate
cp $ENGRAM/.gid/eval-runs/RUN-0005-substrate/locomo-conv26-iss068.db*       $SUB/
cp $ENGRAM/.gid/eval-runs/RUN-0005-substrate/locomo-conv26-iss068.graph.db* $SUB/
bash $SUB/01_retrieve.sh   # tee'd to $SUB/RUN-0006.log
```

(Note: namespace must be `locomo-conv26-iss068`, NOT `conv26`. First attempt this run mis-passed `--ns conv26` and got 0/25 — wrote it down so future-me doesn't repeat.)

**Result**:

- **Headline (all 25)**: hit@5 = **12/25 = 48.0%** — bit-identical to RUN-0005 ✅ (sanity check passes; retrieval is deterministic on this substrate)
- **Cleaner headline (cat 1-4 only)**: **10/20 = 50.0%** — proposed new north-star
- Empty result sets: 2/25 (q13, q21 — both Hybrid; same as RUN-0005)

**Per-LoCoMo-category breakdown (NEW)**:

```
cat=1 Multi-hop   n=3  hits=0  ( 0.0%)    ← broken
cat=2 Temporal    n=7  hits=6  (85.7%)    ← strongest cohort
cat=3 Open-ended  n=1  hits=1  (100.0%)   ← n too small
cat=4 Single-hop  n=9  hits=3  (33.3%)    ← weakest answerable
cat=5 Adversarial n=5  hits=2  (40.0%)    ← noise — gold = "unanswerable"
```

**Per-plan breakdown (unchanged from RUN-0005)**:

```
Factual   n=17  hits=11  (64.7%)  empty=0
Abstract  n=4   hits=1   (25.0%)  empty=0
Affective n=2   hits=0   ( 0.0%)  empty=0
Hybrid    n=2   hits=0   ( 0.0%)  empty=2
```

**Findings**:

1. **Multi-hop dispatcher is structurally broken (0/3, cat=1)**. None of the misses are admission gaps — gold turns are present. The plan dispatcher selects Factual/Abstract for multi-hop queries and falls through to single-shot retrieval. No real traversal is wired. → file new ISS.
2. **Temporal is the high-water mark (85.7%, cat=2)**. The single miss (q2 D1:12) is the same lexical+semantic gap RUN-0005 already flagged — known issue.
3. **Single-hop underperforms (33.3%, cat=4)**. Surprising — should be the easy cohort. Misses split into 3 ranking misses (gold in candidates but outranked), 1 Affective `outcome=no_cognitive_state` (q18 — Affective plan is shipping a no-op), 1 empty Hybrid, 1 Abstract downgrade. → Affective `no_cognitive_state` is a structural bug, file new ISS.
4. **Cat=5 Adversarial (40%) is meaningless as a retrieval metric** — gold label is "abstain". Going forward, **report `hit@5 (cat 1-4)` as the primary number**; track Adversarial separately as "abstain-correctness" once an answering layer exists.
5. **Hybrid plan has 100% empty rate (2/2)**. ISS-063 fallback contract working correctly (no candidates → empty, not crash) but Hybrid is either over-selected by the planner or its executor is not wired to FTS/vector backends. Worth digging.

**Comparison to RUN-0005**:

| Metric          | RUN-0005 | RUN-0006 |  Δ  |
|-----------------|---------:|---------:|----:|
| hit@5 (all 25)  |   12     |   12     |  0  |
| empty results   |    2     |    2     |  0  |
| Factual hits    |   11     |   11     |  0  |

Identical — RUN-0006's value is **decomposition**, not score movement.

**Next actions**:

- File ISS for Multi-hop dispatcher (no traversal — falls through to single-shot)
- File ISS for Affective plan `no_cognitive_state` no-op
- Update PROTOCOL-V2-PLAN: adopt `hit@5 (cat 1-4) = 10/20 = 50.0%` as the primary north-star
- Commit the per-category instrumentation diff in `crates/engramai/examples/locomo_conv26_retrieval.rs` so future runs use it by default

**Artifacts**:

- `.gid/eval-runs/RUN-0006.md` — full run doc
- `.gid/eval-runs/RUN-0006-substrate/RUN-0006.log` — full retrieval log (25 queries)
- `.gid/eval-runs/RUN-0006-substrate/01_retrieve.sh` — reproduction script

---

## RUN-0005 — ISS-068 fix verification smoke: raw memory persists when extractor returns 0 facts (2026-04-29 11:51 -04:00)

**Issue / context**: ISS-068 (resolved by commit `6f5821a`); follow-on to RUN-0004 ingest-gap finding
**Goal**: Verify that the split-persistence fix admits every dialogue turn into FTS/embedding indices, even when the LLM fact extractor returns zero facts, and that the previously-unreachable gold evidence turn D1:12 now lands.
**Hypothesis**: Distinct dia_id count for conv-26 session_1 should jump from 7/18 (pre-fix, RUN-0004 substrate) to 18/18; turns where extractor returns 0 facts should be logged in `graph_extraction_failures` with `error_category = no_facts_extracted` instead of being silently dropped; CLI should return a real UUID for every call instead of `skipped:<hash>`.

### Setup
- **Repo**: engram @ `925800b` (`main`) — fix commit `6f5821a` ("fix(ingestion): persist raw memory when extractor returns no facts")
- **Dataset**: LoCoMo conv-26 **session_1 only** (18 turns D1:1..D1:18) — minimum-cost ingest verification, no retrieval scoring
- **Command**: `python3 .gid/issues/ISS-068/smoke_verify.py` (drives CLI ingest 18 times)
- **Substrate DB**: `.gid/issues/ISS-068/_smoke_2026-04-29/verify.db` (memories, FTS, embeddings)
- **Graph DB**: `.gid/issues/ISS-068/_smoke_2026-04-29/verify.graph.db` (graph_extraction_failures + entity tables — split-brain layout post-ISS-058)
- **Namespace**: `iss068-verify-conv26-s1`
- **Logs**: `.gid/issues/ISS-068/_smoke_2026-04-29/verify.log` (one block per turn with stdout = UUID prefix)

### Result
- **Ingest admission**: 18/18 D1 turns admitted — every turn returned a distinct 8-char UUID prefix (D1:1 → `da4decba`, …, D1:12 → `f0f15544`, …, D1:18 → `53ff2311`). Pre-fix this same path produced 7 distinct dia_ids. **No more `skipped:<hash>` returns.**
- **Substrate `memories` rows** (namespace = `iss068-verify-conv26-s1`): **19** (18 turns + 1 bootstrap row).
- **`graph_extraction_failures`** (in graph DB): **10 rows** with `error_category = no_facts_extracted` — i.e. 10 of the 18 turns hit the "extractor returned 0 facts" branch but their raw content still landed in FTS/embeddings, and the failure is now observable instead of silently swallowed.
- **Gold evidence D1:12**: present (UUID `f0f15544`) — previously unreachable.

This is **ingest-only** verification. No retrieval / hit@5 numbers were measured in this run.

### Conclusions
- ISS-068 root fix confirmed end-to-end on the real ingest path: zero-fact turns are now stored as raw memory + a `graph_extraction_failures` row, instead of being short-circuited to `RawStoreOutcome::Skipped`.
- The recall ceiling on the 3 ingest-gap Factual misses identified in RUN-0004 (q2 D1:12, q19 D2:15, q20 D3:16) is structurally lifted — the underlying turns are now reachable by FTS / embedding retrieval. Whether retrieval actually surfaces them is the next test.
- The remaining 2 RUN-0004 Factual misses (q5 D1:5, q8 D2:14) are retrieval-side semantic gap, untouched by this fix.

### Next actions
- [ ] **RUN-0006**: full conv-26 sessions 1-3 (50 turns) end-to-end with hit@5 scoring on 25 queries — confirm hit@5 ceiling lifts above RUN-0004's 14/25 (56%) baseline. Expected: 3 Factual misses (q2/q19/q20) move from "structurally unreachable" to "retrievable" — actual recovery depends on retrieval ranking.
- [ ] If RUN-0006 still misses q2/q19/q20 despite turns now being indexed, file a retrieval-side ticket (FTS/embedding rank gap, not ingest gap).

## RUN-0004 — Phase B2 dig: 5 Factual misses split 3 ingest-gap + 2 retrieval-rank (2026-04-29 05:53 -04:00)

**Headline:** hit@5 = 14/25 (56.0%), bit-identical to RUN-0003 → stagnation criterion met (Δ < 0.001). Per-plan: Abstract 2/4, Affective 0/2, Factual 12/17, Hybrid 0/2 (same as RUN-0003). Same substrate, same commit (`ac34db9` dirty), but `--ns locomo-conv26-iss058` (the actual ingest namespace) instead of `--ns conv26` — RUN-0002/0003 used the wrong flag and got data only because pre-ISS-064 namespace scoping was leaky. ISS-064 fast-fail (workspace edit, uncommitted) now correctly surfaces this.

**Five Whys on the 5 Factual misses:** 3 of 5 (q2 D1:12, q19 D2:15, q20 D3:16) — gold dia_id is **not in the substrate at all**, dropped silently during ingestion (quarantine table empty, ~22 of 50 conv-26 s1-3 turns missing). 2 of 5 (q5 D1:5, q8 D2:14) — gold memory present but query embeddings don't bridge "identity"↔"transgender" / "relationship status"↔"single parent". **Filed ISS-068** (P1) for the ingestion-gap; deferred query-rewrite work for the rank-loss two.

Full doc: `.gid/eval-runs/RUN-0004.md` · log: `.gid/issues/_smoke-locomo-2026-04-28/RUN-0004.log` · issue: `.gid/issues/ISS-068/issue.md`

---

## RUN-0003 — repro RUN-0002 + confirm Hybrid sub-plans genuinely empty (2026-04-29 05:46 -04:00)

**Headline:** hit@5 = 14/25 (56%) — bit-identical to RUN-0002. Reproduces post-ISS-063 baseline; confirms Hybrid `empty_result_set` is genuine sub-plan-empty (Episodic `DowngradedFromEpisodic` + Abstract `DowngradedL5Unavailable`), not a `hybrid_to_scored` bug. **Filed ISS-067** to decide fallback contract (Option A flag vs upstream Episodic+Abstract fixes). Workspace dirty (issue.md + 4 src files) but no behavior change.

Full doc: `.gid/eval-runs/RUN-0003.md` · log: `.gid/issues/_smoke-locomo-2026-04-28/RUN-0003.log` · issue: `.gid/issues/ISS-067/issue.md`

---

## RUN-0002 — post-ISS-063 fallback contract, conv-26 s1-3 (2026-04-28 22:00 -04:00)

**Issue / context**: ISS-063 (downgrade-to-fallback contract). Sibling: ISS-061 (resolved-by), ISS-060 (superseded-by), ISS-064 (filed from this run).
**Goal**: Verify ISS-063 fix on real LoCoMo data — every non-Factual primary plan must run Associative fallback and surface an outcome label per design §3.4 / §6.4.
**Hypothesis**: With the contract in place, RUN-0001's "0 / 25 hit@5 silent ok" should become either real hits via fallback or terminal `EmptyResultSet`.

### Setup
- **Repo**: engram @ `602ed91` (post 35435b9 ISS-063 impl + closeout)
- **Workspace dirty?**: no
- **Dataset**: LoCoMo `conv-26`, sessions 1-3, 25 queries (same as RUN-0001)
- **Substrate**: reused RUN-0001's `.gid/issues/_smoke-locomo-2026-04-28/locomo-conv26-s1-3-iss058.{db,graph.db}` (no re-ingest)
- **Driver**: `crates/engramai/examples/locomo_conv26_retrieval.rs`
- **Namespace**: `locomo-conv26-iss058` (corrected — see "Discovered" below)

### Headline results

- **Hits @ 5: 14 / 25 (56.0%)** — up from **0 / 25 (0.0%)** in RUN-0001
- **Empty results: 2 / 25 (8.0%)** — terminal `EmptyResultSet { reason="hybrid_all_subplans_empty" }`, no longer silent

| Plan       | n  | Hits | Empty | Outcome distribution                            |
|------------|----|------|-------|--------------------------------------------------|
| Factual    | 17 | 12   | 0     | ok ×17                                           |
| Abstract   | 4  | 2    | 0     | downgraded_from_abstract ×4 (each ran fallback)  |
| Hybrid     | 2  | 0    | 2     | empty_result_set ×2                              |
| Affective  | 2  | 0    | 0     | no_cognitive_state ×2 (label preserved per §6.4) |

### What ISS-063 proved

1. **Abstract→Associative fallback delivers candidates.** Pre-fix: `ok candidates=0`. Post-fix: 4 queries each return 5 candidates from the fallback path; 2 land hits @5.
2. **Hybrid empties are observable.** Now `outcome=empty_result_set reason="hybrid_all_subplans_empty"` instead of silent `ok candidates=0`.
3. **Affective preserves `no_cognitive_state`.** §6.4 surface label kept even though fallback ran.

### Two compounding causes for the 0/25 → 14/25 jump

The headline number includes a **second, independent fix** discovered mid-run (see ISS-064):

- **Cause A (ISS-063 fix):** the fallback contract itself — without it, no plan except Factual could surface candidates.
- **Cause B (namespace mismatch):** RUN-0001's driver invocation used `--ns conv26`, but the substrate ingest stored everything under `--ns locomo-conv26-iss058`. Retrieval silently filtered against a non-existent namespace and returned empty without warning. Fixed by passing the correct namespace.

Both causes had to be fixed for hits to land. ISS-063 alone wouldn't have moved the number — but the silent namespace mismatch is itself a serious observability bug (filed as ISS-064).

### Substrate health (re-checked, unchanged from RUN-0001)

- `memories`: 31 (all in `locomo-conv26-iss058`)
- `graph_entities`: 137 / `graph_edges`: 101
- `graph_extraction_failures`: 0
- `knowledge_topics`: 0 (L5 not built — explains all 4 `downgraded_from_abstract`)

### Findings

- **ISS-063 confirmed resolved** by behavior on real data, not just by unit tests.
- **ISS-061** ("Hybrid 0 despite outcome=ok") symptom no longer reproduces — the new `EmptyResultSet { reason }` makes the same situation explicit. Closed as resolved-by-ISS-063.
- **ISS-060** ("Abstract chain returns 0") — same root cause as ISS-061 (silent ok-empty), now superseded by ISS-063's contract.
- **ISS-064 filed**: namespace mismatch is silently swallowed by retrieval. Driver/orchestrator should fail-fast or warn when `--ns` matches zero entities. This run almost gave a false negative on ISS-063.

### Artifacts

- Driver log: `.gid/issues/_smoke-locomo-2026-04-28/RUN-0002.log`
- Detailed report: `.gid/issues/_smoke-locomo-2026-04-28/RUN-0002.md`
- Substrate (reused): `.gid/issues/_smoke-locomo-2026-04-28/locomo-conv26-s1-3-iss058.{db,graph.db}`

### Next

- **ISS-064**: implement namespace-mismatch fail-fast / warn.
- **L5 substrate gap** (4 `downgraded_from_abstract`): build `knowledge_topics` over the 31 memories so Abstract plan succeeds natively. Tracked under ISS-060's broader "Abstract path needs L5" angle (see follow-up).
- Expand to more LoCoMo conversations once ISS-064 lands so we don't waste cycles on silent empty namespaces.

---

## RUN-0001 — post-ISS-058 conv-26 sessions 1-3 smoke (2026-04-28 21:09 -04:00)

**Issue / context**: ISS-058 (split-brain ingest fix, commit `986ca65`); compare against ISS-049 12/25 acceptance (`7a3f27a`).
**Goal**: Measure whether ISS-058's ingest fix (graph rows now route to `--graph-db` instead of main DB) changes retrieval score on conv-26 s1-3.
**Hypothesis**: hit@5 stays at 12/25 or slightly improves. ISS-058 is a correctness fix on the ingest write path; retrieval at 12/25 came from Factual plans that don't depend on graph correctness, so a flat result would *confirm* the bug was contained to the graph-side substrate.

### Setup
- **Repo**: engram @ `986ca65` (`main`)
- **Workspace dirty?**: yes — ISS-063 retrieval diagnostics stashed (`stash@{0}`); src tree clean for build
- **Dataset**: LoCoMo conv-26 (`/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json`), sessions 1-3, 25 queries (filtered by evidence session ≤ 3)
- **Model(s)**:
  - Extractor: `anthropic` (default — Sonnet via OAuth)
  - Embedder: default (model+dim n/a — not surfaced in log; check engramai default)
  - QA judge: n/a (hit@5 only, no LLM-graded accuracy this run)
- **DB layout**:
  - main: `.gid/issues/_smoke-locomo-2026-04-28/locomo-conv26-s1-3-iss058.db` (626 KB)
  - graph: `.gid/issues/_smoke-locomo-2026-04-28/locomo-conv26-s1-3-iss058.graph.db` (separate file)
- **Config flags**: `--ns locomo-conv26-iss058`, `--max-session 3`, `--limit 5`, `--graph-drain-timeout-secs 120`

### Method
1. Stash dirty diagnostics (`git stash push crates/engramai/src/retrieval/api.rs orchestrator.rs`)
2. `cargo build --release -p engram-cli` and `-p engramai --example locomo_conv26_retrieval`
3. Fresh ingest: run `01_ingest_iss058.py`, which clears any prior `*-iss058.db{,.graph.db}` and re-ingests sessions 1-3 turn by turn via `engram store --graph-db ...`
4. Verify ISS-058 fix: count graph_* rows in main DB (should be 0) vs graph DB (should be non-zero)
5. Run retrieval driver against the freshly-ingested DBs, dump full log

### Files (inputs / outputs)
- Source script: `.gid/issues/_smoke-locomo-2026-04-28/01_ingest_iss058.py`
- Retrieval driver: `crates/engramai/examples/locomo_conv26_retrieval.rs`
- Generated DBs:
  - `locomo-conv26-s1-3-iss058.db` — 58 episodic memories
  - `locomo-conv26-s1-3-iss058.graph.db` — 137 entities, 101 edges, 6 predicates, 31 pipeline_runs, 31 applied_deltas, 137 mentions, 0 extraction_failures
- Logs: `ingest_run.log`, `ingest_iss058.log`, `retrieval-run.log` (115 lines)

### Commands
```sh
# 1. Stash diagnostics
git stash push -m "ISS-063 diagnostics (pre RUN-0001 stash)" \
  crates/engramai/src/retrieval/api.rs crates/engramai/src/retrieval/orchestrator.rs

# 2. Build
cargo build --release -p engram-cli
cargo build --release -p engramai --example locomo_conv26_retrieval

# 3. Ingest (Python wrapper drives `engram store` per turn)
python3 .gid/issues/_smoke-locomo-2026-04-28/01_ingest_iss058.py

# 4. Verify split-brain fix (counts in main vs graph DB)
sqlite3 ...iss058.db "SELECT COUNT(*) FROM graph_entities;"          # → 0
sqlite3 ...iss058.graph.db "SELECT COUNT(*) FROM graph_entities;"   # → 137

# 5. Retrieval
cargo run --release -p engramai --example locomo_conv26_retrieval --quiet -- \
  --db .gid/issues/_smoke-locomo-2026-04-28/locomo-conv26-s1-3-iss058.db \
  --graph-db .gid/issues/_smoke-locomo-2026-04-28/locomo-conv26-s1-3-iss058.graph.db \
  --dataset /Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json \
  --ns locomo-conv26-iss058 \
  --max-session 3 \
  --limit 5
```

### Output
- **Headline metric**: **hit@5 = 12/25 (48.0%)** — flat vs ISS-049 baseline (12/25)
- **Empty results**: 8/25 (32.0%)
- **Per-plan breakdown** (post-execution `plan_used`):
  - Factual: 12/17 hits (70.6%), 0 empty — carries the entire score
  - Abstract: 0/4 hits, 4 empty (all `downgraded_from_abstract`)
  - Affective: 0/2 hits, 2 empty (all `no_cognitive_state`)
  - Hybrid: 0/2 hits, 2 empty (sub-plans return 0 items)
- **Per-outcome**: ok=19, downgraded_from_abstract=4, no_cognitive_state=2
- **Latency**:
  - Ingest: 165.7s for 58 turns (2.86s/turn) — dominated by Anthropic extractor
  - Retrieval: 25 queries finished sub-second total (single timestamp `01:09:55Z` covers most queries; not separately benchmarked)
- **Errors / warnings**: none — 0 extraction_failures, all 31 pipeline runs applied a delta

### Observations
- ✅ **ISS-058 fix verified end-to-end**: main DB has 0 rows in every graph_* table; graph DB holds all 137 entities + 101 edges. Schema exists in both DBs (engramai bootstraps both), but writes now go to the right place. No double-write.
- ✅ **31 pipeline runs ↔ 31 applied deltas ↔ 0 failures** — ingest pipeline is healthy. (Note: 31 runs for 58 turns suggests batching or short-text skip; worth checking what makes 27 turns produce no run.)
- ⚠️ **Score is flat at 12/25** — confirms the hypothesis. ISS-058's split-brain bug *did not* depress conv-26 retrieval, because Factual plans (the only plans currently scoring hits) don't traverse the graph substrate that was being written to the wrong DB.
- 🔴 **The real 48% ceiling is plan coverage, not graph correctness.** Three plan types contribute zero hits across 8 queries:
  - **Abstract (4 queries)** all downgrade — `downgraded_from_abstract` outcome means the abstract executor returned no candidates, fell back to factual (which then also returned 0 here). This is likely ISS-021 territory: extractor isn't producing the higher-level summaries Abstract retrieval indexes.
  - **Affective (2 queries)** hit `no_cognitive_state` — emotion/cognitive-state substrate isn't populated. Separate gap from Abstract.
  - **Hybrid (2 queries)** dispatches both Episodic + Abstract sub-plans, both return 0 items. Same root cause as Abstract for the Abstract leg.
- 🟡 27 turns out of 58 have no pipeline_run — need to confirm whether engramai filters short/non-extractable turns or whether some runs are batched. Not blocking, but worth a follow-up.

### Next actions
- [ ] **ISS-021 spike** — modify extractor prompt to emit Abstract-tier extractions (summaries / generalizations). Expected impact: lifts Abstract n=4 + Hybrid n=2 from 0 hits → ?  Could push hit@5 from 12/25 toward 18/25 if half land.
- [ ] Investigate Affective plan substrate (separate ticket) — `no_cognitive_state` means the cognitive-state readback is empty. Different fix from Abstract.
- [ ] File follow-up: 27 turns with no pipeline_run — is this batching or silent skip?
- [ ] After ISS-021 prompt change → re-ingest with same script (rename to `01_ingest_iss021spike.py`) and re-run retrieval as **RUN-0002**.
- [ ] `git stash pop` ISS-063 diagnostics back into working tree.

---

*This log is the source of truth for "what did we last measure?". Anything not in here didn't happen.*
