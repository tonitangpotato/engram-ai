# ISS-039 Implementation Summary

## Changes Made

### 1. New Function: `compute_association_confidence`
**Location:** `crates/engramai/src/memory.rs` (lines ~5618-5665)

Added a new confidence scoring function specifically for association/causal recall paths:

```rust
fn compute_association_confidence(
    activation: f64,
    age_hours: f64,
) -> f64
```

**Model:**
- **Primary signal:** `activation` (ACT-R retrieval activation) — weighted 95%
- **Secondary signal:** `age_hours` (recency boost) — weighted 5%
- Maps activation to 0-1 using sigmoid centered at 0.0 (ACT-R threshold)
- Returns confidence on same 0-1 scale as `compute_query_confidence`

### 2. Call Site Replacements

Replaced `compute_query_confidence` with `compute_association_confidence` at three locations:

#### Site 1: Line ~4466 (causal recall path)
```rust
// OLD:
let confidence = compute_query_confidence(None, false, 0.0, age_hours);

// NEW:
let confidence = compute_association_confidence(activation, age_hours);
```

#### Site 2: Line ~4611 (cached working-memory associations fallback)
```rust
// OLD:
let confidence = compute_query_confidence(None, false, 0.0, age_hours);

// NEW:
let confidence = compute_association_confidence(activation, age_hours);
```

#### Site 3: Line ~4977 (recall_with_associations associations leg)
```rust
// OLD:
let confidence = compute_query_confidence(None, false, 0.0, age_hours);

// NEW:
let confidence = compute_association_confidence(activation, age_hours);
```

### 3. Unit Tests Added

Added 8 comprehensive tests in the `confidence_tests` module (lines ~5895-5965):

1. `test_association_confidence_strong_activation_fresh` — High activation + fresh → high confidence
2. `test_association_confidence_strong_activation_old` — High activation + old → still high
3. `test_association_confidence_weak_activation_fresh` — Low activation + fresh → low confidence
4. `test_association_confidence_weak_activation_old` — Low activation + old → very low
5. `test_association_confidence_activation_is_primary` — Verify activation dominates
6. `test_association_confidence_recency_is_secondary` — Verify recency has small effect
7. `test_association_confidence_zero_activation_threshold` — ACT-R threshold maps to ~0.5

All tests pass ✅

## Verification

```bash
$ cargo test -p engramai
   ...
   test result: ok. 1419 passed; 0 failed; 2 ignored
```

- All existing `compute_query_confidence` tests remain green ✅
- New `compute_association_confidence` tests all pass ✅
- Query-path call sites (lines 3256, 3721) remain unchanged ✅
- No changes to `RecallResult` shape or `confidence_label` thresholds ✅

## Acceptance Criteria Met

- ✅ New `compute_association_confidence` function added
- ✅ Three association/causal recall call sites updated
- ✅ At least 4 unit tests covering (strong/weak) × (fresh/old)
- ✅ No changes to RecallResult shape
- ✅ Query-path call sites untouched
- ✅ Confidence label thresholds unchanged
- ✅ All tests pass

## Design Decision: Option A (API Split)

Implemented **Option A** from ISS-039: separate `compute_association_confidence` function rather than adding complexity to existing `compute_query_confidence`. This provides:
- Clear semantic separation between query-relevance and association-strength scoring
- Simpler parameter signature (2 params vs 4)
- Better maintainability (each function has single responsibility)
- Future flexibility to tune models independently
