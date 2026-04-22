# ISS-020: Knowledge Compiler Dimensional Awareness

**Status:** investigation
**Feature:** knowledge-compiler (cross-cutting with dimensional-extract)
**Severity:** high — KC silently discards all structured dimensional signals produced by the extractor, forcing downstream LLM compilation to re-derive structure from flat prose.
**Related:** ISS-019 (dimensional-metadata-write-gap), feature/dimensional-extract, feature/knowledge-compiler

## TL;DR

The extractor now produces rich dimensional metadata (`participants`, `temporal`, `causation`, `outcome`, `stance`, `domain`, `sentiment`, `method`, `relations`, `type_weights`) and persists it to SQLite under `memory.metadata.dimensions`. Knowledge Compiler's pipeline — **snapshot loading, clustering (discovery), compilation prompts, conflict detection, and quality scoring** — reads none of it. Every downstream step that could benefit from structured signals instead re-infers them (badly) from the flat `content` string.

**Scope of this doc:** audit only. No code changes. Outputs a prioritized work plan.

**Explicitly out of scope:** Caller-owned opaque metadata (the "side channel" — e.g., `dia_id`, `message_id`, `chat_id`). That is a separate contract being redesigned in parallel. This issue is exclusively about **engram-owned cognitive dimensions**.

---

## Section 1 — Terminology: Two Kinds of Metadata

Engram currently co-locates two semantically distinct payloads inside the same
`memory.metadata` JSON column. Any discussion of "KC reading metadata" must
disambiguate these before proceeding.

### 1.1 Cognitive dimensions (engram-owned)

- **Source:** produced by engram's own `extractor.rs` when `Memory::remember`
  auto-extracts facts from raw text.
- **Location today:** `memory.metadata.dimensions.{participants, temporal,
  location, context, causation, outcome, method, relations, sentiment, stance}`
  plus `memory.metadata.type_weights`.
- **Contract:** opaque to caller, meaningful to engram. These are engram's
  internal cognitive representation — structured knowledge derived from text.
- **Value to KC:** high. Every field directly maps to something KC currently
  has to infer from flat prose.
- **Extractor output NOT currently persisted:** `fact.valence` (`f64`) and
  `fact.confidence` (`Confidence` enum) are produced by the extractor on the
  `ExtractedFact` struct but are **not** written to `memory.metadata` by the
  write path at `src/memory.rs:1320-1345`. `fact.valence` is separately
  captured into a transient per-call buffer (`last_extraction_emotions`,
  `Mutex<Option<Vec<(f64, String)>>>`) consumable only by the immediate
  caller of `remember()`; it is not keyed by memory ID and is clobbered on
  the next `remember()` call. `fact.confidence` is not captured anywhere
  after extraction. Any KC work that depends on these fields must first
  extend the persistence layer (see ISS-021 / §5 P0.0).

### 1.2 Caller-owned side channel

- **Source:** caller passes via `remember(..., metadata: Value)` as opaque
  pass-through (e.g., benchmark adapter records `dia_id`, chat bot records
  `message_id`, `chat_id`, `speaker`).
- **Location today:** merged into the same `memory.metadata` JSON at top level,
  alongside `dimensions`.
- **Contract:** opaque to engram, meaningful to caller. Engram promises never
  to interpret these fields.
- **Value to KC:** zero. These are business-layer identifiers that would
  actively harm clustering and conflict detection if treated as signal.

### 1.3 The architectural smell

Both live in one `metadata` JSON blob today. This creates a documentation
hazard: statements like "KC does not read metadata" are simultaneously correct
(for the side channel) and wrong (for dimensions). A clean fix — physically
separating the two channels at the `Memory` struct / SQLite column level — is
tracked elsewhere (side-channel design in parallel session). This issue
assumes they will be separated, and specifies what KC needs from the
**cognitive** channel regardless of how storage is eventually split.

### 1.4 Scope invariant for this doc

> Every recommendation below concerns cognitive dimensions only. If a
> recommendation ever seems to imply KC reading caller-opaque fields, it is a
> bug in this document — flag it.


## Section 2 — Current State: What KC Reads Today

### 2.1 MemorySnapshot (the compilation unit)

Defined in `src/compiler/compilation.rs:18-30`:

```rust
pub struct MemorySnapshot {
    pub id: String,
    pub content: String,
    pub memory_type: String,
    pub importance: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub tags: Vec<String>,
    pub embedding: Option<Vec<f32>>,
}
```

Every downstream KC stage receives a slice of these. **No dimensional fields
are present.** Anything the extractor produced beyond `content` + `tags` is
invisible to KC.

### 2.2 The conversion point (MemoryRecord → MemorySnapshot)

`src/main.rs:2303-2320` (invoked by `knowledge_compile`):

```rust
let snapshots: Vec<MemorySnapshot> = all_memories.iter().map(|m| {
    let tags = m.metadata.as_ref()
        .and_then(|v| v.get("tags"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    MemorySnapshot {
        id: m.id.clone(),
        content: m.content.clone(),
        memory_type: format!("{:?}", m.memory_type).to_lowercase(),
        importance: m.importance,
        created_at: m.created_at,
        updated_at: m.created_at,
        tags,
        embedding: embedding_map.get(&m.id).cloned(),
    }
}).collect();
```

This function **reads only `metadata.tags`**. It explicitly discards
`metadata.dimensions` and `metadata.type_weights`. This is the single-point
root cause: everything downstream in KC operates on a lossy projection.

### 2.3 Compilation prompt construction

`src/compiler/compilation.rs:327-360` (`build_full_compile_prompt`) and
`:362-418` (`build_incremental_compile_prompt`) format every memory as:

```rust
prompt.push_str(&format!("- [{}] ({date}): {}\n", m.memory_type, m.content));
```

The LLM receives a flat bulleted list of content strings. Any structural
relationship (cause→effect, stance, actors, timeline) that the extractor
already identified is erased before the synthesis LLM sees it.

### 2.4 Conflict detection

`src/compiler/conflict.rs:91-107` (`content_similarity`) uses **word-set
Jaccard on `TopicPage` content**, after stop-word filtering (added in commit
`ac855cb` as a patch against template noise). The recent patch is itself
evidence of the underlying problem: pure lexical similarity is fragile
because the structural signal that would make conflict detection robust —
`stance` — is thrown away at snapshot load time.

### 2.5 Discovery / clustering

Uses pre-computed embeddings plus HNSW (commit `40bb499`). Embeddings carry
content semantics but not discrete structural features like `domain`,
`participants`, or `type_weights`. Two memories about different projects
that happen to use similar technical vocabulary will cluster together under
pure embedding similarity. Discrete dimensional filters could disambiguate
but are unavailable.

### 2.6 Quality scoring

`compilation.rs:188-300` scores coverage, freshness, etc. — all derived from
`MemorySnapshot` fields. No dimensional features considered.

### 2.7 Summary of current state

| KC stage | Reads from memory | Uses dimensional signal? |
|---|---|---|
| Snapshot load (main.rs:2303) | `content`, `tags`, `importance`, `created_at`, `embedding` | No |
| Discovery / clustering | `content`, `embedding`, `tags` | No |
| Compilation prompt | `content`, `memory_type`, `created_at` | No |
| Conflict detection | Topic content (post-compile), source IDs | No |
| Quality scoring | Snapshot fields | No |
| Decay / lifecycle | `created_at`, `updated_at` | No |


## Section 3 — Dimensional Fields: Where Each One Should Flow

One row per field defined in `ExtractedFact` (`src/extractor.rs:16-64`). The
"Target stage" column names the KC module where the field delivers the most
value. "Signal type" distinguishes discrete categorical (good for filtering,
grouping, equality checks) from continuous/textual (good for LLM prompt
enrichment).

| Field | Signal type | Target stage | Intended use |
|---|---|---|---|
| `participants` | Textual entity list | Discovery, conflict | Cluster by co-occurring actors; two memories sharing participants likely relate even with different vocabulary. Also: conflict detection (same participant, opposing stance). |
| `temporal` | Textual timestamp/phrase | Compilation prompt, decay | Let synthesis LLM build timeline ("First X happened, then Y"). Distinguishes "what was true" vs "what is true". |
| `location` | Textual | Compilation prompt | Context anchor — rarely central but useful when present. |
| `context` | Textual | Compilation prompt | Background info that's usually too long for `content` but helps synthesis. |
| `causation` | Textual | Compilation prompt (high value) | The single biggest upgrade: lets topic pages express *why* X happened, not just *that* X happened. Transforms "memory summary" → "knowledge". |
| `outcome` | Textual | Compilation prompt (high value) | Pairs with `causation`. Together they form cause→effect edges that pure content strings obscure. |
| `method` | Textual | Compilation prompt | How-to knowledge; procedural content. |
| `relations` | Textual | Discovery (secondary) | Explicit cross-references to other entities/concepts. Could feed edge weights in clustering. |
| `sentiment` | Categorical (short phrase) | Compilation prompt, quality | Emotional coloring for synthesis; also contributes to `valence` drift tracking. |
| `stance` | Categorical (short phrase) | **Conflict detection (critical)** | Two memories with same participants/topic but opposing stance = contradiction. This is the root fix for the template-noise patch. |
| `domain` | Categorical (enum-like) | Discovery, clustering | Top-level filter for clustering. "coding" memories should not cluster with "trading" memories even if embedding is close. |
| `valence` | Continuous `[-1, 1]` | Quality scoring, decay | Drives emotion tracking; could weight memory importance in compilation (strong valence = more salient). ⚠️ *Not persisted today — see §1.1 and §5 P0.0.* |
| `confidence` | Categorical (3-level) | Compilation prompt | Distinguishes asserted facts from speculations. LLM should not synthesize uncertain facts into high-confidence claims. ⚠️ *Not persisted today — see §1.1 and §5 P0.0.* |
| `type_weights` | Structured (7 floats: factual, episodic, procedural, relational, emotional, opinion, causal) | Discovery, quality | Multi-type classification (e.g., 70% factual, 30% episodic). Enables mixed-type topics with appropriate synthesis style. |

### 3.1 Priority tiers

**P0 — immediate, high leverage:**
- `stance` → conflict detection (root fix for commit `ac855cb`)
- `causation` + `outcome` → compilation prompt (transforms topic quality)
- `domain` → discovery filter (reduces cross-domain clustering noise)

**P1 — meaningful quality improvement:**
- `participants` → discovery edge weighting, conflict detection
- `temporal` → compilation prompt (timeline synthesis)
- `type_weights` → discovery + quality scoring
- `confidence` → compilation prompt (uncertainty propagation) — ⚠️ *blocked on persistence fix (P0.0); do not schedule until the write path emits this field.*

**P2 — polish:**
- `context`, `method`, `location`, `relations` → prompt enrichment
- `sentiment`, `valence` → secondary quality signals — ⚠️ *`valence` blocked on persistence fix (P0.0).*


## Section 4 — Gap Analysis by KC Module

### 4.1 `compilation.rs` — MemorySnapshot struct

**Current:** 8 fields, none dimensional.

**Gap:** No representation of structured extractor output.

**Fix shape:**
```rust
pub struct MemorySnapshot {
    // ... existing fields ...
    pub dimensions: Option<Dimensions>,   // new
    pub type_weights: Option<TypeWeights>, // new
    pub confidence: Option<Confidence>,    // new — see note below
    pub valence: Option<f64>,              // new — see note below
}
```

`Dimensions` should be a concrete struct (not `serde_json::Value`) so downstream
code gets type-checked access. All fields `Option<String>` or similar — memories
created before dimensional extract will have all-None.

> **⚠️ Persistence precondition for `confidence` and `valence`:** these two
> fields on `MemorySnapshot` will be populated as `None` under the current
> write path (see §1.1 / §4.2). Keep the fields in the struct definition —
> they are inert when unread — but do not gate any feature on reading them
> until P0.0 (persistence extension) has landed. Until then, every downstream
> code path must treat `snapshot.confidence` and `snapshot.valence` as
> effectively-always-None.

**Risk:** Touches every `MemorySnapshot::new` / test helper in ~16k LoC. High
blast radius but mechanical.

---

### 4.2 `main.rs:2303` — Snapshot construction

**Current:** Reads only `metadata.tags`.

**Gap:** Discards `metadata.dimensions`, `metadata.type_weights`.

> **Note on `valence` / `confidence`:** the extractor produces these on
> `ExtractedFact` but the write path at `src/memory.rs:1320-1345` does not
> persist them into `memory.metadata` (see §1.1). There is a transient
> per-call cache `last_extraction_emotions` in `memory.rs` that captures
> `(valence, domain)` pairs, but it is global state clobbered on every
> `remember()` call and is **not** keyed by memory ID. KC cannot recover
> per-record `valence` or `confidence` from it — the cache is a hand-off
> mechanism for the immediate caller of `remember()`, not a persistent
> field. KC must therefore receive these via persisted metadata, which
> requires the P0.0 write-path extension.

> **Independent latent bug (same closure):** the current code sets
> `updated_at: m.created_at`, which makes every `MemorySnapshot` report
> `updated_at == created_at`. Quality scoring (`coverage`, `freshness` in
> §4.6) and decay (§4.7) assume `updated_at` is a meaningful freshness
> signal; today it is a no-op copy of `created_at`. **Verified:**
> `MemoryRecord` (`src/types.rs:162`) has no `updated_at` field — only
> `created_at` and `access_times: Vec<DateTime<Utc>>`. So the fix is
> either (a) add `updated_at` to `MemoryRecord` and plumb it through SQLite
> writes (larger scope — file as a separate issue), or (b) derive
> `updated_at` from `access_times.last().copied().unwrap_or(m.created_at)`
> as a stopgap. Pick (b) for now in the same closure change as dimensional
> plumbing; defer (a) to a dedicated ticket.

**Fix shape:** Extend the `.map` closure to parse `metadata.dimensions` into
the new `Dimensions` struct and `metadata.type_weights` into `TypeWeights`.
Also populate `updated_at` from the latest access time as the stopgap above.

**Risk:** Low. Pure additive change; falls back to `None` when fields absent.

**Dependency:** Blocked on §4.1.

---

### 4.3 `compilation.rs:327` — Prompt construction

**Current:** `"- [{type}] ({date}): {content}"`

**Gap:** LLM gets no structural signal. Must re-derive causation / actors /
timeline from prose.

**Fix shape — enriched line template:**
```
- [{type} | {domain} | conf={confidence}] ({date}) participants: {participants}
  fact: {content}
  caused_by: {causation}
  outcome: {outcome}
  stance: {stance}
```

Only include lines for fields that are `Some`. For memories without dimensions
(legacy), fall back to current format.

**Additional guidance in prompt header:** instruct the LLM to preserve
`causation`/`outcome` structure in the synthesized topic, not flatten it.

**Token budget analysis:**

Per-memory line cost (tokens, approximate):

| Variant | Template | Avg tokens/line | 50-memory topic | 200-memory topic |
|---|---|---|---|---|
| Current | `- [{type}] ({date}): {content}` (≈200 tok content) | ~210 | ~10.5k | ~42k |
| Enriched (verbose, 6-line per memory) | multi-line with participants/caused_by/outcome/stance | ~390 | ~19.5k | ~78k |
| Enriched (compact, one-line) | `- [{type}|{domain}|conf={c}] ({date}) {ppl} | cause:{x} | outcome:{y} | stance:{z}: {content}` | ~300 | ~15k | ~60k |

The 200-memory verbose case (~78k input tokens) is close to Claude's 200k
context ceiling once the system prompt, topic-discovery instructions, and
schema examples are included. This could silently cap topic size.

**Mitigations (ship at P0, not after regression):**
1. Use the **compact one-line form** by default — halves the newline overhead
   when most fields are populated.
2. Add `prompt_detail_level: {minimal | standard | full}` config
   (`minimal` = current format, `standard` = compact one-line enriched,
   `full` = multi-line verbose). Default to `standard`.
3. When assembling the prompt, if projected input tokens > configured
   budget (default 120k for safety), drop lowest-importance memories first
   until under budget, and note the truncation in topic metadata.

**Risk:** Medium — prompt changes affect output quality. Requires A/B test on
a snapshot of real memories to validate topic quality doesn't regress.

**Dependency:** Blocked on §4.1, §4.2.

---

### 4.4 `conflict.rs` — Conflict detection

**Current:** `jaccard_similarity(words_a, words_b)` on topic content. Recent
patch (`ac855cb`) strips template noise — treating the symptom.

**Gap:** True contradictions are structural: same (participants, domain,
topic area) + opposing `stance`. Lexical Jaccard can't express this.

**Fix shape:** Add a structural conflict check that runs *before* / *alongside*
lexical similarity:

```rust
fn dimensional_conflict(a: &MemorySnapshot, b: &MemorySnapshot) -> Option<Conflict> {
    let (da, db) = (a.dimensions.as_ref()?, b.dimensions.as_ref()?);
    if da.domain != db.domain { return None; }
    if !participants_overlap(&da.participants, &db.participants) { return None; }
    if stances_oppose(&da.stance, &db.stance) {
        return Some(Conflict::Contradiction { ... });
    }
    None
}
```

`stances_oppose` initially can be "both present AND not equal" as a crude
heuristic; later upgraded with an LLM classifier or a small sentiment-opposition
model. This is the **root fix** for the template-noise patch.

**Risk:** Medium. Need validation that dimensional conflict doesn't produce
false positives (e.g., stance evolution over time — "I used to prefer X" vs
"now I prefer Y" isn't a contradiction, it's temporal succession).

**Dependency:** Blocked on §4.1, §4.2.

---

### 4.5 `discovery.rs` — Clustering

**Current:** HNSW over pre-computed embeddings.

**Gap:** Embeddings collapse discrete categorical info. Two different
projects can look embedding-close.

**Fix shape — two options:**

(a) **Pre-filter by domain:** Cluster within each domain separately, then
optionally merge cross-domain at a higher threshold. Cheap, big win.

(b) **Augmented edge weights:** After HNSW produces candidate edges, multiply
weight by dimensional similarity:
```
final_weight = embedding_sim * (1 + α * domain_match + β * participants_jaccard)
```

Start with (a) — simpler, higher confidence. Defer (b) until we see real
clustering failures that (a) doesn't fix.

**Caveat — `general` domain catch-all:** The extractor's `default_domain()`
(`src/extractor.rs`) is `"general"`, and that bucket is typically the
largest. Pre-filter within `general` is equivalent to full-graph clustering
(the problem we started from). Only non-default domains (e.g., `coding`,
`trading`, `research`, `communication`) benefit directly from (a). This is
still a net improvement since those non-general clusters are usually the
cleanest. Option (b) — augmented edge weights with participant Jaccard —
remains the path to improve clustering within `general` specifically;
schedule it as a P1 follow-up if `general` cluster quality is visibly poor
after (a) ships.

Secondary question (deferred): should cross-domain edges be allowed when
embedding similarity is very high (e.g., > 0.85)? Concrete answer needed
before (a) ships. Provisional: yes, with a hard cap of 5% cross-domain
edges per cluster to avoid silent domain bleed.

**Risk:** Low-to-medium. Changes clustering output; need regression test on
existing corpus.

**Dependency:** Blocked on §4.1, §4.2.

---

### 4.6 `compilation.rs:188-300` — Quality scoring

**Current:** `coverage`, `freshness`, etc. — all position/date based.

**Gap:** No signal for "topic with diverse dimensional coverage" vs "topic
that's just rehashing one angle". A good topic page should surface causation,
outcomes, and stances — not just restate facts.

**Fix shape:** Add `dimensional_coverage` sub-score: fraction of constituent
memories' dimensional fields that are mentioned in the compiled topic. If
50% of source memories have `causation` but the topic page mentions none,
the synthesis dropped signal → quality penalty.

**Risk:** Low. Additive metric.

**Dependency:** Blocked on §4.1, §4.2, §4.3 (needs enriched topics to score).

**Priority:** P2 — nice-to-have, validates the pipeline works.

---

### 4.7 `decay.rs` / `topic_lifecycle.rs`

**Current:** Time-based decay.

**Gap:** `temporal` field sometimes carries explicit "as of 2025-Q1" info
that should override mechanical age. `stance` evolution (user changed mind)
should demote old conflicting memories faster than additive ones.

**Fix shape:** Stretch goal. Annotate memories where `temporal` disagrees
with `created_at`, and memories where a superseding stance exists. Defer.

**Priority:** P2.

---

### 4.8 Not affected

- `lock.rs`, `watcher.rs`, `import.rs`, `export.rs`, `manual_edit.rs`,
  `feedback.rs`, `privacy.rs`, `intake.rs` — all orthogonal to dimensional
  signal. No changes needed.


## Section 5 — Prioritized Work Plan

Grouped into three phases. Each phase is independently shippable and leaves
the codebase in a better state than before.

### Phase P0 — Structural plumbing + root fixes (estimated: 2-3 days)

Goal: KC reads dimensions end-to-end. Root-fix conflict detection. Set up the
foundation for all later work.

0. **P0.0** — Extend the extractor write path at `src/memory.rs:1320-1345`
   to persist `fact.valence` (as `f64`) and `fact.confidence` (as the
   string form of `Confidence`) into `memory.metadata.dimensions` alongside
   the existing 10 Option fields. ~10 lines, low risk. Also add unit tests
   that confirm both fields round-trip through SQLite. Unblocks later
   tiers that reference these fields (§3.1 P1/P2). *(Could alternatively
   live in ISS-021 if we want to keep ISS-020 purely read-side; decision
   noted at start of implementation.)*
1. **P0.1** — Define `Dimensions`, `TypeWeights`, `Confidence` as concrete
   Rust types in `src/compiler/types.rs`. Derive `Debug`, `Clone`,
   `Serialize`, `Deserialize`. Include a `from_metadata_json` constructor
   that parses `memory.metadata` safely (missing fields → `None`).
2. **P0.2** — Extend `MemorySnapshot` (§4.1) with `dimensions`, `type_weights`,
   `confidence`, `valence` as `Option<...>`. Fix all `MemorySnapshot::test`
   helpers and call sites.
3. **P0.3** — Update the conversion point in `src/main.rs:2303` (§4.2) to
   populate dimensional fields from `MemoryRecord.metadata`. Also fix the
   `updated_at` stopgap documented in §4.2.
4. **P0.4** — Enrich compilation prompt (§4.3) to surface
   `causation / outcome / stance / participants / domain`. (Add
   `confidence` only after P0.0 lands, otherwise it is always `None`.)
   Back-compat: if all dimensional fields are `None`, emit current format.
5. **P0.5** — Add dimensional conflict detection (§4.4) alongside existing
   lexical Jaccard. Rule: same `domain` + participant overlap +
   divergent `stance` → contradiction candidate. Keep lexical as secondary
   signal.
6. **P0.6** — Tests: unit tests for `Dimensions::from_metadata_json` (present,
   absent, malformed), prompt generation snapshot tests, conflict detection
   with synthetic stance-opposition pairs.

**P0 internal dependency graph:**

```
P0.0 (persist valence/confidence)  ── independent, can ship first
P0.1 (types)
 └─ P0.2 (snapshot struct)
     └─ P0.3 (main.rs conversion + updated_at fix)
         ├─ P0.4 (prompt enrichment)        ← picks up confidence only after P0.0
         └─ P0.5 (conflict detection)
P0.6 (tests)                       ── runs alongside each of P0.0–P0.5
```

P0.0 and P0.1 can land in parallel (they touch different files). P0.4 and
P0.5 can land in parallel once P0.3 is in. P0.6 grows incrementally.

**Exit criteria:**
- `cargo test` passes.
- Prompt snapshot test shows enriched content for memories with dimensions.
- At least one existing conflict-detection test can be simplified because
  lexical Jaccard is no longer the sole signal.
- Existing KC integration tests pass unchanged (back-compat confirmed).

### Phase P1 — Clustering + confidence propagation (estimated: 2-3 days)

Goal: clustering respects domain boundaries. Uncertainty propagates through
synthesis.

1. **P1.1** — Domain pre-filter in `discovery.rs` (§4.5 option a). Cluster
   within each domain; optionally merge at high-threshold cross-domain edges.
2. **P1.2** — Temporal field surfaced to prompt — enables timeline synthesis.
3. **P1.3** — `confidence` propagation: if >20% of source memories have
   `uncertain` confidence, mark topic as `confidence: draft` in metadata
   and instruct LLM to hedge claims derived from them.
4. **P1.4** — Use `type_weights` to select synthesis style. E.g., topics
   whose memories are mostly procedural should produce how-to topic pages,
   not chronological narratives.
5. **P1.5** — Regression corpus: snapshot the 20 most-recompiled topics
   before/after to evaluate quality shift. Human review on 5.

**Exit criteria:**
- Domain pre-filter reduces cross-domain clusters by measurable amount on
  the real engram DB.
- Confidence tags appear on topics where source memories warrant them.

### Phase P2 — Quality scoring + decay refinements (estimated: 1-2 days)

Goal: KC can measure whether it's actually using dimensional signal well.

1. **P2.1** — `dimensional_coverage` metric (§4.6).
2. **P2.2** — Temporal-aware decay (§4.7) — explicit dates in `temporal`
   override mechanical age.
3. **P2.3** — Stance-succession heuristic — when a memory's stance
   supersedes a prior memory's stance with overlapping participants,
   demote the older one faster in decay.

**Exit criteria:**
- Quality reports include dimensional-coverage score.
- At least one known "superseded opinion" pair in the DB shows correct
  demotion after decay pass.

### Summary table

| Phase | Scope | Effort | Blocks | Status |
|---|---|---|---|---|
| P0 | Persistence fix + plumbing + stance-conflict root fix | 2-3 days | — | Ready to start |
| P1 | Clustering + confidence propagation | 2-3 days | P0 (esp. P0.0 for P1.3) | After P0 |
| P2 | Quality + decay refinements | 1-2 days | P1 | After P1 |
| Total | | 5-8 days | | |

### Phase ordering rationale

P0 is ordered so that each step is independently testable. The struct
definition (P0.1) is pure types, no behavior. P0.2-0.3 wire plumbing without
changing outputs. P0.4 changes LLM prompt (first observable change). P0.5
adds the conflict-detection root fix (second observable change). Any of
these can be merged independently once tested.

P1 can start in parallel with P0.6 if two people are working. Otherwise
strictly sequential.

P2 is the only phase that might be punted indefinitely — it polishes rather
than fixes. Acceptable to stop after P1.


## Section 6 — Open Questions

Questions that need decisions before or during P0. Each has a provisional
answer; none are blockers for starting work.

### Q1: Where does `Dimensions` struct live?

Options:
- (a) `src/compiler/types.rs` — KC-local type, parses from JSON
- (b) `src/extractor.rs` — shared with extractor (single source of truth)
- (c) new `src/dimensions.rs` — neutral module

**Provisional:** (b). The extractor already defines `ExtractedFact` with all
these fields; KC should use a sibling type (`Dimensions`) that's a subset,
defined in the same module. One edit point for schema evolution.

### Q2: How does KC handle memories written before dimensional extract landed?

All older memories have `metadata.dimensions = None`. KC must fall back to
current (content-only) behavior for them.

**Provisional:** yes, graceful degradation. Every dimensional feature checks
`Option<Dimensions>` and skips enrichment when `None`. This is already the
intended design in §4 but worth stating as invariant.

### Q3: What about ISS-019 (metadata write gap)?

ISS-019 says ~76% of dimensional metadata is being silently dropped on write.
If true, KC reading dimensions will mostly see `None` until ISS-019 is fixed.

**Provisional:** P0 in this issue does not block on ISS-019. KC should be
correct for *future* memories with proper dimensions; existing memories
remain fallback cases. Fixing ISS-019 retroactively improves recall quality
but doesn't change KC's interface.

**Action:** cross-link ISS-019 → ISS-020 so whoever fixes ISS-019 knows KC
will consume the fix automatically.

### Q4: Side-channel separation — wait, or proceed?

The parallel side-channel redesign may reshape `memory.metadata` structure.
Does KC work wait?

**Provisional:** no, don't wait. KC reads `metadata.dimensions` today and
should continue reading it from wherever it lives post-separation. The
interface between KC and storage is `Dimensions::from(memory_record)` —
where `memory_record.metadata` ends up structured doesn't affect KC. At
worst, P0.2's conversion function gets updated to read from a different
path (one-line change).

**Coordination:** notify side-channel session that KC will depend on
`memory.metadata.dimensions` (or whatever the post-separation field is
called). Both sessions must agree on the final field path.

### Q5: How to validate prompt enrichment doesn't regress topic quality?

New prompt format could confuse the synthesis LLM.

**Provisional:** P1.5 regression corpus. Before merging P0.4, recompile
20 existing topics both ways; diff output; human-review the top 5 most
changed. If >2 show regression, tune the prompt or add more context/rules.

### Q6: Dimensional conflict detection false positives?

Simple "stances differ" rule might trip on benign stance evolution
("used to think X, now think Y") or contextual differences ("in project A,
X; in project B, not X").

**Provisional:** use `temporal` to distinguish succession from contradiction,
but note that `temporal` in `ExtractedFact` is `Option<String>` holding a
free-form phrase ("yesterday", "Q1 2025", "after the refactor") — not a
parseable timestamp. Concrete comparison plan:
- **Preferred:** when both sides have parseable dates (via `chrono-fuzzy` or
  a small LLM classifier), compare directly: if stance-B's parsed date
  post-dates stance-A's → "evolution", not contradiction.
- **Fallback:** when `temporal` is unparseable on either side, order by
  `memory.created_at` instead — weaker signal but always available.
- Use `participants` and `location` for context scoping to reduce false
  positives from same-author different-project contradictions.

Acceptable to ship P0 with known false positives — specifically, P0.5 should
flag candidates and let the topic-page synthesis LLM make the final
"contradiction vs evolution" call until we invest in the parser/classifier.

### Q7: Performance impact of richer prompts?

Enriched prompt is ~2-3x token count per memory line. For a topic with
50 source memories, that's meaningful context growth.

**Provisional:** acceptable at P0. Monitor compilation cost. If problematic,
add configurable `prompt_detail_level: {minimal, standard, full}` — `full`
is the new enriched format; `minimal` is today's format.


## Section 7 — Migration & Backward Compatibility

### 7.1 Schema — no SQLite migration required

Dimensions already live inside the existing `memory.metadata` JSON column.
KC adding code to read that JSON is **read-side only**. No schema change,
no migration script.

If the parallel side-channel work eventually physically separates cognitive
dimensions into their own column, that's a write-side migration owned by
the side-channel issue — KC will adapt its read path to the new location
(one-function change per §4.2).

### 7.2 Existing memories — graceful fallback

All memories written before dimensional extract have
`metadata.dimensions = None` or `metadata.dimensions = {}`. KC must treat
these as legacy and fall back to content-only behavior (§4.3, §4.4, §4.5
all specify `if let Some(...)` guards).

Stated as a hard invariant:
> Every code path that uses dimensional data must degrade gracefully when
> the data is `None`. A KC run over a DB of 100% legacy memories must
> produce the same output as today.

### 7.3 ISS-019 interaction

ISS-019 reports that 76% of dimensional metadata is being dropped at write
time. This affects *new* memories too, not just legacy ones. Consequences:

- Until ISS-019 is fixed: KC will see dimensions on the ~24% of memories
  that made it through. Enrichment still works on those; others fall back.
- After ISS-019 is fixed: enrichment coverage rises to ~100% for newly
  written memories. Older memories remain in fallback mode.
- Optional: once ISS-019 is fixed, consider a one-off batch job that
  re-extracts dimensions from older memories' original content. Out of
  scope for this issue but worth documenting as a future "retroactive
  enrichment" task.

### 7.4 Topic pages — versioning

Existing topic pages were compiled without dimensional context. After P0
ships, recompiling them will produce richer output.

- Topic pages carry a `version` field; increment on recompile.
- KC stores each compilation with a `sources_hash` — unchanged sources
  won't trigger recompile just because KC got smarter.
- Add a config flag `force_recompile_on_schema_change: bool` (default
  false). When true, a compilation-schema-version bump causes all topics
  to be recompiled on next run. This lets users opt into reaping the
  quality improvement without forcing it.

### 7.5 External consumers

Any tool that consumes `MemorySnapshot` directly (e.g., Rustclaw sub-agent
context assembly, if any) will see new optional fields. Since they're
`Option<...>`, downstream code compiles unchanged — they just don't yet
use the new signal.

### 7.6 Rollback plan

If P0 ships and causes topic-quality regression:
1. Feature-flag the enriched prompt (`prompt_detail_level: minimal` reverts).
2. Feature-flag dimensional conflict detection (env var or config bool).
3. `Dimensions` field in `MemorySnapshot` stays — it's inert if unread.

No schema rollback, no data loss scenarios.

### 7.7 Open coordination items

- [x] Cross-link this issue from ISS-019's investigation.md. *(2026-04-22)*
- [ ] Notify side-channel session (parallel work) of the `metadata.dimensions`
      read dependency; agree on final field path post-separation.
- [ ] Post this doc for review before starting P0 implementation.

---

## Changelog

- **2026-04-22:** Initial audit (this document).
- **2026-04-22:** Applied review r1 findings (FINDING-1 through FINDING-9).
  Corrected persistence status of `valence`/`confidence` (they are produced
  by the extractor but not persisted today; added P0.0 write-path step);
  fixed `TypeWeights` cardinality (7 floats, not 5); removed misleading
  `last_extraction_emotions` claim; documented independent `updated_at =
  m.created_at` bug and stopgap fix; added token-budget analysis and
  compact-line prompt variant; added `general` domain caveat for
  clustering pre-filter; added P0 internal dependency graph; corrected
  `temporal` free-form-string comparison plan in Q6. See
  `reviews/investigation-r1.md` for the full review.

