# Metadata Channel: Opaque Side-Channel for Caller-Owned Facts

> **Status**: Design doc for engram v0.2.3+
> **Audience**: engram integrators (adapter authors, binding maintainers)
> **Last updated**: 2026-04-22

## TL;DR

Engram exposes **two parallel storage channels** on every memory:

- **`content`** (semantic channel) — natural language, LLM-rewritten, embedded, participates in recall
- **`metadata`** (opaque channel) — structured key-value, **zero interpretation by engram**, passed through verbatim

This document defines what belongs in each channel and why.

---

## Why Two Channels?

Engram's core value is **LLM-driven cognitive processing**: fact extraction, clustering, Hebbian co-activation, Ebbinghaus decay. This means `content` gets **rewritten**:

- Stored: `"Caroline: I want to start a podcast about urban gardening"`
- Extracted fact: `"Caroline wants to start a podcast"`

Rewriting is a **feature** — it enables semantic aggregation across conversations, cross-memory linking, compression. But rewriting is also **lossy**: any identifier, anchor, or precise attribute embedded in raw content can be dropped by the LLM.

The metadata channel exists to preserve caller-owned facts that **must not be touched** by engram's cognitive pipeline.

---

## What Belongs in `metadata`

A piece of information belongs in `metadata` if it satisfies **any one** of the following:

### 1. External Identifiers

IDs owned by systems outside engram, used by downstream consumers for lookup/routing.

- `dia_id` — locomo benchmark turn identifier (needed by evaluator)
- `message_id`, `chat_id`, `reply_to_id` — chat system references
- `file_path`, `line_number` — source-code anchors
- `tweet_id`, `thread_id` — social-media references

**Why not content**: engram's LLM doesn't recognize these as semantically meaningful and will drop them during extraction.

### 2. Structured Attributes Already Known to Caller

Attributes the caller knows precisely and does not need engram to infer.

- `speaker` — chat context already knows who said it
- `timestamp` — system clock provides this
- `source_type` — `"chat" | "email" | "doc" | "benchmark"`
- `session_num`, `turn_index` — discrete position markers

**Why not content**: embedding structured attributes in prose pollutes semantic matching ("Caroline said at 2023-01-15 14:00 that...") and asking the LLM to re-derive them is wasteful and unreliable.

### 3. Immutable Anchors to Raw Source

Pointers back to original unprocessed material, to be preserved byte-for-byte.

- `raw_text` — original utterance (if truly needed)
- `paragraph_offset` — position within long document
- `source_url` — canonical origin URL

**Why not content**: `content` will be LLM-rewritten; raw anchors must survive verbatim.

### 4. Pre-Computed Business Tags

Domain-specific labels the caller computes more accurately than engram could guess.

- `priority: "P0"`
- `sentiment: 0.8` (from external sentiment service)
- `project_tag: "RustClaw"` (caller knows from session context)
- `participant_role: "user" | "agent"`

**Why not content**: engram's general-purpose LLM may not match domain-specific labeling rules.

---

## What Does **Not** Belong in `metadata`

Equally important — keep these in `content`:

- **Semantically meaningful statements** — `"Caroline likes urban gardening"` is a fact that must participate in recall
- **Information requiring fuzzy/semantic matching** — searching for `"hobby"` should match `"urban gardening"`; this only works if the content is embedded
- **Human-readable context** — anything a downstream LLM needs to see when reasoning over recall results

---

## Decision Flowchart

For each piece of information to store, ask:

```
Q1: Does this need to participate in semantic similarity matching?
    Yes → content
    No  → continue to Q2

Q2: Is this an external ID, anchor, or pointer?
    Yes → metadata
    No  → continue to Q3

Q3: Does the caller already know this precisely (no LLM inference needed)?
    Yes → metadata
    No  → continue to Q4

Q4: Would embedding this in content pollute LLM extraction quality?
    Yes → metadata
    No  → content is acceptable (metadata preferred for cleanliness)
```

---

## Engram's Contract

Engram makes **three** promises about the `metadata` channel:

1. **Opaque pass-through** — metadata is stored as `serde_json::Value` and returned on recall exactly as provided. Engram does not parse, validate, mutate, or interpret it.
2. **Not used for extraction** — the LLM extractor never sees metadata. It only sees `content`.
3. **Not used for clustering/compilation** — Knowledge Compiler operates on `content`, `tags`, and `embeddings`. Metadata does not influence topic synthesis.

Future capability (not in v0.2.3): **metadata field indexing** — e.g., `recall where speaker='Caroline'`. This would require schema declaration and index maintenance. Out of scope until a second adapter demonstrates need.

---

## Caller's Responsibility: Schema Location

**Engram does not enforce a metadata schema.** Each caller owns its own schema. Best practice:

### Pattern A (Recommended): Adapter Class Holds Schema

Every integration scenario gets a dedicated adapter class. The metadata schema lives in one place — the adapter's docstring and/or typed interface.

```python
class LocomoAdapter:
    """Adapter for locomo benchmark ingestion and evaluation.

    Metadata schema:
      - dia_id (str, required)  — benchmark turn identifier
      - speaker (str)           — utterance speaker
      - session_date (str)      — ISO timestamp
      - session_num (int)       — session number within dialogue
    """

    def store_turn(self, turn: Turn, session: Session) -> None:
        meta = {
            "dia_id": turn.dia_id,
            "speaker": turn.speaker,
            "session_date": session.timestamp.isoformat(),
            "session_num": session.num,
        }
        self._memory.store(content=turn.text, meta=meta)
```

**Consequences**:
- Business code calls `adapter.store_turn(turn)` — never touches `meta` dict directly
- Schema drift impossible because all writes go through one method
- Adding a new scenario (chat, doc, email) = new adapter, zero engram changes

### Pattern B (Future): Engram-Side Schema Declaration

If three or more adapters accumulate similar boilerplate, engram may introduce:

```rust
engram.define_namespace_schema("locomo", json!({
    "dia_id":  {"type": "string", "required": true},
    "speaker": {"type": "string"},
    // ...
}));
```

**Not implemented in v0.2.3**. Revisit when duplication becomes real.

---

## Complete Example: Future Telegram Adapter

```python
telegram_adapter.store(
    content="potato: 这个设计是不是 over-engineered?",
    # ↑ content: semantic, LLM-rewritable
    # Extracted fact may become: "potato questioned whether design is over-engineered"
    meta={
        # External IDs (Rule 1)
        "chat_id":     "7539582820",
        "message_id":  12345,
        "reply_to_id": 12344,

        # Structured attributes (Rule 2)
        "speaker":   "potato",
        "timestamp": "2026-04-22T00:57:00-04:00",
        "source":    "telegram",

        # Pre-computed business tags (Rule 4)
        "project_context":        "engram-locomo-benchmark",
        "conversation_thread_id": "design-discussion-20260422",
    },
)
```

On recall, the adapter can:
1. Read rewritten fact from `content` for LLM reasoning
2. Read original `message_id`/`reply_to_id` from `metadata` to reply in Telegram
3. Filter by `speaker` or `project_context` for structured queries (once field-level indexing lands)

---

## Knowledge Compiler Interaction

**Knowledge Compiler does NOT read `metadata`.** It operates on `MemorySnapshot { id, content, memory_type, importance, created_at, updated_at, tags, embedding }`.

This is intentional:
- Metadata often contains business-specific IDs (`dia_id`, `message_id`) that should not influence topic synthesis
- Topics are semantic aggregations; semantics live in `content` and `tags`
- Keeping KC metadata-agnostic preserves the separation: engram cognition vs. caller business

If a scenario needs KC to see certain structured fields (e.g., aggregate by `project_tag`), the correct path is to either:
1. Add the field to the content itself (if semantically meaningful)
2. Use `tags: Vec<String>` on the memory (already exposed to KC)
3. Propose a future KC extension that opts into specific metadata keys

---

## API Summary (v0.2.3)

### Rust
```rust
memory.add_with_metadata(
    "Caroline wants to start a podcast",
    MemoryType::Factual,
    0.6,
    json!({"dia_id": "D1:3", "speaker": "Caroline"}),
).await?;
```

### Python binding
```python
memory.store(
    content="...",
    memory_type="factual",
    importance=0.6,
    meta={"dia_id": "D1:3", "speaker": "Caroline"},
)
```

### CLI
```bash
engram store "Caroline wants to start a podcast" \
    --type factual \
    --importance 0.6 \
    --meta dia_id=D1:3 \
    --meta speaker=Caroline
```

### Recall
Metadata is returned on every recall result:
```python
results = memory.recall("Caroline's hobbies")
for r in results:
    print(r.content)         # LLM-rewritten fact
    print(r.metadata)        # Original caller-provided dict
```

---

## Migration Notes

- Pre-v0.2.3 callers storing structured data in `content` (e.g., `"[D1:3] Caroline: ..."`) can migrate incrementally — existing data remains readable, new writes use `meta`
- Engram does not auto-migrate old content-embedded IDs into metadata; callers with a need for backfill must implement it themselves
- Removing `source_text` duplicate storage (v0.2.2 experiment) is consistent with this design: use `meta={"raw_text": ...}` if truly needed, don't make it an engram default

---

## FAQ

**Q: Can I search/filter by metadata fields?**
A: Not in v0.2.3. All filtering happens post-recall on the caller side. Field-level indexing is a future feature.

**Q: Does metadata affect Hebbian linking?**
A: No. Links form based on co-activation of recalls, independent of metadata values.

**Q: Does metadata count toward memory importance or decay?**
A: No. Decay and importance are functions of access patterns and caller-provided importance scores, not metadata content.

**Q: What types are allowed in metadata values?**
A: Any `serde_json::Value` — strings, numbers, booleans, arrays, nested objects. Avoid non-JSON types.

**Q: How large can metadata be?**
A: No hard limit, but keep it small. Metadata is loaded on every recall. If you have >1KB of structured data per memory, reconsider whether it belongs in a separate store.
