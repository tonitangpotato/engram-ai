---
title: Storage::delete_embedding skips node_embeddings — single-model DELETE asymmetric with delete_all_embeddings
status: fixed
priority: P1
labels:
- v04-unified-substrate
- phase-b
- dual-write
relates_to:
- ISS-115
- ISS-121
- ISS-124
fixed_by:
- 965b747
---

## Problem

`Storage::delete_embedding(memory_id, model)` in `crates/engramai/src/storage.rs:4185` deletes from legacy `memory_embeddings` but does **not** delete from the corresponding `node_embeddings` row, even though every other embedding op in the same file dual-writes:

- `store_embedding` (line 3812): INSERT OR REPLACE on both tables, in a transaction. ✅
- `delete_all_embeddings` (line 4201): DELETE FROM both tables, in a transaction. ✅
- `delete_embedding` (line 4185): DELETE FROM `memory_embeddings` only. ❌

The asymmetry leaves a phantom `node_embeddings` row whose parent legacy embedding is gone. Under `unified_substrate=true`, `get_embedding(id, model)` returns the orphaned vector; under legacy, it returns `None`. The two substrates disagree.

## Reproducer

```rust
storage.store_embedding("m-1", &vec![0.1; 384], "model-a", 384)?;
storage.delete_embedding("m-1", "model-a")?;

// Unified read still returns the embedding (orphan in node_embeddings):
let unified = Storage::with_unified_substrate(path, true)?;
assert_eq!(unified.get_embedding("m-1", "model-a")?, None); // FAILS — returns Some(vec)
```

## Root cause

Same family as ISS-121 / ISS-124. The single-row DELETE path was written before T20 introduced `node_embeddings`, then never updated when the dual-write contract was established. `delete_all_embeddings` was added later (per its docstring referencing ISS-115) and got the dual-DELETE right, but `delete_embedding` was missed.

## Fix plan

Make `delete_embedding` mirror `delete_all_embeddings`: wrap both DELETEs in a single transaction.

```rust
pub fn delete_embedding(&mut self, memory_id: &str, model: &str) -> Result<(), rusqlite::Error> {
    let model = Self::normalize_model_id(model);
    let tx = self.conn.transaction()?;
    tx.execute(
        "DELETE FROM memory_embeddings WHERE memory_id = ? AND model = ?",
        params![memory_id, model],
    )?;
    tx.execute(
        "DELETE FROM node_embeddings WHERE node_id = ? AND model = ?",
        params![memory_id, model],
    )?;
    tx.commit()?;
    Ok(())
}
```

No FK guard needed — DELETE on a missing row is a 0-rows-affected no-op (matches the `delete_all_embeddings` behavior on rows without unified counterparts).

## Contract tests

Under `crates/engramai/tests/iss125_delete_embedding_dual_write.rs`:

1. **iss125_delete_embedding_clears_both_tables**: store on both substrates via `store_embedding`, call `delete_embedding`, assert `get_embedding` returns `None` on **both** legacy and unified.
2. **iss125_delete_embedding_only_removes_matching_model**: store two different models for the same memory, delete one, assert the other survives on both substrates.
3. **iss125_delete_embedding_idempotent**: call delete twice — second call must be a clean no-op.
4. **iss125_delete_embedding_normalizes_model**: pass model without provider prefix; assert both tables match the normalized form (parity with store_embedding).

## Out of scope

- Other embedding ops (`get_embedding`, `get_memories_without_embeddings`) already correctly switch via `self.unified_substrate` flag — those are read paths and not affected.

## Acceptance criteria

- [ ] `delete_embedding` wraps both DELETEs in a transaction (mirrors `delete_all_embeddings`)
- [ ] 4 contract tests pass
- [ ] 1902/1902 lib tests still pass
- [ ] Phase B/D peer tests all still pass
