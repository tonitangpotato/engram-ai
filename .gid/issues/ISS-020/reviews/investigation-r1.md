# Review of ISS-020 Investigation — r1

**Reviewer:** RustClaw (main agent)
**Date:** 2026-04-22
**Target:** `.gid/issues/ISS-020-kc-dimensional-awareness/investigation.md`
**Depth:** full (investigation/audit with embedded work plan)
**Doc size:** 649 lines, 7 sections, 3 phases (P0/P1/P2)

## Summary

- Critical: 1
- Important: 4
- Minor: 4
- Total: 9 findings

Overall: solid audit. The terminology separation in §1 is excellent and does
the work of preventing the "KC reads metadata" confusion that motivated the
split. The dimensional → KC-stage mapping in §3 is the core contribution and
is largely correct. Phase plan is well-ordered and independently shippable.

Main problems are **factual inaccuracies in §3 and §4.2** where the doc claims
`valence` and `confidence` are persisted to `memory.metadata` — they are not.
`fact.valence` is only captured into the transient `last_extraction_emotions`
cache; `fact.confidence` is never persisted at all. Any P0 plan that depends
on reading them will fail silently.

---

## FINDING-1 ✅ Applied (2026-04-22) — Critical — `valence` and `confidence` are NOT persisted to `metadata`

**Applied changes:** §1.1 now includes a subsection noting `valence`/`confidence` are extractor-output-not-persisted with pointer to `last_extraction_emotions` caveat. §3 table rows for `valence`/`confidence` annotated `⚠️ Not persisted today — see §1.1 and §5 P0.0`. §3.1 moved `confidence` to P1 with blocked-on-P0.0 note; `valence` in P2 annotated likewise. §4.1 adds a `⚠️ Persistence precondition` callout. §4.2 rewrites the stale `last_extraction_emotions` paragraph. §5 adds step P0.0 (extend write path to persist `valence`/`confidence`). §5 summary table and P0.4 updated to reflect the dependency. §5 dependency graph shows P0.0 as independent-parallel.


**Section:** §1.1, §3 (table rows for `valence` and `confidence`), §4.1, §4.2

**Claim under review:** §1.1 states dimensional metadata location is
`memory.metadata.dimensions.{..., sentiment, stance}` plus
`memory.metadata.type_weights`. §3's table lists `valence` (Continuous) and
`confidence` (Categorical 3-level) as dimensional fields KC should read.
§4.1's fix shape adds `confidence: Option<Confidence>` and
`valence: Option<f64>` to `MemorySnapshot`. §4.2 says "Also ignores the
valence / confidence that extractor cached (`last_extraction_emotions` in
`memory.rs`)".

**Ground truth from code** (`src/memory.rs:1320-1345`):

The extractor DOES populate `fact.valence` and `fact.confidence` on the
`ExtractedFact` struct. But the write path to `dim_metadata` only writes
10 Option fields (`participants, temporal, location, context, causation,
outcome, method, relations, sentiment, stance`) plus `type_weights`. It
does NOT write `valence` or `confidence` into the metadata JSON.

`fact.valence` is separately captured into `self.last_extraction_emotions`
(a `Mutex<Option<Vec<(f64, String)>>>`) — but that's a **transient
per-extraction-call buffer**, not persisted to any memory record. A caller
must immediately read it via `take_last_extraction_emotions()` before the
next `remember()` call clobbers it. It is not attached to specific memory IDs.

**Impact:**
- §4.2's fix shape won't work as written: `metadata.dimensions.valence` and
  `metadata.dimensions.confidence` don't exist. Parsing them yields `None`
  forever.
- §4.1's `valence: Option<f64>` and `confidence: Option<Confidence>` fields
  on `MemorySnapshot` will always be `None` under the current write path.
- §3's priority assignment of `confidence` to P1 ("uncertainty propagation")
  is based on a field that doesn't reach storage. Real P0 work item is
  getting these two fields WRITTEN before KC can read them.
- §7.3 mentions ISS-019 as a separate write-path concern, but this isn't
  the same issue — ISS-019 is about drop rate on existing fields;
  `valence`/`confidence` are fields that are never written at all, by
  design or oversight.

**Suggested fix:**

1. In §1.1, correct the location inventory: `valence` and `confidence` are
   produced by the extractor but not currently persisted. Mark them as
   "extractor output, not currently in `memory.metadata`".
2. In §3's table, add a column or footnote distinguishing "persisted today"
   vs "produced by extractor but not persisted". Move `valence` and
   `confidence` rows to a subsection noting they require a write-path fix
   first.
3. In §4.1/§4.2, mark `valence` and `confidence` support as **blocked on a
   new precondition** (file an ISS-021 or expand ISS-019): extractor write
   path must be extended to add `valence: f64` and `confidence: String` to
   the persisted `dimensions` object.
4. In §5 Phase P0, move `confidence` propagation (§4.3's `conf={confidence}`
   in the enriched prompt line) out of P0 scope — it's blocked on write-path
   fix. Keep `causation`, `outcome`, `stance`, `domain`, `participants`
   which ARE persisted.
5. Alternative if we want to preserve P0 scope: add P0.0 "extend
   memory.rs:1320-1345 to persist `valence` and `confidence` alongside the
   other dimensional fields". This is a ~10-line change in the extractor
   write branch and low risk.

This is the highest-leverage correction because it affects what fields the
compiler-side types can actually carry, and therefore the shape of `Dimensions`
struct in §4.1.

---

## FINDING-2 ✅ Applied (2026-04-22) — Important — `TypeWeights` has 7 fields, not 5

**Applied changes:** §3 `type_weights` row changed from "Structured (5 floats)" to "Structured (7 floats: factual, episodic, procedural, relational, emotional, opinion, causal)".


**Section:** §3 table row for `type_weights`

**Claim:** "Structured (5 floats)"

**Ground truth** (`src/type_weights.rs:15-27`):
```rust
pub struct TypeWeights {
    pub factual: f64,
    pub episodic: f64,
    pub procedural: f64,
    pub relational: f64,
    pub emotional: f64,
    pub opinion: f64,
    pub causal: f64,
}
```

Seven fields, corresponding to the seven `MemoryType` variants.

**Impact:** Minor to the overall plan, but an audit doc that misstates
basic cardinality loses credibility. Also affects §3.1's P1 item
"`type_weights` → discovery + quality scoring" — any prompt-style
synthesis uses the type labels, and five vs seven matters.

**Suggested fix:** Change "Structured (5 floats)" → "Structured (7 floats:
factual, episodic, procedural, relational, emotional, opinion, causal)".

---

## FINDING-3 ✅ Applied (2026-04-22) — Important — `last_extraction_emotions` is global transient state, not per-record

**Applied changes:** §4.2 misleading reference removed; replaced with accurate note explaining the cache is global, per-call, and unusable by KC — absorbed into the FINDING-1 callout.


**Section:** §4.2 ("Also ignores the valence / confidence that extractor
cached (`last_extraction_emotions` in `memory.rs`)")

**Claim under review:** implies KC could read the cache to recover
`valence`/`confidence`.

**Ground truth:** `last_extraction_emotions` is a single
`Mutex<Option<Vec<(f64, String)>>>` on the `Memory` struct, overwritten on
every `remember()` call. It only contains `(valence, domain)` pairs for the
**most recent extraction** — not a per-memory-ID map. By the time KC runs
(batch-scanning thousands of memories from SQLite), the cache holds data
for whatever the last `remember()` call extracted, which is meaningless to
KC. Also: `confidence` is NOT in the cache at all (only valence + domain).

**Impact:** The §4.2 sentence is misleading — it implies KC has an
alternative path to those fields. It doesn't. Reading
`last_extraction_emotions` from KC would produce wrong data.

**Suggested fix:** Remove the `last_extraction_emotions` reference from
§4.2, or rewrite as: "Note: `valence` is captured into a transient
per-call cache (`last_extraction_emotions` in `memory.rs`) that KC cannot
consume — it's a hand-off mechanism for the immediate caller of
`remember()`, not a persistent field. KC must receive `valence` via
persisted metadata if at all."

---

## FINDING-4 ✅ Applied (2026-04-22) — Important — §4.2 omits a separate latent bug: `updated_at = m.created_at`

**Applied changes:** §4.2 adds a sub-note documenting the latent bug. **Verified via source:** `MemoryRecord` at `src/types.rs:162` has no `updated_at` field, only `created_at` and `access_times`. Sub-note proposes two fixes: (a) add `updated_at` to `MemoryRecord` (separate issue), or (b) stopgap — derive `updated_at` from `access_times.last()`. Picked (b) for the closure change; (a) deferred to dedicated ticket.


**Section:** §4.2 (analysis of `main.rs:2303-2320`)

**Ground truth:** The conversion closure sets `updated_at: m.created_at`.
This means every `MemorySnapshot` has `updated_at == created_at`
regardless of actual mutations. §7.1 and §4.6 both assume `updated_at` is
a meaningful freshness signal, but it is always equal to `created_at` in
the current pipeline.

**Impact:** Independent of dimensional work, but in the same closure and
should be mentioned because:
- §4.6 says "Quality scoring ... coverage, freshness, etc. — all derived
  from MemorySnapshot fields". Freshness is actually a no-op right now
  since `updated_at` is a copy of `created_at`.
- Any P1 clustering improvement that uses `updated_at` (doesn't currently,
  but §P1.2 mentions temporal) would be operating on stale data.

**Suggested fix:** Add a §4.2 sub-note: "Independent issue: the closure
also sets `updated_at: m.created_at`, which should be `m.updated_at` from
`MemoryRecord`. Fix alongside dimensional plumbing since both touch the
same closure." If `MemoryRecord` doesn't have `updated_at`, note that as
a further gap. Verify before committing this finding.

---

## FINDING-5 ✅ Applied (2026-04-22) — Important — §4.3 enriched prompt line has unquantified length cost

**Applied changes:** §4.3 adds a quantified token-budget table (current vs enriched-verbose vs enriched-compact × 50/200 memory topics), concrete mitigations (compact-line default, `prompt_detail_level` config, projected-token budget enforcement with truncation on overflow).


**Section:** §4.3

**Claim:** "Risk: Medium — prompt changes affect output quality. Requires
A/B test on a snapshot of real memories to validate topic quality doesn't
regress."

**Gap:** Token budget is only mentioned in §6 Q7 as "2-3x token count per
memory line. For a topic with 50 source memories, that's meaningful
context growth." But there's no concrete budget analysis.

If today's line is `- [factual] (2026-04-22): {content}`, and `content` is
typically ~200 tokens, then per-line tokens are ~210. Enriched version
with six additional fields averaging 30 tokens each → ~390 tokens/line, i.e.
~1.85x not 2-3x. For a 50-memory topic: 10.5k tokens → 19.5k tokens. That's
~9k token growth per topic, which at Opus pricing ($15/M input) is ~$0.14
per extra compile. Probably fine.

But for large topics (200+ memories as seen in real usage), 390 × 200 =
78k tokens — close to Claude's 200k context ceiling with system prompt
overhead. This could silently cap topic size.

**Suggested fix:** In §4.3 or §7, add:
- Explicit per-memory prompt token estimate (today vs enriched).
- Max topic size before context overflow (calculated, not guessed).
- Recommended mitigation (truncate low-importance memories first;
  configurable `prompt_detail_level` minimum/standard/full as already
  suggested in Q7).

Also: §4.3's template puts `participants`, `caused_by`, `outcome`, `stance`
each on their own indented line. That's 5 extra newlines per memory.
Consider a one-line compact form: `- [type|domain|conf] (date) participants
| cause: X | outcome: Y | stance: Z: content` — halves the overhead when
most fields are populated.

---

## FINDING-6 ✅ Applied (2026-04-22) — Minor — §4.5(a) domain pre-filter has an edge case

**Applied changes:** §4.5 adds a `general`-domain caveat noting that pre-filter degenerates to full-graph clustering within the catch-all bucket, and that (b) augmented edge weights is the follow-up for improving `general` specifically. Cross-domain edge question noted as deferred with a provisional 5% cap.


**Section:** §4.5 option (a) — cluster within each domain separately

**Gap:** Memories whose `domain` is `"general"` (`default_domain()` per
`extractor.rs`) will be siloed into a single "general" cluster, which is
usually the largest bucket. The pre-filter does nothing for that cluster
since it's back to the original problem.

**Suggested fix:** In §4.5, add: "Caveat: default domain `general` acts as
a catch-all; pre-filter falls back to full-graph clustering within that
cluster. Only non-default domains benefit directly. This is still a net
improvement since non-general domains (coding/trading/research/
communication) typically contain the cleanest clusters."

Secondary: Consider adding a one-liner about whether memories should
participate in cross-domain clustering when embedding similarity is high
enough. The §4.5 doc hints at this ("optionally merge cross-domain at a
higher threshold") but doesn't commit.

---

## FINDING-7 ✅ Applied (2026-04-22) — Minor — §5 P0 phase dependencies are implicit, not charted

**Applied changes:** §5 P0 adds an ASCII dependency graph showing P0.0 independent, P0.1 → P0.2 → P0.3 → {P0.4, P0.5}, P0.6 alongside. Notes which steps can run in parallel.


**Section:** §5 Phase P0 steps 1-6

**Gap:** P0.1 → P0.2 → P0.3 → P0.4 → P0.5 → P0.6 is described in prose
("Phase ordering rationale") but not shown in a dep graph. Since §5 Summary
table only shows phase-level deps (P0 → P1 → P2), someone skimming might
not realize P0.4 and P0.5 can land independently of each other but both
depend on P0.1-0.3.

**Suggested fix:** Add a small ASCII graph or bullet dependency list:
```
P0.1 (types)
 └─ P0.2 (snapshot struct)
     └─ P0.3 (main.rs conversion)
         ├─ P0.4 (prompt enrichment)
         └─ P0.5 (conflict detection)
P0.6 (tests) — runs alongside each of 0.1-0.5
```

---

## FINDING-8 ✅ Applied (2026-04-22) — Minor — §6 Q6 treats `temporal` as orderable, but it's textual

**Applied changes:** §6 Q6 rewritten: notes that `temporal` is `Option<String>` free-form; preferred comparison uses chrono-fuzzy or LLM classifier on parseable dates; fallback to `memory.created_at`. P0.5 ships by flagging candidates and letting synthesis LLM make the final contradiction-vs-evolution call.


**Section:** §6 Q6 ("use `temporal` to distinguish succession from
contradiction. If stance-B's `temporal` > stance-A's `created_at`, mark
as 'evolution' not 'contradiction'")

**Gap:** `temporal` is defined as `Option<String>` in `extractor.rs:22-24`
— a free-form phrase like "yesterday", "Q1 2025", "after the refactor".
It's not a parseable timestamp. The `>` comparison in Q6 isn't possible
without a parsing step.

**Suggested fix:** Rewrite as:
- "Compare `temporal` phrases via LLM or chrono-fuzzy parser before
  ordering" OR
- "Fall back to `created_at` ordering when `temporal` isn't parseable; use
  `temporal` only when both sides provide structured dates."

This doesn't change the plan but avoids a trap where P0.5's "stance
succession" gets implemented as string comparison and produces nonsense.

---

## FINDING-9 ✅ Applied (2026-04-22) — Minor — §7.7 coordination items were stale at time of audit

**Applied changes:** §7.7 first box already `[x] ... *(2026-04-22)*` at audit time; no further change required. Remaining two open items (side-channel notification; post for review before P0) left unchecked — they belong to future sessions.


**Section:** §7.7 Open coordination items

**Observation:** At audit time, "Cross-link this issue from ISS-019's
investigation.md" was an open item. This is now resolved (added to
ISS-019 front-matter as "Downstream consumer: ISS-020"). Mark as done
with date.

**Suggested fix:** Already applied in this review cycle. Change box to
`[x] ... (2026-04-22)`.

---

## Non-findings (things I checked that are correct)

- **§2.2 code reference** (`main.rs:2303-2320`) — verified exact line
  range and content; matches.
- **§2.3** (`compilation.rs:327-360` and `:362-418`) — verified line
  ranges and prompt string format; matches.
- **§2.4** (`conflict.rs:91-107` word-set Jaccard with stop-word filter
  per commit `ac855cb`) — verified. `content_similarity` is at lines
  96-107 in the actual file; the doc's range is within a line or two.
- **§1.1 location of `dimensions` and `type_weights`** (`memory.rs:1320-1345`)
  — verified: `dim_metadata` gets a `dimensions` sub-object and a
  sibling `type_weights` key. The doc's structure is correct.
- **§3 dimensional field list vs `ExtractedFact`** — all 10 Option fields
  (`participants, temporal, location, context, causation, outcome,
  method, relations, sentiment, stance`) are accurate. `domain` is a
  required `String` in `ExtractedFact` with `default_domain()` fallback.
- **§6 Q3** correctly notes ISS-019 blocks coverage, not correctness.
  Sound position.
- **§7.2 graceful fallback invariant** — consistent with §4 `Option<...>`
  guards. Well-stated.
- Terminology section (§1) is the strongest part. The "statements like
  'KC does not read metadata' are simultaneously correct and wrong"
  framing should be quoted in the side-channel session's design doc.

---

## Recommended next step

~~1. Apply FINDING-1 (critical)...~~

**Status (2026-04-22): ALL FINDINGS APPLIED.**

Summary:
- 9/9 findings applied to `investigation.md`.
- FINDING-4 verified against source: `MemoryRecord` has no `updated_at`
  field; doc records the gap and proposes a stopgap (derive from
  `access_times.last()`).
- Doc is now implementation-ready for P0 (starting with P0.0 persistence
  extension so later steps have real data to read).
- Changelog entry added to `investigation.md`.

Remaining open coordination items (not review findings):
- Notify side-channel session of `metadata.dimensions` read dependency.
- Post updated doc for potato's review before starting P0 implementation.
