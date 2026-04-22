# Engram Learnings & Operational Notes

> Shared with TS version. See `../agent-memory-prototype/engram-ts/LEARNINGS.md` for full details.

## Production Issues — RustClaw (2026-03-31)

> Full investigation: `INVESTIGATION-2026-03-31.md`

### Garbage Memory Accumulation
- **1,434 total memories**, ~190 (13%) are garbage
- Root cause: `EngramStoreHook` stores ALL agent replies including heartbeat status checks
- Haiku extractor can't distinguish system instructions from knowledge → extracts heartbeat directives as procedural memory with importance 0.7-0.9
- No dedup → same heartbeat instruction stored 10+ times

### Recall Accuracy Degradation
- Garbage memories compete with real memories in ACT-R ranking
- High-importance heartbeat garbage (0.9) outranks real memories (0.6)
- Repeated entries get elevated ACT-R activation (more access_log entries)
- No embedding configured → recall is FTS5 keyword matching only, no semantic search

### Key Takeaways for Engram Development
1. **Extractor needs negative examples** — system instructions, status reports, error logs should be filtered
2. **Need dedup in `add()`** — either content hash or similarity check before insert
3. **Recall needs post-processing dedup** — avoid returning 5 nearly-identical results
4. **Importance calibration** — auto-extracted facts should have lower ceiling than manual storage
5. **Auto-store design flaw** — "store everything" assumption is wrong; need filtering or let agent decide

---

## Rust-Specific Notes (2026-03-15)

### Schema Incompatibility
- Rust binary expects `namespace` column in `hebbian_links` table (added in Phase 1)
- Existing DBs created by TS/Python version don't have this column
- Binary starts in 13ms but fails on schema init
- **Need**: Migration command or graceful fallback for missing columns

### Priority for Rust CLI
1. Schema migration (ALTER TABLE ADD COLUMN IF NOT EXISTS)
2. `engram reindex` command (re-embed all memories with current provider)
3. Ollama embedding integration (match Python CLI capability)
4. Benchmark: target <50ms for recall with 768d embeddings on 6K memories

---

## Recall Quality Improvements (2026-04-05)

> From RustClaw production usage. These are engramai crate-level fixes.

### 1. Recency Bias in Recall Ranking
- **Problem**: ACT-R activation doesn't adequately weight recency. A memory accessed 10 times 3 months ago outranks a memory accessed 2 times yesterday, even when recency matters more.
- **Fix**: Add time-decay factor to activation calculation. Recent memories should get a boost. ACT-R base-level learning equation already has a decay parameter `d` — verify it's being applied correctly and tune the value.
- **Priority**: High — directly affects recall usefulness in daily agent operation.

### 2. Confidence Score Calculation
- **Problem**: Recall results don't have a meaningful confidence score. Hard to distinguish "highly relevant match" from "vaguely related noise".
- **Fix**: Compute confidence from multiple signals: embedding similarity, ACT-R activation, recency, keyword overlap. Normalize to 0.0-1.0 range. Return with each recall result so consumers can filter/threshold.
- **Priority**: High — enables downstream filtering (e.g., "only show memories with confidence > 0.7").

### 3. Recall Result Dedup (repeated from above, still not fixed)
- **Problem**: Same information stored N times → recall returns N near-identical results, wasting context window.
- **Fix**: Post-retrieval dedup — cluster results by content similarity, return only the highest-activation representative from each cluster.
- **Priority**: Medium — workaround exists (caller can dedup), but should be built-in.

### 4. [RustClaw-side] Auto-recall Flag for LLM Awareness
- **Problem**: When engram recall results are injected into system prompt, LLM sometimes ignores them or doesn't realize they indicate prior work.
- **Fix**: NOT an engramai fix — this is RustClaw system prompt formatting. When recall finds relevant memories, prepend with `⚠️ Relevant prior memory — you may have already done this:` to make the LLM pay attention.
- **Priority**: Medium — improves agent behavior, not recall quality itself.
- **Owner**: RustClaw (src/agent.rs or system prompt assembly)
