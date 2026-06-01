//! ISS-090 e2e: caller-supplied `emotion` / `domain` on
//! `StorageMeta` propagate through `store_raw` to the persisted
//! record's typed `Dimensions`, with the documented fallback
//! discipline:
//!
//! - Path B (no extractor) — caller's values apply directly.
//! - Path A (extractor present) — caller's values fill in only
//!   when the extracted fact left the field at its sentinel
//!   default (`valence == 0.0`, `domain == "general"`); the
//!   extractor's per-fact judgment wins otherwise.
//!
//! Also asserts that the deprecated `add_with_emotion` shim now
//! routes through `store_raw` (full v0.3 pipeline), not the v0.2
//! `add_raw` bypass it used pre-ISS-090.

use engramai::dimensions::{Dimensions, Domain};
use engramai::extractor::{ExtractedFact, MemoryExtractor};
use engramai::memory::Memory;
use engramai::store_api::{RawStoreOutcome, StorageMeta, StoreOutcome};
use engramai::types::MemoryType;
use std::error::Error as StdError;

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

/// Test extractor: emits one fact with sentinel defaults
/// (`valence=0.0`, `domain="general"`). This is the "neutral
/// extractor" case that the Path A fallback is designed to fill
/// from caller-supplied `meta.emotion` / `meta.domain`.
struct NeutralExtractor;

impl MemoryExtractor for NeutralExtractor {
    fn extract(
        &self,
        text: &str,
        _reference: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<ExtractedFact>, Box<dyn StdError + Send + Sync>> {
        let fact = ExtractedFact {
            core_fact: text.chars().take(80).collect(),
            importance: 0.6,
            confidence: "confident".into(),
            valence: 0.0,             // sentinel
            domain: "general".into(), // sentinel
            ..Default::default()
        };
        Ok(vec![fact])
    }
}

/// Test extractor: emits one fact with explicit, opinionated
/// values (`valence=-0.4`, `domain="trading"`). Used to verify
/// the extractor's per-fact judgment wins over the caller's
/// `meta.emotion` / `meta.domain` priors.
struct OpinionatedExtractor;

impl MemoryExtractor for OpinionatedExtractor {
    fn extract(
        &self,
        text: &str,
        _reference: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<ExtractedFact>, Box<dyn StdError + Send + Sync>> {
        let fact = ExtractedFact {
            core_fact: text.chars().take(80).collect(),
            importance: 0.6,
            confidence: "confident".into(),
            valence: -0.4,
            domain: "trading".into(),
            ..Default::default()
        };
        Ok(vec![fact])
    }
}

/// Helper: fetch the typed Dimensions for a stored memory id.
/// Mirrors the read path used by `iss019_v2_metadata_compat.rs`.
fn dims_of(mem: &Memory, id: &str) -> Dimensions {
    let rec = mem.get(id).expect("get ok").expect("record exists");
    let meta = rec
        .metadata
        .clone()
        .expect("v0.3 pipeline must populate metadata");
    Dimensions::from_stored_metadata(&meta, &rec.content).expect("metadata parses to Dimensions")
}

fn first_id(out: RawStoreOutcome) -> String {
    match out {
        RawStoreOutcome::Stored(outcomes) => match outcomes.into_iter().next().expect("outcome") {
            StoreOutcome::Inserted { id } => id,
            StoreOutcome::Merged { id, .. } => id,
        },
        other => panic!("expected Stored, got {:?}", other),
    }
}

#[test]
fn iss090_path_b_emotion_applied_directly() {
    // Path B = no extractor configured. Caller-supplied
    // `emotion` / `domain` apply directly to the single admitted
    // record's dimensions.
    let mut mem = new_mem();

    let meta = StorageMeta {
        importance_hint: Some(0.6),
        source: Some("iss090-test".into()),
        namespace: Some("iss090-pathb".into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: None,
        emotion: Some(-0.7),
        domain: Some("trading".into()),
    };

    let out = mem
        .store_raw("Market closed red after the surprise announcement.", meta)
        .expect("store_raw ok");
    let id = first_id(out);

    let dims = dims_of(&mem, &id);

    assert!(
        (dims.valence.get() - (-0.7)).abs() < 1e-9,
        "Path B: caller emotion must propagate to dimensions.valence, got {}",
        dims.valence.get()
    );
    assert_eq!(
        dims.domain,
        Domain::Trading,
        "Path B: caller domain must propagate to dimensions.domain, got {:?}",
        dims.domain
    );
}

#[test]
fn iss090_path_a_caller_emotion_fills_when_extractor_neutral() {
    // Path A with an extractor that emits sentinel defaults
    // (`valence=0.0`, `domain="general"`). The fallback rule
    // applies and the caller's prior fills both fields.
    let mut mem = new_mem();
    mem.set_extractor(Box::new(NeutralExtractor));

    let meta = StorageMeta {
        importance_hint: Some(0.6),
        source: Some("iss090-test".into()),
        namespace: Some("iss090-patha-fill".into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: None,
        emotion: Some(0.5),
        domain: Some("coding".into()),
    };

    let out = mem
        .store_raw("Refactored the storage layer and shipped the change.", meta)
        .expect("store_raw ok");
    let id = first_id(out);

    let dims = dims_of(&mem, &id);

    assert!(
        (dims.valence.get() - 0.5).abs() < 1e-9,
        "Path A neutral-extractor: caller emotion must fill, got {}",
        dims.valence.get()
    );
    assert_eq!(
        dims.domain,
        Domain::Coding,
        "Path A neutral-extractor: caller domain must fill, got {:?}",
        dims.domain
    );
}

#[test]
fn iss090_path_a_extractor_judgment_wins() {
    // Path A with an opinionated extractor. The fact already
    // carries `valence=-0.4, domain="trading"` (non-default),
    // so the caller's `meta.emotion=0.9, meta.domain="life"`
    // priors are IGNORED — extractor wins.
    let mut mem = new_mem();
    mem.set_extractor(Box::new(OpinionatedExtractor));

    let meta = StorageMeta {
        importance_hint: Some(0.6),
        source: Some("iss090-test".into()),
        namespace: Some("iss090-patha-win".into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: None,
        emotion: Some(0.9),
        domain: Some("life".into()),
    };

    let out = mem
        .store_raw("Took a loss on the EURUSD trade after a bad entry.", meta)
        .expect("store_raw ok");
    let id = first_id(out);

    let dims = dims_of(&mem, &id);

    assert!(
        (dims.valence.get() - (-0.4)).abs() < 1e-9,
        "Path A opinionated-extractor: extractor valence must win, got {}",
        dims.valence.get()
    );
    assert_eq!(
        dims.domain,
        Domain::Trading,
        "Path A opinionated-extractor: extractor domain must win, got {:?}",
        dims.domain
    );
}

#[test]
fn iss090_deprecated_shim_still_routes_through_store_raw() {
    // The deprecated `add_with_emotion` shim must now delegate to
    // `store_raw` (full v0.3 pipeline). Asserted by:
    //   1. The call returns a valid id and the row is fetchable.
    //   2. The row has metadata populated (v0.3 stores typed
    //      Dimensions; the v0.2 `add_raw` bypass would not).
    //   3. The caller's emotion/domain land on the dimensions
    //      (Path B applies directly because no extractor is set).
    let mut mem = new_mem();

    #[allow(deprecated)]
    let id = mem
        .add_with_emotion(
            "Closed the position at a small loss.",
            MemoryType::Episodic,
            None,
            None,
            None,
            None,
            -0.6,
            "trading",
        )
        .expect("add_with_emotion ok");

    let rec = mem.get(&id).expect("get ok").expect("record exists");
    assert!(
        rec.metadata.is_some(),
        "v0.3 pipeline must populate MemoryRecord.metadata; \
         missing metadata indicates the shim bypassed store_raw"
    );

    let dims = dims_of(&mem, &id);
    assert!(
        (dims.valence.get() - (-0.6)).abs() < 1e-9,
        "deprecated shim must thread emotion through store_raw, got {}",
        dims.valence.get()
    );
    assert_eq!(
        dims.domain,
        Domain::Trading,
        "deprecated shim must thread domain through store_raw, got {:?}",
        dims.domain
    );
}
