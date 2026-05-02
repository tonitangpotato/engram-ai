//! ISS-089 e2e: `store_raw` Path A and Path B both transmit
//! `meta.occurred_at` to the persisted record's `created_at`.
//!
//! This is the regression test guarding the bug ISS-087's first
//! implementation missed: the `--occurred-at` flag worked from
//! the user's POV, but only because the CLI bypassed `store_raw`
//! entirely via `add_with_emotion_at` → `add_raw`. After ISS-089
//! `store_raw` itself honors the override, AND graph substrate
//! (extractor-produced facts) is still produced. Both invariants
//! must hold simultaneously for the v0.3 pipeline to be valid
//! under replay/backfill.

use chrono::TimeZone;
use engramai::dimensions::Importance;
use engramai::enriched::EnrichedMemory;
use engramai::extractor::{ExtractedFact, MemoryExtractor};
use engramai::memory::Memory;
use engramai::store_api::{RawStoreOutcome, StorageMeta, StoreOutcome};
use std::error::Error as StdError;

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

/// Test extractor: returns exactly one fact echoing the input.
/// Sufficient to drive Path A through `from_extracted`.
struct EchoExtractor;

impl MemoryExtractor for EchoExtractor {
    fn extract(
        &self,
        text: &str,
    ) -> Result<Vec<ExtractedFact>, Box<dyn StdError + Send + Sync>> {
        let fact = ExtractedFact {
            core_fact: text.chars().take(80).collect(),
            importance: 0.6,
            confidence: "confident".into(),
            domain: "general".into(),
            ..Default::default()
        };
        Ok(vec![fact])
    }
}

/// Test extractor: returns zero facts. Drives Path A's
/// `no_facts_extracted` fallback branch.
struct NoFactsExtractor;

impl MemoryExtractor for NoFactsExtractor {
    fn extract(
        &self,
        _text: &str,
    ) -> Result<Vec<ExtractedFact>, Box<dyn StdError + Send + Sync>> {
        Ok(vec![])
    }
}

#[test]
fn iss089_path_b_no_extractor_honors_occurred_at() {
    // Path B = no extractor configured. Caller-supplied
    // `occurred_at` must thread through to the record's
    // `created_at`.
    let mut mem = new_mem();

    let backfill_time = chrono::Utc.with_ymd_and_hms(2023, 5, 8, 12, 0, 0).unwrap();

    let meta = StorageMeta {
        importance_hint: Some(0.6),
        source: Some("iss089-test".into()),
        namespace: Some("test-ns".into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: Some(backfill_time),
        emotion: None,
        domain: None,
    };

    let out = mem
        .store_raw("Yesterday I learned that elephants sleep standing up.", meta)
        .expect("store_raw ok");

    let id = match out {
        RawStoreOutcome::Stored(outcomes) => match outcomes.first().expect("at least one outcome") {
            StoreOutcome::Inserted { id } => id.clone(),
            StoreOutcome::Merged { id, .. } => id.clone(),
        },
        other => panic!("expected Stored, got {:?}", other),
    };

    // Fetch the stored record directly by id (avoids recall ranking
    // / namespace-filter complications for this contract test).
    let record = mem
        .get(&id)
        .expect("get ok")
        .expect("record exists");

    assert_eq!(
        record.created_at, backfill_time,
        "Path B: created_at must equal caller's occurred_at, got {:?} (expected {:?})",
        record.created_at, backfill_time
    );
}

#[test]
fn iss089_path_a_extracted_facts_inherit_occurred_at() {
    // Path A with extractor producing facts: every fact's
    // EnrichedMemory.occurred_at must be set, AND the persisted
    // row's created_at must reflect it.
    let mut mem = new_mem();
    mem.set_extractor(Box::new(EchoExtractor));

    let backfill_time = chrono::Utc.with_ymd_and_hms(2023, 5, 8, 12, 0, 0).unwrap();

    let meta = StorageMeta {
        importance_hint: Some(0.6),
        source: Some("iss089-test".into()),
        namespace: Some("test-ns-a".into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: Some(backfill_time),
        emotion: None,
        domain: None,
    };

    let out = mem
        .store_raw("Last Tuesday we shipped the v0.3 release.", meta)
        .expect("store_raw ok");

    let outcomes = match out {
        RawStoreOutcome::Stored(outcomes) => outcomes,
        other => panic!("expected Stored, got {:?}", other),
    };
    assert!(
        !outcomes.is_empty(),
        "EchoExtractor should have produced at least one fact"
    );

    // Every fact's persisted created_at must match backfill_time.
    let mut checked = 0;
    for outcome in &outcomes {
        let id = match outcome {
            StoreOutcome::Inserted { id } => id.clone(),
            StoreOutcome::Merged { id, .. } => id.clone(),
        };
        let rec = mem.get(&id).expect("get ok").expect("record exists");
        assert_eq!(
            rec.created_at, backfill_time,
            "Path A: fact {} created_at must equal occurred_at",
            id
        );
        checked += 1;
    }
    assert!(checked > 0, "at least one fact should be persisted");
}

#[test]
fn iss089_path_a_no_facts_fallback_honors_occurred_at() {
    // Path A but extractor returns zero facts: `no_facts_extracted`
    // fallback admits the raw content as a minimal EnrichedMemory.
    // That fallback must also honor occurred_at.
    let mut mem = new_mem();
    mem.set_extractor(Box::new(NoFactsExtractor));

    let backfill_time = chrono::Utc.with_ymd_and_hms(2023, 5, 8, 12, 0, 0).unwrap();

    let meta = StorageMeta {
        importance_hint: Some(0.5),
        source: Some("iss089-test".into()),
        namespace: Some("test-ns-nf".into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: Some(backfill_time),
        emotion: None,
        domain: None,
    };

    let out = mem
        .store_raw(
            "A short note that the extractor will deem fact-less.",
            meta,
        )
        .expect("store_raw ok");

    let id = match out {
        RawStoreOutcome::Stored(outcomes) => match outcomes.first().expect("at least one outcome")
        {
            StoreOutcome::Inserted { id } => id.clone(),
            StoreOutcome::Merged { id, .. } => id.clone(),
        },
        other => panic!("expected Stored, got {:?}", other),
    };

    let record = mem.get(&id).expect("get ok").expect("record exists");

    assert_eq!(
        record.created_at, backfill_time,
        "Path A no_facts fallback: created_at must equal occurred_at"
    );
}

#[test]
fn iss089_no_occurred_at_falls_back_to_now() {
    // Sanity: when the caller does not provide occurred_at, behavior
    // is unchanged (created_at = wall-clock now). This is the
    // pre-ISS-089 default and must not regress.
    let mut mem = new_mem();

    let before = chrono::Utc::now();

    let meta = StorageMeta {
        importance_hint: Some(0.5),
        source: Some("iss089-test".into()),
        namespace: Some("test-ns-now".into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: None,
        emotion: None,
        domain: None,
    };

    let out = mem
        .store_raw("Live ingest with no override.", meta)
        .expect("store_raw ok");

    let id = match out {
        RawStoreOutcome::Stored(outcomes) => match outcomes.first().expect("outcome") {
            StoreOutcome::Inserted { id } => id.clone(),
            StoreOutcome::Merged { id, .. } => id.clone(),
        },
        other => panic!("expected Stored, got {:?}", other),
    };

    let after = chrono::Utc::now();

    let record = mem.get(&id).expect("get ok").expect("record exists");

    assert!(
        record.created_at >= before && record.created_at <= after,
        "no occurred_at: created_at must be in [before, after] window, got {:?}",
        record.created_at
    );
}

#[test]
fn iss089_storage_meta_default_has_no_occurred_at() {
    // Lightweight contract test: callers using `..Default::default()`
    // get `occurred_at: None`, preserving v0.2 default behavior.
    let _em = EnrichedMemory::minimal(
        "anchor",
        Importance::new(0.5),
        Some("test".into()),
        None,
    )
    .expect("minimal ok");

    let m = StorageMeta {
        importance_hint: None,
        source: None,
        namespace: None,
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: None,
        emotion: None,
        domain: None,
    };
    assert!(
        m.occurred_at.is_none(),
        "fresh StorageMeta must have occurred_at=None"
    );
}
