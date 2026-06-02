---
title: Generation ignores dated gold line at rank 3 (distractor saturation) — q0 retrieval delivers but answer is 'I don't know'
status: todo
priority: 2
labels: generation, synthesis, retrieval-ok, conv-26-q0
relates_to: ISS-210, ISS-205, ISS-204
---

# ISS-211: generation drops the dated gold line when higher-scored distractors crowd top-K

## Summary

After ISS-210 fixed anchor resolution, conv-26-q0 retrieval is now
**fully correct** — the gold dated episode is delivered into top-10 with
its resolved date visible — yet the generation still answers
"I don't know." The remaining defect is entirely in the
generation/synthesis layer.

## Evidence (run 2026-06-02T20-07-10Z_locomo, binary e9f4a247, DB .tmp6uuC9p)

`iss207_q0_delivery_probe` top-10 for
"When did Caroline go to the LGBTQ support group?":

```
[0] 0.8201 812c1a90  Caroline is inspiring others through her journey
[1] 0.8036 c201d93a  [2023-10-13] Caroline was inspired by the energy, support...
[2] 0.8194 9b642a7f  Caroline struggled with mental health and received h...
[3] 0.7942 9fff4171  [2023-05-07] Caroline attended a LGBTQ support group  <== GOLD
[4] 0.8045 6b5d9f93  Caroline credits her friends, family, and mentors...
...
```

Gold is in context (rank 3) WITH the date. Probe verdict:
"retrieval DELIVERS the dated gold episode into top-10."

But the generated answer:
> "I don't know. The memories mention Caroline speaking up for the trans
> community and receiving support, but they don't specify when she
> attended an LGBTQ support group."

The model read ranks 0–2 (higher-scored Caroline memories that don't
mention the support group), concluded no answer exists, and ignored the
exact-match dated line at rank 3.

## Root cause hypothesis

**Distractor saturation.** When the top of the candidate list is
dominated by semantically-adjacent but non-answering memories about the
same entity, the generation model anchors on them and discounts a
lower-ranked exact match. The gold line scores 0.7942 vs distractors at
0.80–0.82, so the relevance signal does not surface the answer to the top.

## Candidate fixes (pick after probe)

- **Re-rank dated gold for date-asking queries**: when the query
  `asks_for_date()` (already detected — query_classifier.rs) AND a
  reserved `occurred_on` edge exists, promote the reserved dated memory
  to the TOP slot, not just into the pool. (Extends ISS-205's reservation
  from "admit" to "prioritize" for date-asking queries.)
- **Generation prompt hardening**: instruct the model to scan for explicit
  dated lines (`[YYYY-MM-DD]`) matching the question subject before
  answering "I don't know".
- **Cross-encoder re-rank** on the date-asking subset.

## Acceptance criteria

- [ ] AC-1: conv-26-q0 flips 0→1 (gold dated line is used in the answer).
- [ ] AC-2: no regression on the other conv-26 single-hop / temporal
      questions (aggregate ≥ prior run within ingest-noise band).
- [ ] AC-3: the fix targets the date-asking subset specifically, not a
      blanket re-rank that disturbs non-temporal queries.

## Why this is separate from ISS-210

ISS-210 (entity nodes-projection NULL last_seen) was a **retrieval** bug —
the anchor never resolved, so the gold edge was never reserved. That is
fixed and proven (gold now at rank 3 in top-10). ISS-211 is a
**generation** bug — the gold is delivered but not used. Different layer,
different fix.
