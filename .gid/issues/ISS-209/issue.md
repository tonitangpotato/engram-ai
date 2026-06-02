---
id: ISS-209
title: Case-fold entity split (caroline vs Caroline) survives ISS-203 — reservation works only because anchor resolution lands on the edge-owning node by luck; fragile + aggregate-suppressing
status: open
priority: P1
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

## Notes

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
