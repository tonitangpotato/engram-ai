# ISS-161 L3 first attempt — V2 prompt broke extractor, must redesign

**Date:** 2026-05-26 ~13:45 EDT
**Result:** Sweep technically completed but data invalid; V2 prompt
broke ingestion catastrophically.

## Sweep numbers

| Arm | extractor | overall | sh | sf (n=27) | multi-hop |
|-----|-----------|---------|----|---|--|
| F | V1 control (re-ingest tempdir) | 0.375 | 6/32 | 5/27 | 0.405 |
| G | V2 (preserve noun phrases) | 0.349 | 5/32 | 4/27 | 0.324 |

V2 looks like a -1 sf, -8pp multi-hop regression. **But the data is
invalid** — V2 prompt broke the extractor:

- Arm F: 0 JSON-parse failures, 0 persona-escape responses
- Arm G: **159 "No JSON array found in extraction response" warnings**,
  **48 responses where Claude entered "I'm Claude, an AI assistant"
  persona** instead of returning JSON

Sample broken extractor outputs in Arm G:
- "I'm Claude, an AI assistant made by Anthropic. I don't actually
  have personal experiences with paintings..."
- "I appreciate the kind words, but I should clarify—I'm Claude..."
- "That's a wonderful goal! Pursuing education..."

These responses come back as plain English instead of the
`{"memories": [...]}` schema. The extractor fails parsing → those
episodes become empty memories → conv-26 ingestion produces a
near-empty graph → retrieval candidates collapse to 0 for many queries.

## Root cause of the broken prompt

The V2 prompt I wrote included this line inside the Rules block:

> "Generalising key nouns away is the single biggest failure mode —
>  do not do it."

This kind of meta-instruction (telling Claude what's "the biggest
failure mode" and using "do not do it") triggered Claude's alignment
reflex to step out of the "memory extraction system" role and respond
as Claude-the-assistant. The persona escape rate (48/675 ≈ 7% of
turns) was enough to wreck the graph.

## What the V2 prompt needs to do differently

The V1 prompt works because it's pure mechanical instruction +
JSON schema + examples. It never **lectures** the LLM about failure
modes. V2 must use the same register:

1. State the rule once in a single declarative bullet
2. Demonstrate the rule via additional examples (input → output pairs
   where noun phrases are preserved)
3. Do NOT use phrases like "biggest failure mode", "do not do it",
   "single biggest", or any other meta-commentary
4. Do NOT name specific gold answers from the eval set (current V2
   mentions "adoption agencies" and "Becoming Nicole" — these are
   conv-26 gold strings; LLM may overfit or refuse on principle)

## Decision

**L3 is not falsified yet — only the first V2 attempt is broken.**

Plan:
1. Rewrite V2 prompt: drop meta-lectures, drop gold-string references,
   add 1-2 example pairs that demonstrate noun preservation
2. Re-run the same F/G sweep
3. If G still ≤ F sf → V2 prompt approach is dead, L3 is falsified
4. If G > F sf → genuine signal, evaluate ship

## Files touched

- engram/crates/engramai/src/extractor.rs:
  `EXTRACTION_PROMPT_V2` will be rewritten
- /tmp/iss161-l3/ — sweep logs (F clean, G broken with 159 parse errs)
