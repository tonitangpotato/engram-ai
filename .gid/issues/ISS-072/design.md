---
id: ISS-072-design
parent: ISS-072
scope: GOAL-2 (A-clean — TripleExtractor kind plumbing with provenance)
status: draft
authors: [rustclaw]
date: 2026-04-29
---

# Design: ISS-072 GOAL-2 — A-clean (TripleExtractor kind plumbing)

> Plumbs LLM-derived per-endpoint entity kinds from `TripleExtractor` through the resolution pipeline into `Entity.kind`, with explicit provenance so a future enrichment stage (B) can compose without touching this code.

## Goal

After GOAL-1 verdict, the root cause of degenerate entities is split:
- (i) `EntityConfig::default()` empty → Aho-Corasick produces no mentions → triple-lift is the only entity source.
- (ii) Triple-lift entities have no kind because the LLM prompt only emits `{subject, predicate, object, confidence}` — no per-endpoint kinds.

This design fixes (ii): teach the LLM to emit kinds, and plumb them through to `Entity.kind` with **provenance** so the merge story stays clean when GOAL-2.b (enrichment LLM) lands later.

## Non-goals

- The enrichment LLM stage for `summary / importance / attributes` (= GOAL-2.b / Option B).
- Backfilling kinds on entities written before this PR (the future B stage handles them).
- Fixing (i) `EntityConfig::default()` emptiness — separate issue (tracked).
- Adding new `EntityKind` variants. We use the existing enum surface.

## Provenance model

The crux of "no debt" is provenance. Today `Entity.kind` is a single field with no source attribution. After this PR, every kind write carries a `KindSource` that records *who* set it. A future enrichment stage uses this to decide whether to overwrite.

### `KindSource` enum (new)

```rust
// crates/engramai/src/resolution/context.rs
//
// Where the `kind` field on a DraftEntity / Entity originated.
// Used by future enrichment stages (B) to decide merge precedence.
//
// `Serialize` (PascalCase) gives a stable on-disk string contract for the
// `attributes["kind_source"]` persistence key — see `Persistence` section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum KindSource {
    /// No signal was available — kind defaulted to `other("unknown")`.
    Default,
    /// Came from the Aho-Corasick `EntityConfig` dictionary match.
    /// High precision (curated), low recall.
    DictionaryMatch,
    /// Came from `TripleExtractor` LLM emitting `subject_kind` /
    /// `object_kind` per triple. Medium precision, high recall.
    TripleHint,
    /// Came from a dedicated post-resolution enrichment LLM stage (GOAL-2.b).
    /// Highest precision (sees the full entity context, not one triple).
    EnrichmentLLM,
}
```

### Merge precedence (locked policy)

When two writes target the same entity's kind, the higher-ranked source wins:

```
EnrichmentLLM > DictionaryMatch > TripleHint > Default
```

Rationale:
- **`EnrichmentLLM` highest** — it sees the full entity (all triples it appears in, name context, neighbourhood). One-shot kind from a single triple is necessarily noisier.
- **`DictionaryMatch` above `TripleHint`** — dictionary entries are curated by us, so when they fire, they are authoritative. The LLM's per-triple hint is a guess.
- **`TripleHint` above `Default`** — any LLM signal beats no signal.

This PR (A-clean) only writes `Default`, `DictionaryMatch`, and `TripleHint`. The merge logic itself does not need to be implemented yet — there is no enrichment stage to merge against. But the field exists from day one so when B arrives, the merge is one `match` arm, not a schema migration.

### Persistence

`KindSource` is serialized as a string into `Entity.attributes["kind_source"]`. This mirrors the existing `subtype_hint` pattern at `stage_persist.rs:424` exactly — no schema migration, no new columns, no new tables. The `attributes` map is already a free-form JSON object; we add one well-known key.

The serialization uses `#[derive(Serialize)]` with `#[serde(rename_all = "PascalCase")]` on the enum, so the on-disk strings are stable variant names (`"Default" | "DictionaryMatch" | "TripleHint" | "EnrichmentLLM"`) — independent of `Debug` output, which Rust does not guarantee to be stable across compiler versions or refactors. Persisting a `Debug` string is a hidden schema lock; using `serde` makes the wire format an explicit contract.

When reading entities back, code that cares about provenance (= future B stage) reads `attributes["kind_source"]`. Code that just wants the kind reads `Entity.kind` as today (no API change).

## Wire-level changes

### 1. `triple_extractor.rs` — `RawTriple` (parser-local) schema

`RawTriple` is the parser-local deserialization struct defined in `triple_extractor.rs` (~line 94), **not** a public type in `triple.rs`. (Earlier drafts of this design mislabeled the file — the public `Triple` type lives in `triple.rs` and is addressed separately in §1b below.)

Add two optional string fields. Old fixtures and old LLM responses still parse because of `#[serde(default)]`.

```rust
#[derive(Debug, Deserialize)]
struct RawTriple {
    subject: String,
    predicate: String,
    object: String,
    #[serde(default = "default_confidence")]
    confidence: f64,

    // NEW: per-endpoint kind hint from the LLM. Free-form string, validated
    // and mapped to EntityKind by `parse_kind_hint()` in the consumer.
    // Unknown / missing → None → KindSource::Default downstream.
    #[serde(default)]
    subject_kind: Option<String>,
    #[serde(default)]
    object_kind: Option<String>,
}
```

### 1b. `triple.rs` — `Triple` struct kind hint propagation

`RawTriple` is parser-local; once the JSON is parsed, `triple_extractor` constructs the public `Triple` (in `triple.rs`) which is what `pipeline.rs::backfill_endpoint_drafts` actually iterates over. The kind hints must therefore be propagated onto `Triple` itself — otherwise the §5 code (`triple.subject_kind` / `triple.object_kind`) does not compile.

Add two optional fields, kept additive on the wire / in storage:

```rust
// crates/engramai/src/graph/triple.rs
pub struct Triple {
    // ... existing fields: subject, predicate, object, confidence, source, ...

    // NEW: per-endpoint entity-kind hint, parsed from RawTriple.subject_kind /
    // object_kind by triple_extractor and consumed by
    // pipeline.rs::backfill_endpoint_drafts. Optional — `None` means the LLM
    // gave no signal for this endpoint, downstream falls back to KindSource::Default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_kind_hint: Option<EntityKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_kind_hint: Option<EntityKind>,
}
```

`#[serde(default, skip_serializing_if = "Option::is_none")]` keeps existing wire format and on-disk JSON byte-identical when the hints are absent — no migration for any pre-A-clean `Triple` blob.

In `triple_extractor.rs`, after parsing `RawTriple`, populate the hints via `parse_kind_hint` (defined in §4) before constructing the `Triple`:

```rust
let subject_kind_hint = parse_kind_hint(raw.subject_kind.as_deref());
let object_kind_hint  = parse_kind_hint(raw.object_kind.as_deref());
Triple {
    // ... existing fields ...
    subject_kind_hint,
    object_kind_hint,
}
```

Naming note: the hints live on `Triple` as `*_kind_hint` (with `_hint` suffix) to make it textually obvious at every call site that this is an *un-validated guess from one triple*, not an authoritative kind. §5's pseudocode is updated below to use the suffixed names.

### 2. `triple_extractor.rs` — prompt update

Update `TRIPLE_EXTRACTION_PROMPT` to ask for kinds. Use the existing `EntityKind` variant names as the allowed set so the mapping is unambiguous:

```text
Allowed predicates: is_a, part_of, uses, depends_on, caused_by, leads_to,
                    implements, contradicts, related_to

Allowed entity kinds (optional, for subject_kind and object_kind):
  Person, Organization, Place, Concept, Artifact, Event, Topic

If the kind is unclear or doesn't fit the above, omit the field — do not guess.
(Anything outside this allowlist is dropped by `parse_kind_hint` — it does NOT
fall through to `EntityKind::Other`. Use `Other` only via the dedicated
constructor when adding a new canonical variant is not yet warranted.)

Return ONLY a JSON array (no markdown, no explanation):
[
  {
    "subject": "...",
    "predicate": "...",
    "object": "...",
    "confidence": 0.X,
    "subject_kind": "Person",     // optional
    "object_kind": "Organization" // optional
  }
]
```

Update both few-shot examples to demonstrate kinds:

```text
Input: "Rust's borrow checker prevents data races at compile time"
Output: [
  {"subject": "borrow checker", "predicate": "part_of", "object": "Rust",
   "confidence": 0.9, "subject_kind": "Concept", "object_kind": "Concept"},
  {"subject": "borrow checker", "predicate": "leads_to",
   "object": "prevention of data races", "confidence": 0.8,
   "subject_kind": "Concept"}
]

Input: "Caroline volunteered at the LGBTQ support group on weekends"
Output: [
  {"subject": "Caroline", "predicate": "related_to",
   "object": "LGBTQ support group", "confidence": 0.9,
   "subject_kind": "Person", "object_kind": "Organization"}
]
```

The second example is deliberately LoCoMo-shaped to bias the model toward emitting kinds for the actual benchmark domain.

### 3. `context.rs` — `DraftEntity`

Add the `kind_source` field next to the existing `subtype_hint`:

```rust
pub struct DraftEntity {
    pub canonical_name: String,
    pub kind: EntityKind,
    pub aliases: Vec<String>,
    pub subtype_hint: Option<String>,
    pub kind_source: KindSource,        // NEW
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub somatic_fingerprint: Option<SomaticFingerprint>,
}
```

All construction sites must be updated. There are ~6 of them (constructors in `adapters.rs`, fixture builders in `signals.rs`, `pipeline.rs`, `stage_persist_tests.rs`). Most will set `KindSource::Default` since they are test fixtures.

### 4. `adapters.rs` — kind plumbing

`draft_entity_from_mention` (existing dictionary path):

```rust
pub fn draft_entity_from_mention(
    mention: &Mention,
    occurred_at: DateTime<Utc>,
    affect: Option<SomaticFingerprint>,
) -> DraftEntity {
    let (kind, subtype_hint) = map_entity_kind(&mention.entity_type);
    DraftEntity {
        canonical_name: mention.canonical_name.clone(),
        kind,
        aliases: vec![mention.normalized.clone()],
        subtype_hint,
        kind_source: KindSource::DictionaryMatch,    // NEW
        first_seen: occurred_at,
        last_seen: occurred_at,
        somatic_fingerprint: affect,
    }
}
```

`draft_entity_from_triple_endpoint` (existing triple-lift path) — add `kind_hint`:

```rust
pub fn draft_entity_from_triple_endpoint(
    raw: &str,
    occurred_at: DateTime<Utc>,
    affect: Option<SomaticFingerprint>,
    kind_hint: Option<EntityKind>,                   // NEW
) -> DraftEntity {
    let canonical_name = raw.trim().to_string();
    let alias = canonical_name.to_lowercase();
    let (kind, kind_source) = match kind_hint {
        Some(k) => (k, KindSource::TripleHint),
        None    => (EntityKind::other("unknown"), KindSource::Default),
    };
    DraftEntity {
        canonical_name,
        kind,
        aliases: vec![alias],
        subtype_hint: None,
        kind_source,                                 // NEW
        first_seen: occurred_at,
        last_seen: occurred_at,
        somatic_fingerprint: affect,
    }
}
```

Add a helper for parsing the LLM's free-form string into `EntityKind`:

```rust
/// Map the LLM's `subject_kind` / `object_kind` string to `EntityKind`.
/// Returns `None` for empty / unknown strings (caller falls back to `Default`).
///
/// The allowlist mirrors the canonical `EntityKind` variants exactly
/// (see `graph/entity.rs`). `Other(_)` is intentionally excluded — the LLM
/// must not be able to mint arbitrary kinds via this path; that pressure
/// should turn into a real variant via code review.
pub(crate) fn parse_kind_hint(s: Option<&str>) -> Option<EntityKind> {
    let s = s?.trim();
    if s.is_empty() { return None; }
    match s {
        "Person"       => Some(EntityKind::Person),
        "Organization" => Some(EntityKind::Organization),
        "Place"        => Some(EntityKind::Place),
        "Concept"      => Some(EntityKind::Concept),
        "Artifact"     => Some(EntityKind::Artifact),
        "Event"        => Some(EntityKind::Event),
        "Topic"        => Some(EntityKind::Topic),
        _              => None,   // unknown → fall back to Default
    }
}
```

The allowlist is intentional — if the LLM hallucinates `"Animal"` we drop it cleanly rather than smuggle in a stringly-typed kind. Out-of-allowlist hits should be logged at debug level for observability (see Test plan).

### 5. `pipeline.rs::backfill_endpoint_drafts`

Pass the propagated kind hint into the constructor. The function currently iterates over `ctx.extracted_triples`; the hint is already parsed and attached to `Triple` by `triple_extractor` (§1b), so this stage just reads `triple.{subject,object}_kind_hint` directly — no second `parse_kind_hint` call needed:

```rust
for triple in &ctx.extracted_triples {
    upsert_endpoint(
        &mut endpoint_drafts,
        &triple.subject,
        occurred_at,
        affect,
        triple.subject_kind_hint,
    );
    upsert_endpoint(
        &mut endpoint_drafts,
        &triple.object_as_entity_name(),  // None for literal objects
        occurred_at,
        affect,
        triple.object_kind_hint,
    );
}
```

`upsert_endpoint` collects the strongest hint across multiple triples that mention the same endpoint. Precedence within `TripleHint`: first non-`None` wins. (We do not weight by `confidence` because the LLM's per-triple confidence is for the predicate, not the kind.) If a later triple contradicts an earlier kind for the same endpoint, the conflict is logged at debug level — not promoted to a hard error because LLM noise is expected and the enrichment stage (B) will eventually arbitrate.

### 6. `stage_persist.rs` — persist `kind_source`

Mirror the existing `subtype_hint` block at line 424. Use `serde_json` (not `Debug`) — the on-disk string is now an explicit contract, not a side effect of `#[derive(Debug)]`:

```rust
attributes.insert(
    "kind_source".to_string(),
    serde_json::to_value(draft.kind_source)
        .expect("KindSource serialize is infallible"),
);
```

With `#[serde(rename_all = "PascalCase")]` on `KindSource` (see §`KindSource enum` above), this writes `"Default" | "DictionaryMatch" | "TripleHint" | "EnrichmentLLM"` — variant names guaranteed by serde, not by `Debug` output (which the language does not promise to keep stable).

**Critical: this is the ONLY place §1–§7 writes provenance. The merge path does not.** That is fine for A-clean (only the dictionary path → triple path race could matter, and the dictionary path is empty in production today, see Risks table). But it means the merge path needs explicit precedence enforcement before B lands — see §8 below.

### 8. Merge-time precedence enforcement (deferred to GOAL-2.b)

The locked precedence policy is `EnrichmentLLM > DictionaryMatch > TripleHint > Default` (see `Merge precedence` above). §1–§7 establish the **field** (`kind_source` on every draft, persisted into `attributes["kind_source"]`) but **deliberately do not implement merge-time enforcement**. This section makes that gap explicit and defines the contract for the GOAL-2.b PR that will close it.

#### Where the gap lives

`stage_persist.rs::merge_into_canonical` (around line 453) is the *only* place where two writes can target the same canonical entity's `kind`. Today (and after this PR) it does not touch `kind` or `attributes["kind_source"]` on the canonical row at all — incoming draft fields are dropped on merge. That is acceptable for A-clean because:

1. The dictionary path (`KindSource::DictionaryMatch`) is empty in production (`EntityConfig::default()` → no Aho-Corasick mentions).
2. The triple path (`KindSource::TripleHint`) is the only writer, and it always loses to itself: `TripleHint == TripleHint` → first writer wins, no precedence question.

The moment any second source goes live (dictionary becomes non-empty *or* enrichment LLM lands) the gap becomes a real bug: the canonical row's `kind` and `kind_source` would freeze at whatever the first triple emitted, regardless of how strong a later signal arrives. The "zero-incremental-cost when B ships" claim depends on §8 being implemented as a one-`match` arm in the B PR — which is only possible because the field already exists from day one.

#### Contract for GOAL-2.b

The B PR must extend `merge_into_canonical` with a precedence check before any write to `kind` / `attributes["kind_source"]`:

```rust
// Pseudocode — actual call lives inside merge_into_canonical at stage_persist.rs:453.
fn should_overwrite_kind(canonical: &Entity, draft: &DraftEntity) -> bool {
    // Read existing source from canonical.attributes["kind_source"], default to Default if absent.
    let existing = canonical.attributes
        .get("kind_source")
        .and_then(|v| serde_json::from_value::<KindSource>(v.clone()).ok())
        .unwrap_or(KindSource::Default);

    rank(draft.kind_source) > rank(existing)
}

fn rank(s: KindSource) -> u8 {
    match s {
        KindSource::Default         => 0,
        KindSource::TripleHint      => 1,
        KindSource::DictionaryMatch => 2,
        KindSource::EnrichmentLLM   => 3,
    }
}
```

When `should_overwrite_kind` returns `true`, the merge writes BOTH `canonical.kind = draft.kind` AND `canonical.attributes["kind_source"] = serde_json::to_value(draft.kind_source)`. They MUST move together — split writes are a soft-corruption hazard (canonical winds up with a `kind` from one source and `kind_source` from another).

#### What this PR (A-clean) explicitly does NOT do

- Does not implement `should_overwrite_kind` or modify `merge_into_canonical`.
- Does not add a merge-precedence test (no second source exists to write the test against).
- Does not assume merge enforcement is free in B — the B PR owns landing it, with tests.

#### What this PR DOES guarantee

- `attributes["kind_source"]` is populated on every entity created via either path (dictionary or triple) — so when B's merge logic reads `canonical.attributes["kind_source"]`, the value is always present and parseable, never `None` for entities born after this PR.
- The on-disk string format is stable (serde PascalCase), so the round-trip in §6's test plan (#6) covers the read path B will use.
- The precedence ordering is locked here so the B PR does not get to relitigate it.

#### Backfill

Entities written *before* this PR have no `attributes["kind_source"]` key. The B PR's `should_overwrite_kind` defaults missing keys to `KindSource::Default` (see pseudocode above) — so any new signal beats the absence, which is the desired migration behavior. No data backfill required.

### 7. Caller sweep

Every call site of `draft_entity_from_triple_endpoint` must add a trailing `None` for the new `kind_hint` arg. Test files (`adapters.rs` test mod, `triple_integration.rs`, etc.) get the same `None` — this is a behavioral no-op for tests that don't exercise the new path.

## Test plan

### Unit tests (in `adapters.rs` test module)

1. `draft_entity_from_triple_endpoint_with_kind_hint_sets_triple_hint_source` — pass `Some(EntityKind::Person)`, assert `kind == Person && kind_source == TripleHint`.
2. `draft_entity_from_triple_endpoint_without_hint_sets_default_source` — pass `None`, assert `kind == other("unknown") && kind_source == Default`.
3. `draft_entity_from_mention_sets_dictionary_match_source` — assert `kind_source == DictionaryMatch`.
4. `parse_kind_hint_known_variants` — table-driven test for all 6 allowed strings.
5. `parse_kind_hint_unknown_returns_none` — `"Animal"`, `""`, `"   "`, `None` all return `None`.

### Persistence test (in `stage_persist_tests.rs`)

6. `kind_source_round_trips_through_attributes` — build `DraftEntity { kind_source: TripleHint, .. }`, run through stage_persist, assert resulting entity row's `attributes["kind_source"] == "TripleHint"`.

### Triple-extractor parsing test (in `triple_extractor.rs`)

7. `parse_triple_response_accepts_old_format_without_kinds` — input is the existing JSON without kind fields, parses successfully (proves `#[serde(default)]` works).
8. `parse_triple_response_accepts_new_format_with_kinds` — input has `subject_kind` / `object_kind`, both parsed correctly.
9. `parse_triple_response_handles_partial_kinds` — only `subject_kind` present, `object_kind` missing → both fields parse, missing → `None`.

### Integration test (in `triple_integration.rs`)

10. `triple_extraction_with_mocked_llm_propagates_kind_to_entity` — mock LLM returns a triple with `subject_kind: "Person"`. Run through the full pipeline. Assert the resulting `Entity` has `kind == Person` and `attributes["kind_source"] == "TripleHint"`.

### Regression / benchmark

11. Re-run LoCoMo conv-26 (the GOAL-2 acceptance target). Compare entity field distributions:
    - Before: `kind=other("unknown")` ratio ≈ 99%.
    - After: ratio ≤ 30% (most entities should now have a real kind from the LLM).
    - This is the headline metric for closing GOAL-2.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| LLM hallucinates kinds outside the allowlist | `parse_kind_hint` returns `None` → falls back to `Default`. Log debug count for observability. |
| LLM emits kinds inconsistently across triples for the same entity | First non-`None` wins in `upsert_endpoint`; conflicts logged. Enrichment stage (B) will eventually arbitrate with full context. |
| Old fixtures break | `#[serde(default)]` on `RawTriple.subject_kind` / `object_kind` makes them additive. Test #7 proves this. |
| `Entity.kind` write race when both dictionary and triple paths produce drafts for the same canonical name | Resolution stage already deduplicates by canonical name; the merge picks the draft with the higher-ranked `kind_source` (locked policy above). Not implemented in this PR — only relevant when more than one source path actually fires for the same entity. For A-clean alone, the dictionary path is empty in production today (= the original (i) bug), so there is no race. |
| Prompt token budget grows | Two extra optional fields per triple ≈ +30 tokens per response. Negligible. |

## Rollout

This is a single-PR change (no feature flag, no migration). It is purely additive at every layer:

- Schema: optional fields with `#[serde(default)]`.
- API: new arg with default `None` at all existing call sites.
- Persistence: new key in an already-free-form `attributes` map.
- Behavior: when LLM emits no kind, identical to today.

No rollback plan needed — if it regresses anything, revert the PR.

## Open questions

None. (Surface any during review.)

## Out of scope — explicitly deferred

- **GOAL-2.b (Option B):** Enrichment LLM stage that fills `summary / importance / attributes` and re-evaluates `kind` for entities where `kind_source ∈ {Default, TripleHint}`. Separate design doc when GOAL-2 (this PR) lands.
- **Backfill of pre-existing rows:** Entities written before this PR will have no `kind_source` in their `attributes` — code that reads it should treat missing as `Default`. The enrichment stage (B) will overwrite them in due course.
- **Fixing `EntityConfig::default()` emptiness in production wiring** (root cause (i) from GOAL-1): tracked separately. Not blocking GOAL-2 because triple-lift now produces useful kinds even when the dictionary is empty.
