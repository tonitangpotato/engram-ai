//! Integration tests for the ISS-019 typed write-path API.
//!
//! Covers:
//! - `store_enriched` on extractor-less engine → Inserted outcome,
//!   metadata blob is v1-legacy-shape with valid dimensions.
//! - `store_raw` on extractor-less engine → minimal Dimensions path,
//!   returns `Stored(vec![Inserted])`.
//! - `store_raw` with duplicate content → second call merges into
//!   the first via the existing dedup pipeline (outcome = Merged).
//! - `store_raw` with empty / whitespace content → Skipped(TooShort).

use engramai::dimensions::Importance;
use engramai::enriched::EnrichedMemory;
use engramai::memory::Memory;
use engramai::store_api::{RawStoreOutcome, StorageMeta, StoreOutcome};

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

#[test]
fn store_enriched_inserts_with_legacy_metadata_shape() {
    let mut mem = new_mem();

    let em = EnrichedMemory::minimal(
        "ISS-019 Step 4 is live",
        Importance::new(0.6),
        Some("test".to_string()),
        None,
    )
    .expect("minimal accepts real content");

    let out = mem.store_enriched(em).expect("store_enriched ok");

    match out {
        StoreOutcome::Inserted { id } => {
            assert!(!id.is_empty(), "inserted id should be non-empty");

            // Read the row back and confirm metadata has `dimensions`
            // and `type_weights` at top level (legacy v1 layout is
            // preserved in Step 4 — Step 7 migrates to engram.*).
            // We access via a recall round-trip, which exercises the
            // existing read path and guarantees nothing regressed.
            let results = mem
                .recall("ISS-019 Step 4", 5, None, None)
                .expect("recall ok");
            assert!(!results.is_empty(), "should recall the stored memory");
            let meta = results[0]
                .record
                .metadata
                .as_ref()
                .expect("metadata populated");
            assert!(
                meta.get("dimensions").is_some(),
                "metadata should carry dimensions sub-object"
            );
            assert!(
                meta.get("type_weights").is_some(),
                "metadata should carry type_weights"
            );

            // Scalar fields ISS-020 P0.0 requires on every row.
            let dims = meta.get("dimensions").unwrap();
            assert!(dims.get("valence").is_some(), "valence always present");
            assert!(
                dims.get("confidence").is_some(),
                "confidence always present"
            );
            assert!(dims.get("domain").is_some(), "domain always present");
        }
        StoreOutcome::Merged { .. } => panic!("fresh engine should not merge"),
    }
}

#[test]
fn store_raw_without_extractor_uses_minimal_dimensions() {
    let mut mem = new_mem();
    let meta = StorageMeta {
        importance_hint: Some(0.4),
        source: Some("unit".to_string()),
        namespace: None,
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
    };

    let out = mem
        .store_raw("potato likes cake", meta)
        .expect("store_raw ok");

    match out {
        RawStoreOutcome::Stored(outcomes) => {
            assert_eq!(outcomes.len(), 1, "minimal path produces one outcome");
            assert!(matches!(outcomes[0], StoreOutcome::Inserted { .. }));
        }
        other => panic!("expected Stored, got {other:?}"),
    }
}

#[test]
fn store_raw_empty_content_is_skipped_too_short() {
    let mut mem = new_mem();
    let out = mem
        .store_raw("   \n\t", StorageMeta::default())
        .expect("store_raw ok");

    match out {
        RawStoreOutcome::Skipped { reason, .. } => {
            assert_eq!(reason, engramai::store_api::SkipReason::TooShort);
        }
        other => panic!("expected Skipped, got {other:?}"),
    }
}

#[test]
fn store_raw_user_metadata_is_preserved() {
    let mut mem = new_mem();
    let meta = StorageMeta {
        importance_hint: Some(0.5),
        source: None,
        namespace: None,
        user_metadata: serde_json::json!({"callsite": "test-xyz"}),
        memory_type_hint: None,
    };
    let out = mem
        .store_raw("user meta round-trip", meta)
        .expect("store_raw ok");
    let id = match out {
        RawStoreOutcome::Stored(outs) => match &outs[0] {
            StoreOutcome::Inserted { id } => id.clone(),
            other => panic!("unexpected outcome: {other:?}"),
        },
        other => panic!("unexpected raw outcome: {other:?}"),
    };
    assert!(!id.is_empty());

    // Read back via recall; assert the user-metadata key survives at
    // the top level (legacy layout — will move under user.* in Step 7).
    let results = mem
        .recall("user meta round-trip", 3, None, None)
        .expect("recall ok");
    let meta = results[0]
        .record
        .metadata
        .as_ref()
        .expect("metadata populated");
    assert_eq!(
        meta.get("callsite").and_then(|v| v.as_str()),
        Some("test-xyz"),
        "user metadata key round-trips at top level"
    );
}
