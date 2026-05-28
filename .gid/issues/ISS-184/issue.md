---
title: 'B-bucket activation — Bitemporal plan: dispatches but temporal category weakest; no bitemporal-specific A/B'
status: open
priority: P3
severity: feature-inert
category: cognitive-substrate
created: 2026-05-28
relates: [engram:ISS-181, engram:ISS-179]
relates_to: .gid/issues/ISS-181/issue.md
discovered_in: ISS-181 cognitive feature coverage matrix
---

## Summary

The bitemporal plan exists in the dispatcher and gets selected
for a non-trivial share of LoCoMo conv-26 queries (the "temporal"
category, ~13–14% of the corpus). Plan routing works; the bitemporal
code path executes.

But across every clean A/B we've run on conv-26 (ISS-137 baseline,
ISS-138 K=10, ISS-139 MMR, ISS-147/150 BM25 wirings, ISS-152 pool
sweep, ISS-153 HyDE, ISS-161 prompt variants, ISS-175 factual
reweight, ISS-178 prev-turn-context), the **temporal category is
consistently the weakest non-list bucket** (0.30–0.50 range vs
single-hop 0.20–0.50 and multi-hop 0.35–0.65), and **no bitemporal-
specific A/B has been run** — every lever we've tested either
moved temporal flat or moved it in lockstep with the other
categories (i.e. ranking/retrieval-side, not time-resolution-side).

The substrate ships `valid_from` / `valid_until` / `as_of` /
`occurred_at` fields on every memory, but the bitemporal plan
does not appear to use a **bitemporal-specific scoring channel**
that rewards temporal proximity between the query's resolved
date and the candidate memory's `occurred_at`. Without that
channel, the bitemporal plan is functionally indistinguishable
from the factual plan on temporal queries — it picks up the
right memories only when text/entity overlap already would have.

This means the bitemporal substrate is **read at write time
(field population)** and **read at routing time (plan dispatch
based on temporal keywords)** but **not read at scoring time**.
Coverage matrix bucket B: real but inert.

## What it would take to make bitemporal production-active

Two paths, not exclusive:

### Path A — Bitemporal-specific scoring channel

Add a `temporal_proximity` signal to the bitemporal plan's
fusion that scores candidate `occurred_at` against the query's
resolved temporal anchor:

1. Add query temporal resolution (parse "last month", "in 2024",
   "before the trip" into a `TemporalAnchor { date, granularity,
   relation }`). Path B below produces this signal cleanly;
   without it Path A degrades to keyword match.
2. In the bitemporal fusion combine step, add a `temporal`
   weight channel: `score = base + w_t * proximity(candidate.occurred_at, query.anchor)`.
3. Default the channel weight low (0.05–0.10) so it doesn't
   disturb the locked envelope.
4. A/B sweep on conv-26 + conv-44 with the temporal sub-bucket
   isolated (need ≥30 temporal queries for stable signal — conv-26
   alone has ~20, may need conv-44 + cross-validation).

Risk: low (additive, weight-bounded, flag-gated). Failure mode is
that LoCoMo temporal queries cluster on a small date range so
proximity scoring degenerates to "everything is close enough" —
which would be itself a useful negative finding.

### Path B — Upstream temporal resolver improvements

Build a query-side temporal resolver that maps natural-language
time expressions in the query into the same canonical anchor
the substrate stores. Without Path B, Path A's channel has
nothing to compare against. Without Path A, Path B's anchor
is unused. They compose.

Path B alone (no Path A) still wouldn't move the needle —
the bitemporal plan would have a cleaner anchor in hand but
nowhere to spend it. So Path B is a prerequisite, not an
independent activation lever.

## Acceptance criteria for activation

- [ ] AC-1 — Temporal sub-bucket isolated in bench harness:
  bitemporal-plan-dispatched queries vs all temporal-category
  queries, so we can distinguish routing accuracy from scoring
  accuracy.
- [ ] AC-2 — Bitemporal-specific A/B on conv-26 + conv-44
  with Path A (temporal proximity channel) shows ≥+5pp lift
  on the bitemporal-dispatched sub-bucket vs locked envelope.
- [ ] AC-3 — Path B query temporal resolver lands first;
  Path A wired against its output. Both flag-gated, both
  default-off until benched.

## Why P3 now

Three reasons this isn't worth promoting:

1. **No corpus that rewards bitemporal-specific scoring**: LoCoMo
   conv-26 temporal queries are mostly "when did X happen" /
   "what did Y do last month" — temporal proximity is a weak
   signal because most candidate memories cluster in a short
   date window. We'd be tuning against noise.

2. **ISS-179 may redefine the SF target away from conv-26**:
   if AC-5a moves to axis-level evaluation or a different corpus,
   the temporal sub-bucket gate changes entirely. Designing
   the bitemporal channel before we know the target is premature.

3. **Path B is the bottleneck**: query temporal resolution is
   its own non-trivial project (parsing, anchor canonicalization,
   handling relative references like "before the trip"). Without
   it, Path A has no input. Shipping Path A alone would burn
   cycles on a feature that can't be exercised.

Hold criteria for promotion to P2/P1:
- ISS-179 lands with a target that includes a temporal axis
  with ≥30 queries, OR
- A non-conv-26 corpus arrives with bitemporal-rewarding queries
  (date proximity scoring meaningfully separates good from bad
  candidates), OR
- Path B query temporal resolver lands as a side effect of
  some other work (then Path A becomes cheap and worth a sweep).
