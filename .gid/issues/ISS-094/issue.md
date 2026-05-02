---
id: ISS-094
title: cogmembench adapter drops temporal.value at recall (only feeds bare content to LLM)
status: done
priority: high
labels:
- eval
- cogmembench
- temporal
- quick-win
created: 2026-05-01
relates_to:
- ISS-088
- ISS-093
---

# cogmembench adapter drops temporal.value at recall

## Problem

ISS-088 added temporal grounding at the **extraction** layer (engramai correctly stamps `metadata.engram.dimensions.temporal.value = "yesterday (2023-05-07)"` when extracting facts).

But the cogmembench LoCoMo adapter's `recall_for_question` (in `cogmembench/benchmarks/locomo/engram_adapter.py`, around line 379) renders memories to the LLM as:

```
[{memory_type}] [activation=X] [confidence=Y] {content}
```

Where `content` is the bare extracted text ("Caroline attended a LGBTQ support group"). It reads `metadata.user.dia_id` (for evidence-match scoring) but **does NOT read `metadata.engram.dimensions.temporal.value`**. So the temporal grounding we worked to extract is dropped before the LLM ever sees it.

This is why LoCoMo cat=2 (multi-hop temporal) is hard — the LLM gets memories with no dates attached.

## Evidence (verified 2026-05-01)

- Adapter file: `/Users/potato/clawd/projects/cogmembench/benchmarks/locomo/engram_adapter.py`, function `recall_for_question` at line 359, the f-string that drops temporal is at line 399-402.
- RUN-0012 metadata spot-check (locomo-conv26-full.db, 441 memories):
  - 441/441 have `metadata.engram.dimensions`
  - **170/441 (38%) have a `temporal` dimension** — not all, because not every memory has explicit time anchoring
  - Format confirmed: `metadata.engram.dimensions.temporal = {"kind": "vague", "value": "yesterday (2023-05-07)"}`
  - Other observed `value`s: `"ongoing"`, `"future/planned"` — non-date strings still useful as time-orientation hints to the LLM
  - `kind` field is structural metadata; only `value` should be rendered to the LLM

## Fix (estimated ~15 lines)

In `recall_for_question`, after pulling `dia_id`, also pull:

```python
temporal_str = None
engram_md = md.get("engram") if isinstance(md, dict) else None
if isinstance(engram_md, dict):
    dims = engram_md.get("dimensions", {})
    temporal = dims.get("temporal") if isinstance(dims, dict) else None
    if isinstance(temporal, dict):
        temporal_str = temporal.get("value")  # e.g. "yesterday (2023-05-07)"
```

Then format:

```python
date_marker = f" [date={temporal_str}]" if temporal_str else ""
context_parts.append(
    f"[{memory_type}]{date_marker} [activation={activation:.2f}] [confidence={confidence:.2f}] {content}"
)
```

Possibly also surface `spatial.value` if present (same metadata side channel).

## Acceptance

- [x] `recall_for_question` reads `metadata.engram.dimensions.temporal.value` when present (verified in `benchmarks/locomo/tests/test_iss094_temporal_inline.py`)
- [x] Date marker rendered into context_parts so judge LLM sees it (unit test passes)
- [x] No regression on ISS-088 dia_id pass-through (asserted in same unit test)
- [x] Smoke test on 5 cat=2 temporal questions: confirm date appears in formatted context (verified 2026-05-01 via `cogmembench/scripts/smoke_iss094_date_marker.py` against RUN-0012; 5/5 questions show `[date=...]` markers from extracted-fact records)
- [ ] Re-run RUN-0012 retrieve+judge subset (cat=2 only, ~30 questions): record cat=2 J-score before/after — **deferred** (LLM cost; substrate fix verified, J-score delta is a separate eval task)

## Resolution

Patch landed in cogmembench `604a88a` (recall-side temporal inline). Smoke evidence committed in cogmembench `4fe0913` (`scripts/smoke_iss094_date_marker.py`).

Note: smoke surfaced an adjacent finding — raw dialogue turns in RUN-0012 lack `metadata.engram.dimensions.temporal` entirely (only LLM-extracted facts have it). That's an ingestion-quality concern (some records bypass extraction), not an ISS-094 scope issue. Filing separately if cat=2 J-score remains poor after this lands.

## Out of scope

- Changing engramai retrieval API itself (this is downstream-only)
- Architectural fix for two-layer split (see ISS-096)
- Surfacing other dimensions (`participants`, `tags`, `spatial`) to the LLM — separate issue if cat=2 fix isn't enough
- Eval pipeline tooling debt (see ISS-097)

## Estimated effort

30 min code + 30 min validation run = **~1 hour**.
