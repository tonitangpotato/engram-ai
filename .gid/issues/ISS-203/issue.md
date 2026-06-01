---
id: ISS-203
title: Entity canonicalization fragments one person into dozens of nodes — case-fold merge fails + possessive/prepositional phrases become standalone entities
status: open
priority: P1
severity: data-quality
tags: [unified-substrate, graph, entity, canonicalization, resolution, locomo]
created: 2026-05-31
relates_to: [ISS-202]
parent: ISS-202
---

# ISS-203: entity canonicalization fragments one person into dozens of nodes

> **One-line:** the resolution pipeline does not (a) case-fold for merge,
> nor (b) strip possessive/prepositional wrappers to the head noun. One
> person ("Caroline") becomes 20+ separate entity nodes. Even after
> ISS-202 reconnects edge provenance, the entity bridge stays diluted
> because anchor resolution can land on the wrong fragment.

## Relationship to ISS-202

ISS-202 is the **primary** fix: it repopulates `edges.source_memory_id` so
the `factual` plan can seed the gold memory from a traversed edge. That
unlocks the *mechanism*. ISS-203 is the **amplifier**: it ensures the
anchor the plan resolves on, and the entity the gold edge hangs off, are
the **same node**. Both are needed for the entity bridge to fire reliably.
ISS-202 lands first (single-point, directly measurable). ISS-203 value is
quantified only after ISS-202 is in.

## Evidence (live DB `.tmpa0Kbrm/substrate.db`, conv-26, 2026-05-31)

### Defect (a) — case-fold merge fails

Two nodes for the same person:

| node id | content | attributes |
|---|---|---|
| `1d11ce4c…` | `caroline` | `{"entity_type":"person"}` (DictionaryMatch) |
| `ce689add…` | `Caroline` | `{"_legacy_kind":"person","kind_source":"DictionaryMatch"}` |

The gold edge for conv-26 q104 (`Caroline --uses--> Becoming Nicole`,
memory_id `ad15485c`) hangs off `ce689add`. A query that resolves its
anchor to the lowercase `1d11ce4c` would miss the bridge entirely.

### Defect (b) — possessive / prepositional phrases as standalone entities

~20 `"Caroline's X"` nodes, e.g.:

```
Caroline's advice          (artifact? no — TripleHint)
Caroline's artwork         (artifact)
Caroline's city            (place)
Caroline's commitment to LGBTQ rights advocacy
Caroline's drive
Caroline's experience
Caroline's group
Caroline's happiness
Caroline's identity
Caroline's inspiration to make art
Caroline's journey as a trans woman
Caroline's motivation
Caroline's own experience of being helped
Caroline's paintings
Caroline's support
Caroline's talk
…
```

Plus prepositional forms: `conversation with Caroline`,
`support from Caroline`, `bothering Caroline`. Same pattern for
`Melanie's *`. None are stripped to the head noun (`Caroline`/`Melanie`)
nor linked back to the canonical person node.

### Scale of fragmentation

- `nodes` entity rows on conv-26: **694**
- `graph_memory_entity_mentions` distinct entity_ids: **699**
- Many of these 694–699 are phrase fragments of a handful of real people.

## Code location to investigate

- `resolution/pipeline.rs` — the resolution stage ordering; where entity
  drafts are matched against existing canonical entities.
- `resolution/entities.rs` — the §3.4.3 entity decision algebra
  (CreateNew vs MergeInto). This is where (i) case-folding for the match
  key should happen, and (ii) a head-noun-extraction / possessive-strip
  normalizer should run before the match.
- The extractor that emits `"Caroline's X"` as an entity span (upstream of
  resolution) — decide whether to strip at extraction or at resolution.
  Leaning toward **resolution-time normalization** (single chokepoint,
  keeps the extractor dumb).

## Proposed fix direction (NOT locked — investigate first)

1. **Case-fold the match key** (not the stored display name) so
   `caroline` and `Caroline` resolve to the same canonical node. Preserve
   original casing in `content`/display; fold only for the dedup key.
2. **Possessive/prepositional normalizer** before the match: strip
   `"X's Y"` → consider `X` as the head person AND `Y` as a separate
   concept linked via a possessive edge (or drop `Y` if it's not a real
   entity). Strip `"<prep> X"` (with/from/about Caroline) → `X`.
3. **Merge migration** for already-ingested DBs (re-point edges + mentions
   from fragment nodes to the canonical node, then soft-delete fragments).

## Acceptance criteria

- **AC-1** `caroline` and `Caroline` resolve to ONE canonical entity node
  (case-fold match key). Display casing preserved.
- **AC-2** `"Caroline's X"` / `"<prep> Caroline"` spans no longer create
  standalone person-fragment nodes; the head person resolves to the
  canonical node.
- **AC-3** Merge migration for existing DBs re-points edges + mentions and
  soft-deletes fragments; idempotent.
- **AC-4** conv-26 same-DB A/B (post-ISS-202) shows the entity-bridge
  questions (q104 class) benefit from de-fragmentation, with no regression.
- **AC-5** Entity-node count on conv-26 drops materially (fragments
  collapsed); spot-check that no two display-distinct real entities were
  wrongly merged.

## Out of scope

- Predicate quality (`uses`/`is_a`/`implements` being code-flavored rather
  than conversational `read`/`recommended`) — that is a separate defect
  (ISS-202 secondary (b)); file independently if pursued.

## Investigation refinement — 2026-05-31 (live DB `.tmpa0Kbrm/substrate.db`)

Architecture mapped. Canonicalization centre = `entities.rs:293`
`normalize_entity_name(name, entity_type)` — currently only
`to_lowercase` + Person strips `@` + Url strips trailing `/`. Dedup has
two layers:

1. **In-batch** (`pipeline.rs::lift_novel_endpoints` ~L980): HashSet on
   `aliases[0]` (lowercased) collapses same-name drafts within one
   episode. Case-fold **works** at this layer.
2. **Cross-batch** (resolution): `search_candidates` → `fusion.rs` →
   `decision.rs` `DecisionThresholds{merge=0.85, defer=0.60}`. Different
   episodes merge by **probabilistic** NameMatch (Jaro-Winkler, already
   case-folded in `signals.rs`) + embedding cosine, gated at 0.85.

**Live-DB reality (sharper than the original autopsy):**

- **Defect (a) is real but rare/legacy.** The graph has exactly ONE clean
  `Caroline` (ce689add, UUID id, `kind="person"`). The lowercase
  `caroline` (1d11ce4c) is a **legacy oddity**: 16-hex short id (not a
  UUID), `entity_type` in a column not attributes JSON — a different write
  path, not a case-fold miss. NameMatch is already case-folded, so true
  case-only splits should merge; this one didn't because it's a
  foreign-shaped node from a legacy path, not the resolution pipeline.
  **Low ROI.**

- **Defect (b) is the disaster.** 21 `Caroline's X` / prepositional
  phrase nodes, each a standalone entity, NONE linked to `Caroline`:
  `Caroline's advice / artwork / city / commitment… / drive / experience
  / group / happiness / identity / inspiration… / journey… / motivation /
  own experience… / paintings / support / talk / wellbeing / work`, plus
  `bothering Caroline`, `conversation with Caroline`, `support from
  Caroline`. `normalize_entity_name` strips none of these → they never
  collide with `caroline` at either dedup layer. **This is the high-ROI
  fix.**

**Decision:** prioritise possessive/prepositional head-noun stripping in
`normalize_entity_name` (defect b). Case-fold legacy reconciliation
(defect a) is a separate, lower-priority concern — the foreign 16-hex
node is not produced by the resolution pipeline this fix touches.

**Implementation plan:**
1. Extend `normalize_entity_name`: strip trailing possessive (`'s`, `s'`,
   `'s X` → head noun before the possessive); strip leading/trailing
   prepositional wrappers (`support from X` → `X`, `conversation with X`
   → `X`). Conservative: only when a known head referent results; do not
   over-strip multi-word proper nouns.
2. Unit tests in `entities.rs`: possessive, prepositional, no-op on clean
   names, no over-strip on legit multi-word names.
3. Re-ingest conv-26 A/B (paired with ISS-202 AC-4) to quantify lift.

## ROOT CAUSE — final analysis (2026-05-31)

This is NOT a normalization bug. Reframing from first principles:

A memory system's job is to map natural language into a knowledge graph
where **nodes are things (entities)** and **edges are relations between
things**. Grammatical structures — possessives (`X's Y`), prepositional
phrases (`Y from X`) — are *the language's way of expressing a relation*,
not entity names.

`"Caroline's paintings"` denotes, in the world:
- entity `paintings` (an Artifact)
- entity `Caroline` (a Person)
- relation `paintings —belongs_to→ Caroline`

When the system extracts the whole phrase as ONE entity node, it has
**encoded a relation inside a node name**. That violates the graph's
basic contract (relations belong on edges). Consequences are structural
and cumulative, not noise that decay clears:
1. One entity fragments into many (Caroline → 21 `Caroline's X` shards;
   the real `Caroline` carries 195 edges, each shard 1–3, and **zero
   shards link back to `Caroline`** — verified on the live DB).
2. The possession relation becomes unqueryable (it's a string, not an
   edge): you cannot ask "what does Caroline own".
3. Every new conversation about Caroline mints more shards — persistent
   structural decay.

### Where the defect actually lives

The shards are triple endpoints lifted by
`adapters.rs::draft_entity_from_triple_endpoint`. Their source is
`triple_extractor.rs::TRIPLE_EXTRACTION_PROMPT`. That prompt **does not
constrain subject/object to atomic entities**, and worse, its own example
demonstrates the wrong behavior — it outputs the phrase
`"prevention of data races"` as an object. The LLM faithfully mirrors
this and emits `"Caroline's paintings"`, `"support from Caroline"` as
endpoints.

### The root fix (what is correct for a memory system)

**Decompose possessive/prepositional phrases into `(head_entity,
relation, referent_entity)` at the extraction layer**, so the graph
stores the relation as an edge and the head as its own atomic entity:
- `Caroline's paintings` → `paintings` (Artifact) + edge
  `paintings —belongs_to→ Caroline`
- `support from Caroline` → `support` + edge `support —from→ Caroline`

Result: `Caroline` absorbs these edges (connectivity grows, not
dilutes), and the sub-entities (`paintings`, `support`) still exist with
their correct kinds — no semantic loss. This is what "one graph" means:
relations on edges, never baked into entity names.

### Why NOT the cheaper options

- Stripping possessives in `normalize_entity_name` (collapse
  `Caroline's paintings` → `caroline`): **lossy** — discards the
  `paintings` sub-entity, and overloads the normalization layer with a
  responsibility (parsing grammar) that isn't its job (its job is folding
  surface variants of the *same* entity).
- Keeping the phrase node + adding an `owned_by` edge downstream:
  **patch** — accepts the wrong premise that `Caroline's paintings` is an
  entity, then sutures around it. Adds an entity the graph shouldn't have.

### Honest risk note

The root fix touches the **extraction layer** (the `belongs_to`/`from`
relation vocabulary + prompt contract + endpoint decomposition), not a
single function. The extraction layer has bitten us before (ISS-162 /
ISS-178 prev-turn context measured *harmful*; ISS-161 L3/L7 prompt
rewrites mostly inert or regressive). So: implement it, but gate it
behind a flag and A/B it on conv-26 + conv-44 — correctness of the graph
representation is the goal, but we still verify the change does not
regress retrieval before flipping the default.

### Implementation direction
1. Add `belongs_to` (and a prepositional relation, e.g. `associated_with`
   /`from`) to the allowed predicates in `TRIPLE_EXTRACTION_PROMPT`.
2. Rewrite the prompt's atomicity contract + replace the bad
   `"prevention of data races"` example with a decomposed one; add a
   possessive example (`X's Y` → `Y belongs_to X`).
3. Verify endpoint lift (`draft_entity_from_triple_endpoint`) handles the
   now-atomic endpoints correctly (it should — they're already atomic).
4. Flag-gate; A/B conv-26 + conv-44 paired with ISS-202 AC-4.

## The architecture already supports the root fix — only the prompt is wrong

Tracing the extraction→graph path confirms the graph layer was DESIGNED
for "relations on edges, arbitrary predicates allowed":

- `graph/schema.rs::Predicate` = `Canonical(CanonicalPredicate)` |
  `Proposed(String)`. `CanonicalPredicate` is already rich
  (`CreatedBy`, `MarriedTo`, `ParentOf`, `MemberOf`, `WorksAt`, …), and
  **`Proposed(String)` preserves ANY LLM-authored relation verbatim**
  (GOAL-1.8 no info loss; defaults to ManyToMany cardinality).
- `resolution/stage_edge_extract.rs`: "known labels pass through, novel
  labels become `Predicate::Proposed(label)`" — the pipeline already
  lifts every triple into a `DraftEdge` and accepts novel predicates.
- The narrow 9-predicate list in `triple.rs::Predicate::from_str_lossy`
  is only the v0.2 triple-extractor vocabulary; `belongs_to` already
  folds to `PartOf` there, and the Proposed path carries anything else.

So the graph can ALREADY represent `paintings —belongs_to→ Caroline`.
Nothing downstream needs new types. The ONLY defect is that
`TRIPLE_EXTRACTION_PROMPT` never tells the LLM to decompose possessive /
prepositional phrases into `(head, relation, referent)` — and its example
actively models the wrong behavior (emitting the phrase
`"prevention of data races"` as an object).

**This is the cleanest possible root fix: a single-layer prompt-contract
change, fully supported by the existing graph + resolution machinery.**
No schema change, no new predicate type, no downstream edits. Just teach
extraction to honor the contract the rest of the system already keeps:
nodes are things, edges are relations.

**Scope (final):**
1. `triple_extractor.rs::TRIPLE_EXTRACTION_PROMPT` — add an atomicity
   contract ("subject and object MUST be atomic entities — a single
   person/place/thing/concept, never a possessive or prepositional
   phrase"); replace the `"prevention of data races"` example with a
   decomposed one; add a possessive example
   (`"Caroline's paintings"` → `paintings —part_of/belongs_to→ Caroline`)
   and a prepositional example (`"support from Caroline"` →
   `support —from→ Caroline`).
2. Flag-gate the new prompt (env/config), keep old as default until A/B.
3. Unit tests: prompt contains the atomicity clause + decomposed
   examples; no eval gold strings leaked into examples.
4. A/B conv-26 + conv-44 paired with ISS-202 AC-4; flip default only on
   no-regression.

---

## Implementation (commit 4a931ff, 2026-05-31)

Root fix landed exactly as scoped, with one refinement discovered during
implementation: **no new `Predicate` variant was needed.** `Predicate::from_str_lossy`
already maps `belongs_to` → `PartOf` and `associated_with` → `RelatedTo`, so the
possessive/prepositional edges round-trip through the existing enum. The fix is
purely a prompt-contract change; the vocabulary was already sufficient.

- Added `TRIPLE_EXTRACTION_PROMPT_V2` (atomicity contract + decomposition rules
  + two new examples: `paintings belongs_to Caroline` and
  `support associated_with Caroline`). Replaced the bad `prevention of data races`
  phrase-object example.
- `select_triple_prompt()` gates on `ENGRAM_TRIPLE_PROMPT_V2` (truthy: `1/true/on/yes`).
  Default = legacy prompt, untouched.
- Wired into both `AnthropicTripleExtractor` and `OllamaTripleExtractor`.
- 4 unit tests: predicate-roundtrip guard, atomicity/decomposition present,
  bad example dropped (legacy untouched), env-var gating. 2093 lib tests pass (+4).

### Acceptance criteria
- [x] AC-1: V2 prompt enforces atomicity contract + demos possessive/prepositional decomposition.
- [x] AC-2: Legacy prompt untouched; new prompt flag-gated (default off).
- [x] AC-3: Unit tests green; no eval gold strings leaked into prompt examples.
- [ ] AC-4: conv-26 + conv-44 A/B (paired with ISS-202 AC-4) confirms no retrieval regression before flipping default.

### Open questions resolved
- *Which prepositional predicate?* → `associated_with` (aliases `RelatedTo`).
  Generic by design; a more specific predicate can be emitted by the LLM when one fits.
- *New predicate variant?* → No. `belongs_to`/`associated_with` already alias existing variants.
- *Does endpoint-lift handle now-atomic endpoints?* → Yes — the lift path
  (`draft_entity_from_triple_endpoint`) consumes subject/object strings verbatim;
  atomic endpoints flow through unchanged. Decomposition happens at the prompt
  layer (the LLM emits two atomic endpoints + a relation), not in `parse_triple_response`.
