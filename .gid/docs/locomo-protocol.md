# LoCoMo Evaluation Protocol

> **Single source of truth** for how we run LoCoMo benchmarks against engram.
> If you're about to run LoCoMo, read this first. If you're inventing a new
> variant, update this doc — don't fork silently.

**Status:** active
**Owners:** potato + rustclaw
**Last updated:** 2026-04-30
**Supersedes:** `PROTOCOL-V2-PLAN.md` (planning doc, now closed)

---

## 0. Index of LoCoMo-related files

Everything that has ever been LoCoMo-shaped. **If you're looking for "where
is the locomo X", start here.**

### Living docs (you should be reading these)

- **`.gid/docs/locomo-protocol.md`** — this file. The fixed protocol.
- **`.gid/docs/locomo-test-log.md`** — append-only run log. One entry per run, newest at top.
- **`.gid/eval-runs/`** — substrate + per-RUN reports.

### Per-RUN artifacts (active)

- **`.gid/eval-runs/PROTOCOL-V2-PLAN.md`** — historical planning doc (closed; kept for context, do **not** edit)
- **`.gid/eval-runs/RUN-0003.md`** … **`RUN-0009-*.md`** — individual run reports (newest = highest number)
- **`.gid/eval-runs/RUN-0005-substrate/`** … **`RUN-0009-substrate/`** — ingested DBs + retrieve scripts per run
- **`.gid/eval-runs/RUN-PROTO-2026-04-30/`** — investigation artifacts (spreading-activation log)

### Driver / dataset

- **Driver (retrieval):** `crates/engramai/examples/locomo_conv26_retrieval.rs`
- **Driver (ingest):** `crates/engramai/examples/locomo_conv26_ingest.rs` + per-run `01_ingest.py`
- **Dataset:** `/Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json`
- **Cogmembench harness:** `/Users/potato/clawd/projects/cogmembench/` (used for J-score adapter, not yet wired)

### Stale / orphan substrates (candidates for cleanup, see §7)

These pre-date the eval-runs/ convention and live next to issues. Substrate
only — no scripts, no reports. Safe to archive once confirmed nothing
references them.

- `.gid/issues/ISS-021/pilot/locomo-conv26-smoke{,-v2,-v3,-haiku{,-before-causal-fix}}.db` (~3.9M, 6 DBs)
- `.gid/issues/ISS-048/locomo-conv26-s1-3-iss048.{db,graph.db}` (~1.1M)
- `.gid/issues/ISS-049/locomo-conv26-s1-3-default-ns.{db,graph.db}` (~1.1M)
- `.gid/issues/ISS-055/locomo-conv26-iss055.{db,graph.db}` (~1.2M)
- `.gid/issues/_smoke-locomo-2026-04-27/` (~2.1M)
- `.gid/issues/_smoke-locomo-2026-04-28/` (~1.1M)

---

## 1. The North Star

**Primary metric:** `hit@5` over LoCoMo categories **1–4** (single-hop, multi-hop, temporal, open-domain).

**Why hit@5 and not J-score (LLM-as-judge):**
- We're benchmarking **retrieval**, not answer generation. Engram's job is to
  surface the right substrate; an external answerer composes the response.
- hit@5 is deterministic, free, runnable in seconds. J-score needs an LLM
  call per question (~$4/run on full conv).
- **Tradeoff:** hit@5 is **not directly comparable** to Mem0 / MemGPT / Letta
  published numbers (those use J-score). For external comparison runs,
  see §3.3 (Tier 3).

**Why exclude category 5 (Adversarial):**
- LoCoMo cat=5 gold answer is `"unanswerable"`. The "right" retrieval result
  is "nothing in substrate is relevant" — `hit@k` semantics break (any hit
  is technically wrong).
- Cat 5 is reported separately as "abstention rate" when relevant.

**Decision lineage:** This was nailed in **RUN-0006 verdict** (2026-04-29).
Don't relitigate without writing a new RUN report that justifies the change.

---

## 2. Standard substrate

**Dataset:** LoCoMo `locomo10.json` conversation #0 (a.k.a. **conv-26**).

**Why conv-26:** This is the "canonical" conversation we've been iterating
on since RUN-0001. Switching conversations resets the comparison baseline.
If you want a multi-conversation run, file a new protocol amendment.

**Ingest namespace convention:**
- `locomo-conv26-{tier}-{tag}` where `tag` is an issue-id or date for
  traceability. Examples: `locomo-conv26-iss068`, `locomo-conv26-full`.
- **Each substrate is immutable.** If a code change requires re-ingest,
  start a new RUN with a new namespace. Never overwrite a substrate that
  has a published RUN report against it.

---

## 3. The three tiers

We run LoCoMo at three different scales for three different purposes.
**Pick the smallest tier that answers your question.**

### 3.1 Tier 1 — Smoke (3 sessions, 25 QAs, 20 cat 1–4)

- **Purpose:** every-PR regression check. "Did I break retrieval?"
- **Wall time:** ~2 min (ingest + retrieve)
- **Substrate name:** `locomo-conv26-{issue-id}` (sessions 1–3 only)
- **Driver invocation:**
  ```sh
  cargo run --release --example locomo_conv26_retrieval -- \
    --db    $SUB/locomo-conv26-{issue-id}.db \
    --graph-db $SUB/locomo-conv26-{issue-id}.graph.db \
    --dataset /Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json \
    --max-session 3 \
    --limit 5 \
    --ns locomo-conv26-{issue-id}
  ```
- **Headline output:** single number `hit@5 (cat 1-4) = X/20 = Y%`
- **Predictive power vs Tier 2:** RUN-0009 confirmed Tier 1 headline tracks
  Tier 2 within ~3pp (50% vs 52.7%, inside noise band). **Reliable for
  regression detection. Not reliable for plan-level analysis.**

### 3.2 Tier 2 — Full (19 sessions, 199 QAs, 150 cat 1–4)

- **Purpose:** issue-close / milestone validation. "Did this fix actually move the needle?"
- **Wall time:** ~30 min (ingest dominates: ~25 min) on M-series Mac mini
- **Substrate name:** `locomo-conv26-full` (single canonical full substrate, re-ingested per build)
- **Driver invocation:** same as Tier 1 but `--max-session 19 --ns locomo-conv26-full`
- **Required outputs in RUN report:**
  - `hit@5 (cat 1-4)` headline
  - Per-category breakdown (cat 1, 2, 3, 4 separately; cat 5 abstention rate)
  - Per-plan breakdown (which retrieval plan answered which questions, coverage holes)
  - Comparison to previous Tier 2 RUN
- **When required:**
  - Closing any P0/P1 retrieval issue
  - Cutting an engram release
  - Before claiming "fixed X"

### 3.3 Tier 3 — External-comparable (Tier 2 + LLM-as-judge)

- **Purpose:** publishing numbers comparable to Mem0 / MemGPT / Letta.
- **Wall time:** Tier 2 + ~20 min for J-score (~$4 in API calls)
- **Status:** **not yet wired.** Needs cogmembench adapter to consume
  engram's retrieval-only output and run answer-generation + J-score.
- **Tracking issue:** see follow-up #3 in `PROTOCOL-V2-PLAN.md`.
- **When to run:** before any external claim ("we beat Mem0 on …"), and
  on every minor release once wired.

---

## 4. Workflow per run

1. **Pick tier** (default Tier 1; escalate per §3 criteria)
2. **Create substrate dir:** `mkdir .gid/eval-runs/RUN-NNNN-substrate`
3. **Copy templates:** start from the latest RUN-NNNN's `01_ingest.py` /
   `02_retrieve.sh`. Only change the namespace and any new flags.
4. **Ingest** (Tier 2/3 only): `python 01_ingest.py | tee ingest.stdout.log`
5. **Retrieve:** `bash 02_retrieve.sh` (already tees to `RUN-NNNN.log`)
6. **Write RUN report:** `.gid/eval-runs/RUN-NNNN.md` (or `RUN-NNNN-{descriptor}.md`)
   - Include: build commit, substrate path, headline, per-category, deltas
     vs previous RUN, anomalies, follow-up issues filed
7. **Append to** `.gid/docs/locomo-test-log.md` (one paragraph entry, newest at top)
8. **Update this protocol doc** if anything in §1–3 changed

**RUN numbering:** monotonically increasing. Don't reuse numbers. If you
abort a run mid-way, leave the number burned and start the next one.

---

## 5. Hard rules / anti-footguns

- **Namespace must match substrate.** Driver default is `default`; if you
  forget `--ns`, you'll get 0/N hits and waste 30 min wondering why.
  (RUN-0006 hit this; documented in its report.)
- **`--limit 5` is the contract.** Changing limit changes the metric. If
  you want to study k=10, file an amendment first.
- **Substrate path lives under `.gid/eval-runs/RUN-NNNN-substrate/`.** Not
  under `.gid/issues/`. If you're tempted to put it under an issue
  directory because "it's just for this issue" — don't. That's how we got
  the 6 orphan smoke DBs in §7.
- **One conversation (conv-26).** Don't silently introduce multi-conv runs.
- **Driver is `locomo_conv26_retrieval.rs`.** If you patch it, commit
  before publishing the RUN report (so the run is reproducible).

---

## 6. What "fixing" something means

A change "fixes" a LoCoMo regression iff:

1. Tier 1 headline does not regress (≥ previous Tier 1, within ±3pp noise)
2. Tier 2 headline strictly improves on the metric the issue identified
3. Per-category breakdown shows movement on the targeted category
4. RUN report explicitly states "fixes ISS-NNN" with diff of pre/post numbers

A change does **not** fix it if Tier 1 looks good but Tier 2 regresses on
a different category. That's a tradeoff, not a fix — file a follow-up.

---

## 7. Cleanup queue

Substrates living outside `.gid/eval-runs/` (see §0). All are non-canonical
and pre-date this protocol. Action items (deferred, run by hand when
potato confirms):

- [ ] Move `ISS-021/pilot/*.db` → `.gid/eval-runs/_archive-ISS-021/` (6 DBs, ~3.9M)
- [ ] Move `ISS-048/049/055/*.db` → `.gid/eval-runs/_archive-issue-substrates/` (3.4M total)
- [ ] Move `_smoke-locomo-2026-04-27/` and `-04-28/` → `.gid/eval-runs/_archive-smoke/` (3.2M)

**Do not delete.** Archive in place; the issue reports cite these paths,
breaking them = breaking history.

---

## 8. Future amendments

When this protocol changes:

1. Bump "Last updated" at top
2. Add an entry to the changelog below
3. Cross-reference any RUN report that motivated the change

### Changelog

- **2026-04-30** — Initial protocol fixed from RUN-0001..0009 history.
  Codifies hit@5 (cat 1–4), three-tier model, conv-26 only, eval-runs/
  layout convention. Identifies cleanup queue.
