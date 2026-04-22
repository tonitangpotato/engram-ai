# Engram Embedding Protocol Specification v2

**Status:** Canonical  
**Version:** 2.0  
**Last Updated:** 2026-04-02

---

## 1. Overview

This document defines the **Engram Embedding Protocol v2**, the authoritative specification for storing and managing vector embeddings across all Engram implementations (Rust, Python, and future languages).

All implementations **MUST** conform to this specification to ensure compatibility and data portability.

### 1.1 Key Changes from v1

- **Multi-model support:** Primary key changed from `(memory_id)` to `(memory_id, model)`
- **Binary-only storage:** BLOB format required; TEXT/JSON storage is prohibited
- **Strict validation:** Finite value enforcement and dimension checking
- **Model namespacing:** Structured provider/model naming convention

---

## 2. Database Schema

### 2.1 Table Definition

```sql
CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_id   TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    model       TEXT NOT NULL,
    embedding   BLOB NOT NULL,
    dimensions  INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (memory_id, model)
);

CREATE INDEX IF NOT EXISTS idx_embeddings_model ON memory_embeddings(model);
```

### 2.2 Column Specifications

| Column | Type | Constraints | Description |
|--------|------|-------------|-------------|
| `memory_id` | TEXT | NOT NULL, FK to memories(id) | UUID or unique identifier of the memory |
| `model` | TEXT | NOT NULL | Namespaced model identifier (see §3) |
| `embedding` | BLOB | NOT NULL | Raw binary f32 vector (see §4) |
| `dimensions` | INTEGER | NOT NULL | Vector dimension count (must match blob size) |
| `created_at` | TEXT | NOT NULL | ISO-8601 UTC timestamp |

### 2.3 Constraints

- **Primary Key:** Composite `(memory_id, model)` enables multiple embeddings per memory
- **Foreign Key:** `memory_id` references `memories(id)` with `ON DELETE CASCADE`
- **Index:** `idx_embeddings_model` optimizes model-scoped queries

---

## 3. Model Naming Convention

### 3.1 Format

Model identifiers **MUST** follow the pattern:

```
{provider}/{model_name}
```

### 3.2 Rules

1. **Immutability:** Once written, model strings are immutable (never update in place)
2. **Case-sensitivity:** Model names are case-sensitive
3. **No whitespace:** Use hyphens or underscores for multi-word names
4. **Provider namespace:** Prevents collisions across different embedding services

### 3.3 Canonical Models

| Model String | Dimensions | Provider |
|--------------|------------|----------|
| `ollama/nomic-embed-text` | 768 | Ollama (local) |
| `ollama/mxbai-embed-large` | 1024 | Ollama (local) |
| `openai/text-embedding-3-small` | 1536 | OpenAI |
| `openai/text-embedding-3-large` | 3072 | OpenAI |
| `openai/text-embedding-ada-002` | 1536 | OpenAI (legacy) |
| `local/minilm-l6-v2` | 384 | Local transformer |
| `cohere/embed-english-v3.0` | 1024 | Cohere |
| `voyage/voyage-2` | 1024 | Voyage AI |

### 3.4 Unknown/Legacy Models

Use `unknown/legacy` for migrated embeddings with unspecified models.

---

## 4. BLOB Format Specification

### 4.1 Binary Layout

Embeddings are stored as **raw little-endian IEEE 754 binary32 (f32) arrays**:

- **No header, no wrapper, no JSON**
- Byte length: `dimensions × 4`
- Endianness: Little-endian on all platforms

### 4.2 Serialization

**Rust:**
```rust
let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
```

**Python:**
```python
import struct
blob = struct.pack(f'<{len(embedding)}f', *embedding)
```

### 4.3 Deserialization

**Rust:**
```rust
fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}
```

**Python:**
```python
import struct
n = len(blob) // 4
embedding = list(struct.unpack(f'<{n}f', blob))
```

### 4.4 Validation Rules

Implementations **MUST** validate blobs before writing and after reading:

1. **Alignment:** `len(blob) % 4 == 0`
2. **Dimension match:** `len(blob) / 4 == dimensions`
3. **Finite values:** All f32 values must be finite (no NaN, no Inf)

**Validation pseudocode:**
```
function validate_blob(blob, expected_dimensions):
    assert len(blob) % 4 == 0, "Blob length must be multiple of 4"
    actual_dimensions = len(blob) / 4
    assert actual_dimensions == expected_dimensions, "Dimension mismatch"
    
    values = deserialize(blob)
    for value in values:
        assert is_finite(value), "Non-finite value detected"
```

---

## 5. Write Protocol

### 5.1 Insert/Update Query

```sql
INSERT OR REPLACE INTO memory_embeddings 
    (memory_id, model, embedding, dimensions, created_at) 
VALUES (?, ?, ?, ?, ?);
```

### 5.2 Parameter Binding

1. `memory_id`: String (UUID or identifier from `memories.id`)
2. `model`: String (following §3 naming convention)
3. `embedding`: BLOB (validated per §4.4)
4. `dimensions`: Integer (must equal `len(embedding) / 4`)
5. `created_at`: String (ISO-8601 UTC, e.g., `2026-04-02T05:26:34.123Z`)

### 5.3 Semantics

- `INSERT OR REPLACE` is correct: re-embedding the same (memory_id, model) pair updates the row
- Each write **MUST** validate the blob before insertion
- The `created_at` timestamp reflects the embedding generation time, not the memory creation time

### 5.4 Example

```rust
// Rust example
let timestamp = Utc::now().to_rfc3339();
let blob = serialize_embedding(&vector); // Per §4.2

db.execute(
    "INSERT OR REPLACE INTO memory_embeddings VALUES (?, ?, ?, ?, ?)",
    params![memory_id, "ollama/nomic-embed-text", blob, 768, timestamp]
)?;
```

---

## 6. Read Protocol

### 6.1 Single Memory Query

```sql
SELECT embedding, dimensions 
FROM memory_embeddings 
WHERE memory_id = ? AND model = ?;
```

Returns the embedding for a specific memory using a specific model.

### 6.2 All Embeddings for Model

```sql
SELECT memory_id, embedding 
FROM memory_embeddings 
WHERE model = ?;
```

Returns all embeddings generated with a specific model (used for recall).

### 6.3 Post-Read Validation

After reading, implementations **MUST** validate:

```
assert len(blob) == dimensions * 4
```

### 6.4 Cross-Model Comparison

**UNDEFINED BEHAVIOR:** Never compute cosine similarity between vectors from different models.

Implementations **MUST** filter by model before similarity computations.

---

## 7. Recall Protocol

### 7.1 Query Embedding

1. Embed the query text using the **currently configured model**
2. Retrieve all embeddings for that model only

### 7.2 Similarity Computation

```sql
SELECT memory_id, embedding 
FROM memory_embeddings 
WHERE model = ?;  -- Current model only
```

3. Deserialize each embedding
4. Compute cosine similarity against the query vector
5. Rank by similarity score

### 7.3 Coverage Check

Before recall, implementations **SHOULD** check embedding coverage:

```sql
SELECT 
    (SELECT COUNT(*) FROM memory_embeddings WHERE model = ?) AS embedded_count,
    (SELECT COUNT(*) FROM memories) AS total_count;
```

**Warning condition:** If `embedded_count / total_count < 0.5`, log:
```
WARNING: Only {percentage}% of memories have embeddings for model {model}.
Consider running backfill to improve recall quality.
```

### 7.4 Empty Result Handling

If no embeddings exist for the current model, return empty results (do not fall back to other models).

---

## 8. Migration Protocol (v1 → v2)

### 8.1 Detection

Detect v1 schema by inspecting table structure:
- Primary key is only `(memory_id)`
- May have `embedding` column of type TEXT

### 8.2 Migration Steps

```sql
-- Step 1: Create new table
CREATE TABLE memory_embeddings_v2 (
    memory_id   TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    model       TEXT NOT NULL,
    embedding   BLOB NOT NULL,
    dimensions  INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (memory_id, model)
);

-- Step 2: Migrate BLOB rows (if model column exists)
INSERT INTO memory_embeddings_v2 
    (memory_id, model, embedding, dimensions, created_at)
SELECT memory_id, model, embedding, dimensions, created_at 
FROM memory_embeddings 
WHERE typeof(embedding) = 'blob';

-- Step 3: Handle TEXT/JSON rows
-- (Implementation-specific: parse JSON, convert to BLOB)

-- Step 4: Handle missing metadata
-- For rows without model: use 'unknown/legacy'
-- For rows without dimensions: infer from blob size

-- Step 5: Replace old table
DROP TABLE memory_embeddings;
ALTER TABLE memory_embeddings_v2 RENAME TO memory_embeddings;

-- Step 6: Create index
CREATE INDEX idx_embeddings_model ON memory_embeddings(model);
```

### 8.3 TEXT to BLOB Conversion

**Python example:**
```python
import json
import struct

# Read TEXT embedding
text_embedding = row['embedding']
vector = json.loads(text_embedding)  # Parse JSON array

# Convert to BLOB
blob = struct.pack(f'<{len(vector)}f', *vector)
dimensions = len(vector)

# Insert into v2 table
cursor.execute(
    "INSERT INTO memory_embeddings_v2 VALUES (?, ?, ?, ?, ?)",
    (memory_id, model or 'unknown/legacy', blob, dimensions, created_at)
)
```

### 8.4 Version Tracking

After migration, update protocol version:

```sql
CREATE TABLE IF NOT EXISTS engram_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR REPLACE INTO engram_meta (key, value) 
VALUES ('embedding_protocol_version', '2');
```

---

## 9. Model Switching Protocol

### 9.1 Principles

- **New memories:** Use currently configured model
- **Old memories:** Retain original model embeddings (immutable)
- **Multi-model storage:** Each memory can have embeddings from multiple models

### 9.2 Configuration Change

When the active model changes from `model_A` to `model_B`:

1. **Immediate effect:** New memories are embedded with `model_B`
2. **Existing embeddings:** Remain unchanged (both `model_A` and `model_B` rows can coexist)
3. **Recall:** Uses only `model_B` embeddings

### 9.3 Backfill Process

To improve recall coverage after model switch:

```python
# Query memories without embeddings for new model
missing_memories = db.execute("""
    SELECT id, content 
    FROM memories 
    WHERE id NOT IN (
        SELECT memory_id 
        FROM memory_embeddings 
        WHERE model = ?
    )
""", [new_model]).fetchall()

# Embed and insert
for memory in missing_memories:
    vector = embed(memory.content, new_model)
    blob = serialize(vector)
    db.execute(
        "INSERT INTO memory_embeddings VALUES (?, ?, ?, ?, ?)",
        [memory.id, new_model, blob, len(vector), now()]
    )
```

### 9.4 Cleanup (Optional)

After full re-embedding, old model embeddings can be removed:

```sql
-- WARNING: This is irreversible
DELETE FROM memory_embeddings WHERE model = ?;  -- Old model
```

**Recommendation:** Retain old embeddings for audit/rollback purposes unless storage is constrained.

---

## 10. Reference Implementations

### 10.1 Rust

**Serialization:**
```rust
use std::convert::TryInto;

fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
    embedding.iter()
        .flat_map(|f| f.to_le_bytes())
        .collect()
}
```

**Deserialization:**
```rust
fn deserialize_embedding(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() % 4 != 0 {
        return Err("Invalid blob length".to_string());
    }
    
    let vec: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    
    // Validate finite
    if vec.iter().any(|f| !f.is_finite()) {
        return Err("Non-finite value in embedding".to_string());
    }
    
    Ok(vec)
}
```

**Write:**
```rust
use chrono::Utc;
use rusqlite::params;

fn store_embedding(
    conn: &Connection,
    memory_id: &str,
    model: &str,
    embedding: &[f32],
) -> Result<()> {
    let blob = serialize_embedding(embedding);
    let dimensions = embedding.len() as i64;
    let created_at = Utc::now().to_rfc3339();
    
    conn.execute(
        "INSERT OR REPLACE INTO memory_embeddings VALUES (?, ?, ?, ?, ?)",
        params![memory_id, model, blob, dimensions, created_at],
    )?;
    
    Ok(())
}
```

### 10.2 Python

**Serialization:**
```python
import struct

def serialize_embedding(embedding: list[float]) -> bytes:
    """Convert float list to little-endian f32 BLOB."""
    return struct.pack(f'<{len(embedding)}f', *embedding)
```

**Deserialization:**
```python
import struct
import math

def deserialize_embedding(blob: bytes) -> list[float]:
    """Convert BLOB to float list with validation."""
    if len(blob) % 4 != 0:
        raise ValueError("Invalid blob length")
    
    n = len(blob) // 4
    embedding = list(struct.unpack(f'<{n}f', blob))
    
    # Validate finite
    if not all(math.isfinite(v) for v in embedding):
        raise ValueError("Non-finite value in embedding")
    
    return embedding
```

**Write:**
```python
from datetime import datetime, timezone

def store_embedding(
    conn,
    memory_id: str,
    model: str,
    embedding: list[float]
) -> None:
    blob = serialize_embedding(embedding)
    dimensions = len(embedding)
    created_at = datetime.now(timezone.utc).isoformat()
    
    conn.execute(
        "INSERT OR REPLACE INTO memory_embeddings VALUES (?, ?, ?, ?, ?)",
        (memory_id, model, blob, dimensions, created_at)
    )
    conn.commit()
```

---

## 11. Versioning and Compatibility

### 11.1 Current Version

**Protocol Version:** 2  
**Metadata Storage:**

```sql
CREATE TABLE IF NOT EXISTS engram_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR REPLACE INTO engram_meta (key, value) 
VALUES ('embedding_protocol_version', '2');
```

### 11.2 Version Detection

Implementations **MUST** check protocol version on initialization:

```sql
SELECT value FROM engram_meta WHERE key = 'embedding_protocol_version';
```

- If missing or `< 2`: Trigger migration (see §8)
- If `> 2`: Log warning about newer protocol version

### 11.3 Future Changes

Future protocol updates will:
1. Increment version number
2. Document breaking changes
3. Provide automated migration paths
4. Maintain backward-read compatibility where possible

---

## 12. Error Handling

### 12.1 Validation Failures

| Error | Cause | Action |
|-------|-------|--------|
| `BLOB_LENGTH_INVALID` | `len(blob) % 4 != 0` | Reject write, log error |
| `DIMENSION_MISMATCH` | `len(blob) / 4 != dimensions` | Reject write, log error |
| `NON_FINITE_VALUE` | NaN or Inf detected | Reject write, log error |
| `MODEL_NAME_INVALID` | Missing `/` separator | Reject write, suggest format |

### 12.2 Migration Failures

If migration encounters unrecoverable data:
1. Log detailed error with `memory_id`
2. Skip corrupted row
3. Continue migration
4. Report summary of skipped rows

### 12.3 Read Failures

If blob validation fails on read:
1. Log error with `memory_id` and `model`
2. Return error (do not silently skip)
3. Consider database corruption

---

## 13. Performance Considerations

### 13.1 Indexing

The `idx_embeddings_model` index is **critical** for recall performance:
- Enables O(log n) model filtering
- Required for efficient backfill queries

### 13.2 Blob Size

Typical embedding sizes:
- 384D (MiniLM): 1.5 KB per embedding
- 768D (Nomic): 3.1 KB per embedding
- 1536D (OpenAI): 6.1 KB per embedding

For 10,000 memories with 768D embeddings: ~31 MB storage.

### 13.3 Batch Operations

For backfill, use transactions:

```python
with conn:  # Transaction context
    for memory in batch:
        store_embedding(conn, memory.id, model, vector)
```

Batch size recommendation: 100-1000 embeddings per transaction.

---

## 14. Security Considerations

### 14.1 Model String Injection

Model strings are user-controlled. Implementations **MUST**:
- Use parameterized queries (never string concatenation)
- Validate format (contains exactly one `/`)
- Limit length (e.g., 256 characters)

### 14.2 Blob Validation

Always validate blobs before deserialization to prevent:
- Buffer overflows (Rust: use `chunks_exact`)
- Memory exhaustion (check dimensions are reasonable)
- NaN/Inf propagation in similarity computations

---

## 15. Compliance

### 15.1 Implementation Checklist

- [ ] Schema matches §2 exactly
- [ ] Model naming follows §3 convention
- [ ] BLOB serialization per §4.2
- [ ] BLOB validation per §4.4
- [ ] Write protocol per §5
- [ ] Read protocol per §6
- [ ] Recall filters by model per §7
- [ ] Migration from v1 supported per §8
- [ ] Version tracking in `engram_meta`
- [ ] Reference implementation tests pass

### 15.2 Testing Requirements

Implementations **MUST** pass:
1. Round-trip serialization (serialize → deserialize → compare)
2. Cross-implementation compatibility (Rust ↔ Python)
3. Validation rejection (NaN, Inf, wrong dimensions)
4. Multi-model storage (same memory, different models)
5. Migration from v1 schema

---

## 16. Changelog

### Version 2.0 (2026-04-02)
- Initial formal specification
- Multi-model support via composite primary key
- Binary-only BLOB format requirement
- Structured model naming convention
- Migration protocol from v1

---

## 17. Authors and Governance

**Maintainer:** Engram Core Team  
**License:** MIT  
**Feedback:** Submit issues to engram-ai-rust repository

All implementations are expected to contribute test cases demonstrating compliance with this specification.

---

**End of Specification**
