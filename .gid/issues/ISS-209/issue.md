---
id: ISS-209
title: Case-fold entity split (caroline vs Caroline) survives ISS-203 — reservation works only because anchor resolution lands on the edge-owning node by luck; fragile + aggregate-suppressing
status: open
priority: P0
severity: data-quality
tags:
- unified-substrate
- graph
- entity
- canonicalization
- resolution
- locomo
created: 2026-06-02
relates_to:
- ISS-202
- ISS-203
- ISS-205
- ISS-207
labels:
- blocker
- entity-canonicalization
- proven-blocker
- ac0-done
- fix-insufficient
---

# ISS-209: case-fold entity split survives ISS-203

> **One-line:** ISS-203 correctly pivoted to the ranking-layer reservation
> (ISS-205/207) and explicitly deferred **case-fold defect (a)** as "Low
> ROI". Today's q0 delivery probe shows that judgement is unsafe: the same
> person still exists as two base-name nodes (`caroline` lowercase mention
> node, `Caroline` uppercase structural node). The ISS-205 reservation
> works on conv-26 q0 **only because anchor resolution happens to land on
> the uppercase node that owns the edges.** Nothing guarantees that. Fix
> the split at write time so the mention-path node and the edge-owning node
> are one.

## Why this is filed separately from ISS-203 (not a reopen)

ISS-203 is correctly `resolved`: it diagnosed entity fragmentation, tried
the extraction-prompt V2 fix, falsified it (multi-hop −8pp on two corpora,
DB-verified crowding), and pivoted to the ranking-layer reservation —
which became ISS-205 and is verified working (see below). That was the
right call for that issue.

What ISS-203 explicitly set aside, with the reasoning *"the foreign 16-hex
node is not produced by the resolution pipeline this fix touches"*, is
**case-fold defect (a)**: `caroline` (lowercase, content-hash id) vs
`Caroline` (uppercase, UUID). ISS-203 treated the lowercase node as a
dismissable legacy artifact. This issue carries the evidence that it is
NOT dismissable — it is the live mention-path node that retrieval anchor
resolution can land on, and the split directly determines whether
date-asking queries can ever reserve their gold edge.

## Evidence (forensic DB `.tmpK8lZyN/substrate.db`, conv-26, 2026-06-02)

Two base-name Caroline nodes, split by WRITE PATH:

- `1d11ce4c` — `caroline` (lowercase, content-hash id). Owns **0**
  `occurred_on` edges. This is the dedup / mention-path canonical.
- `d7f9a67a` — `Caroline` (uppercase, UUID). Owns **all 31**
  `occurred_on` edges, incl. the q0 gold edge
  (`source_memory_id=a838a102`, `target_literal="2023-05-07"`).

Plus the ~21 `Caroline's X` / prepositional phrase shards already
catalogued in ISS-203 (those are a separate, harder fix — see ISS-203's
root-cause section; this issue is specifically the **base-name case
split**).

### Why the ISS-205 reservation works today — and why that is fragile

`iss205_anchor_probe` against this DB resolves `Caroline` (`d7f9a67a`,
the edge owner) at **rank 0, strength 1.0000** for the q0 query. Because
the anchor lands on the edge-owning node, `edges_of(anchor, OccurredOn)`
returns all 31 edges, the date-asking reservation admits the gold
episode, and the end-to-end delivery probe
(`iss207_q0_delivery_probe`) confirms gold `a838a102` reaches **rank 2 in
top-10** with its surfaced line `[2023-05-07] Caroline attended a LGBTQ
support group`.

This is a **success that depends on a coin landing the right way up.** If
the resolver had instead matched the lowercase `caroline` mention node
(0 edges) — which is the dedup canonical and a perfectly plausible anchor
target — the reservation would have had nothing to admit and q0 would
stay 0.0. The split means anchor-resolution correctness and edge-ownership
are decoupled; they only coincide by the resolver's current scoring
happening to prefer the uppercase node.

## Impact

1. **Fragility on the q0-class fix.** ISS-205's date-asking reservation is
   load-bearing for the whole temporal-retrieval track (ISS-190/191/201/
   205/207). It silently depends on anchor resolution and edge ownership
   pointing at the same node. Any change to resolver scoring, embedder, or
   ingest order can split them and regress date-asking queries with no
   obvious cause.
2. **Aggregate suppression.** Every person mentioned across conversations
   risks the same mention-path vs structural-path split. The mention node
   accumulates dedup provenance; the structural node accumulates the
   semantic edges. Retrieval that anchors on the wrong half loses the
   edges. This is a plausible contributor to the conv-26 aggregate sitting
   at ~0.29 vs the ~0.40 high-water mark (ISS-136), though that link is a
   hypothesis to be quantified, not yet proven.

## Root fix direction (NOT locked — investigate first)

The two base-name nodes must become one at write time, so the
mention-path canonical and the edge-owning entity share an id. Candidate
approaches (decide after reading the resolution pipeline):

- **(A) Deterministic case-fold canonical key.** Before the probabilistic
  `search_candidates → fusion → decision` merge path, force an exact
  case-folded-name match to MergeInto (or add an `ExactCanonical` signal
  with confidence 1.0 so case-only differences never reach the 0.85
  probabilistic gate). This is the root fix for defect (a).
- **(B) Unify the write paths.** The lowercase content-hash node comes
  from the mention/dedup path; the uppercase UUID node from the
  triple/resolution path. Investigate whether both paths can mint/lookup
  the same canonical entity id instead of two.

Workaround (NOT the root fix, but worth noting as a safety net):
- **(C) Make reservation anchor-resolution fragment-tolerant** — gather
  edges across all same-base-name entity nodes (case-insensitive) so the
  reservation does not depend on landing on the exact edge owner. Cheaper,
  but leaves the split in place and the graph still fragmented for every
  other query type.

Leaning (A) as the root fix: it is a single chokepoint, it is what a
memory system's entity-identity contract requires (case is not identity),
and it removes the fragility for ALL query types, not just the reserved
temporal path.

## Out of scope

- The `Caroline's X` possessive/prepositional shards — that is the harder
  extraction-layer decomposition already root-caused in ISS-203. This
  issue is ONLY the base-name case split (caroline ↔ Caroline). The shard
  fix can be a follow-up off ISS-203's root-cause section once the
  base-name merge is proven.

## Acceptance criteria

- [ ] AC-1: On a fresh conv-26 ingest, the entity `Caroline` exists as a
      single node (case-insensitive base name `caroline`), owning all the
      `occurred_on` edges currently split across `1d11ce4c`/`d7f9a67a`.
      Verified by SQL: exactly one `node_kind='entity'` row whose
      case-folded content is `caroline` AND `count(occurred_on edges
      where source_id = that node) == 31` (or the post-merge total).
- [ ] AC-2: The fix is at write time (resolution/dedup), not a read-time
      gather. The DB after ingest has no duplicate base-name entity nodes.
- [ ] AC-3: `iss205_anchor_probe` resolves the single merged node for the
      q0 query (rank 0), and the merge does not depend on resolver scoring
      preference — it is deterministic from the name.
- [ ] AC-4: `iss207_q0_delivery_probe` still delivers gold `a838a102` into
      top-10 (no regression from the merge).
- [ ] AC-5: Paired conv-26 A/B (merge off vs on) under the locked ISS-190
      envelope: single-hop and temporal categories do not regress; if the
      crowding mechanism from ISS-203 reappears (denser entity → date
      episodes pushed out), the reservation must still hold them in
      (cross-check with ISS-205's reservation R). Overall within ±10%
      wobble; flag any multi-hop regression for the same crowding analysis
      ISS-203 did.

## AC-0 DIAGNOSIS (resolved 2026-06-02) — the merge gate is a red herring

The issue premise ("WHY did caroline/Caroline not merge despite
Jaro-Winkler ~1.0 / which signal pulled them below 0.85") is a **wrong
hypothesis. The probabilistic merge gate never ran.** The split is not a
threshold/scoring problem — it is two independent id schemes for the same
entity that never enter the same candidate pool.

### The two writers (both in `default` namespace)

1. **Legacy entities path** — `Storage::upsert_entity` (storage.rs:6116)
   derives the id via `generate_entity_id(name, type, namespace)`
   (storage.rs:311): an **FNV-1a hash of `lowercase(name)|lowercase(type)|ns`**,
   formatted `{:016x}`. **This path already case-folds correctly.**
   Verified by recomputation: `caroline`, `Caroline`, `CAROLINE` ALL hash
   to `1d11ce4c7b5c12d8` — exactly the lowercase node's id. (`melanie` →
   `8fd5fe104ddd2ad4`, `go` → `2f61644ae102c34a` — the only 3 legacy
   `entities` rows, all matching.) The T23 backfill
   (`backfill_memory_entities_to_edges`, substrate/backfill.rs:1475)
   projects this `entities` row verbatim into `nodes` and hangs the
   `provenance/mentions` edges off it.

2. **Resolution pipeline path** — drafts a `DraftEntity` and on
   `Decision::CreateNew` mints a fresh **`Uuid::new_v4()`** (random). This
   becomes `Caroline` (original case, UUID `d7f9a67a`) and owns **all 31
   `occurred_on` + structural edges**. The graph store *requires*
   UUID-format ids (`graph/store.rs:716` — "nodes.id is the 36-char
   hyphenated UUID TEXT").

### Why they never merge

The resolution pipeline **never consults `generate_entity_id` / the
legacy `entities` table** before minting its random UUID. The legacy
path's deterministic FNV id and the pipeline's random UUID are different
strings for the same person, so the two nodes coexist. `candidate_retrieval`
only compares entities the pipeline itself produced — the FNV/mention node
is invisible to it. No shared candidate pool → `decision.rs` never compares
them → no merge, *regardless of name similarity*. Jaro-Winkler is
irrelevant.

### Revised root-fix direction — supersedes (A)/(B)/(C) above

The clean root fix is **make the resolution pipeline's `CreateNew` derive a
deterministic id from the case-folded canonical key instead of
`Uuid::new_v4()`**. To satisfy the graph store's UUID invariant, use
**UUIDv5** (content-addressed SHA-1 over a fixed namespace + the canonical
key `lowercase(name)|type|namespace`). Then:

- `Caroline`, `caroline`, `CAROLINE` → the same UUIDv5 → one node,
  deterministically, with no merge-gate, no Jaro-Winkler, no threshold
  tuning. (Strictly better than `Uuid::new_v4()` and than approach (A)'s
  ExactCanonical signal, which would still be probabilistic-pool-dependent.)
- The legacy FNV path and the resolution path should ALSO be reconciled so
  the mention/provenance node and the structural node share one id. Two
  sub-options: (i) make the legacy `upsert_entity` for already-resolved
  entities reuse the pipeline's UUIDv5 instead of the FNV id, or (ii) have
  the resolution pipeline look up / adopt the existing legacy entity id
  when one exists for the case-folded name. Decide during implementation.

**Decision needed from potato before implementing:** the UUIDv5 namespace
seed + whether to migrate existing 16-hex FNV legacy ids or only fix
forward (new ingests). Existing DBs with the split would need a one-time
merge migration; fix-forward alone fixes new ingests but leaves historical
DBs fragmented.

## Notes

- AC-0 verification: `generate_entity_id` recomputation (FNV-1a) confirms
  legacy path case-folds; resolution `CreateNew` uses `Uuid::new_v4()`
  (non-deterministic). The split is write-path divergence, not a merge
  scoring failure.
- Today's verification artifacts:
  `crates/engramai/examples/iss205_anchor_probe.rs` (anchor lands on
  edge owner rank 0) and
  `crates/engramai/examples/iss207_q0_delivery_probe.rs` (gold rank 2 in
  top-10, dated line surfaced). Forensic DB
  `.tmpK8lZyN/substrate.db`.
- Code locations to investigate (from ISS-203): `resolution/pipeline.rs`,
  `resolution/entities.rs` (`normalize_entity_name`, the §3.4.3 decision
  algebra), `resolution/decision.rs` (`DecisionThresholds{merge=0.85}`),
  `resolution/signals.rs` (NameMatch is already case-folded via
  Jaro-Winkler — so investigate WHY caroline/Caroline did not merge
  despite name similarity ~1.0; likely embedding distance or occurred_at
  window pulled them apart below the 0.85 gate. That diagnosis is AC-0
  before choosing approach A vs B).

## VERIFICATION (2026-06-02 post-fix) — fix bb94f337 INSUFFICIENT, q0 still FAILS

Post-fix q0 confirmation arm (run `2026-06-02T17-31-32Z_locomo`, ISS-190
envelope conv-26 R=5 surface=on):

- **conv-26-q0: FAIL, score 0.0** (predicted "I don't know. The memories
  mention Caroline being part of a group and receiving support, but they
  don't specify when she went to an LGBTQ support group.")
- **overall: 0.2961** — slightly DOWN from the 0.3026 pre-fix baseline
  (within re-ingest noise, but NOT the improvement expected).

### Autopsy (post-fix DB `.tmptVtS8h/substrate.db`) — split SURVIVES

The fix made the resolution-path id *deterministic* (random `d7f9a67a` →
stable UUIDv5 `3e2fb6bb-a831-5dda-...`) but did **not unify it with the
legacy FNV node**. Both Caroline nodes still exist:

- `1d11ce4c7b5c12d8` — `caroline`, `{entity_type:person}`, FNV-16hex id.
  **0 occurred_on, 98 incoming mention edges.** Anchor resolver lands here.
- `3e2fb6bb-a831-5dda-defb-c11153815661` — `Caroline`,
  `{_legacy_kind:person,kind_source:DictionaryMatch}`, deterministic
  UUIDv5 (the fix). **31 occurred_on (incl gold 2023-05-07), 209 outgoing.**

### Why the fix could never work

Path (1) `memory.rs` add/enrich → `storage.upsert_entity()` (call sites
memory.rs:3044/3115/3200/6813) → `generate_entity_id()` = **FNV-1a 64-bit
→ 16-hex string**. Path (2) resolution pipeline → `stage_persist` →
`deterministic_entity_id()` = **SHA-256 → UUIDv5**. Both key on
`lowercase(name)|kind|ns`, but **FNV-16hex and SHA256-UUID are different
functions producing different id formats** — they can NEVER collide.
Making path (2) deterministic does nothing to merge it with path (1). The
AC-0 "sub-option (i)/(ii) reconcile the two paths, decide during
implementation" half was the load-bearing part and was skipped.

### Corrected root fix (decision needed)

The two writers must emit the **same id** for the same canonical key:
- **(i)** `storage.upsert_entity` adopts the same UUIDv5 as the pipeline
  (changes legacy node id format 16-hex → UUID; touches the
  `UNIQUE(name,entity_type,namespace)` index and every FNV-keyed row;
  needs historical-DB migration but fix-forward suffices for the bench).
- **(ii)** resolution pipeline looks up the existing legacy entity id (by
  `generate_entity_id` on the case-folded name) before minting, and
  adopts it when present — keeps legacy 16-hex format as canonical.
- **(iii)** workaround: anchor resolution gathers edges across all
  same-canonical-key nodes (leaves split in place).

bb94f337 stays (deterministic > random is strictly better and harmless),
but it does not satisfy AC-1/AC-3. Issue remains **open**.

## ROOT FIX (2026-06-02, commit ce9075fd) — unify id across BOTH writers

The insufficient fix (bb94f337) touched only the resolution pipeline. The
root fix introduces ONE shared id primitive both writers call:

```
graph::canonical_entity_id(name: &str, namespace: &str) -> Uuid
// key = lowercase(name.trim()) | namespace ; SHA-256 -> first 16 bytes -> UUID
```

- `storage::generate_entity_id` (legacy add/enrich path) → delegates; its
  16-hex FNV id is gone, now emits the shared UUID. `entities.id` is
  `TEXT PRIMARY KEY` (no format constraint) so no schema/index migration.
- `resolution::pipeline::deterministic_entity_id` (CreateNew mint) →
  delegates; kind argument retained for call-site compat but ignored.

**Identity contract decision (potato delegated):** key on
`lowercase(name)|namespace` only — kind is NOT part of the id. The two
writers classify with divergent taxonomies (`entities::EntityType` =
project/person/technology/concept/file/url/org vs `graph::EntityKind` =
person/organization/place/other) stored in different attribute shapes
(`entity_type` vs `_legacy_kind`), so a kind-in-key scheme would silently
re-split entities whenever they disagree. Data confirms the cost is
near-zero: in the failing-fix DB, 687 entity nodes → 684 distinct
lowercase names; all 3 collisions (caroline/melanie/go) are the SAME
entity we want merged — no real same-name/different-kind cases. Person
"Mercury" vs Place "Mercury" collapsing is the accepted, documented
tradeoff; kind disambiguation belongs in a later resolution pass.

Tests: 2126 lib + entity_integration + iss072/120/122/123 integration all
green. 11 ISS-209 unit tests (4 on `canonical_entity_id`, 7 on the pipeline
wrapper) assert case-fold collapse, namespace discrimination, UUID format,
and kind-not-in-identity across taxonomies.

Re-running conv-26 q0 confirmation arm (STAMP 20260602T174640Z) to verify
the split is gone end-to-end and q0 flips 0→1.
