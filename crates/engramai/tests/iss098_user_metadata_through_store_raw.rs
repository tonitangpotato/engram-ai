//! ISS-098: `store_raw` must transmit caller-supplied `meta.user_metadata`
//! end-to-end on BOTH paths (extractor produced facts, and no-facts fallback).
//!
//! Background: RUN-0012 (locomo-conv26-full.db) ingested by an earlier
//! binary persisted only 17/441 ≈ 3.9% of memories with non-null
//! `user_metadata`, even though every cogmembench ingest call passed
//! `dia_id`/`session` in `meta.user_metadata`. The 17 that survived only
//! contained the ISS-088-injected `original_content` field — caller's keys
//! were silently dropped before persistence.
//!
//! With current `main` HEAD (post ISS-091, commit 38c38fe) the data flows
//! correctly. These tests pin that contract so any future refactor of the
//! fact path / no-facts fallback / store_enriched cannot silently drop
//! `user_metadata` again.
//!
//! Implementation note: persisted user_metadata is exposed via
//! `MemoryRecord.metadata["user"]` (v2 layout, see ISS-019 Step 7a).
//! These tests read through that path, not via raw column access.

use engramai::extractor::{ExtractedFact, MemoryExtractor};
use engramai::memory::Memory;
use engramai::store_api::{RawStoreOutcome, StorageMeta, StoreOutcome};
use std::error::Error as StdError;

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

/// Echo extractor: produces exactly one fact echoing the input. Drives the
/// fact path through `from_extracted`.
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

/// Returns zero facts. Drives Path A's `no_facts_extracted` fallback branch.
struct NoFactsExtractor;

impl MemoryExtractor for NoFactsExtractor {
    fn extract(
        &self,
        _text: &str,
    ) -> Result<Vec<ExtractedFact>, Box<dyn StdError + Send + Sync>> {
        Ok(vec![])
    }
}

fn caller_user_metadata() -> serde_json::Value {
    serde_json::json!({
        "dia_id": "D1:3",
        "session": "S05",
        "speaker": "Caroline",
        "custom_field": "preserved-please",
    })
}

fn first_inserted_id(out: &RawStoreOutcome) -> String {
    match out {
        RawStoreOutcome::Stored(outcomes) => {
            assert!(!outcomes.is_empty(), "Stored with empty outcomes");
            match &outcomes[0] {
                StoreOutcome::Inserted { id } => id.clone(),
                StoreOutcome::Merged { id, .. } => id.clone(),
            }
        }
        other => panic!("expected Stored, got {:?}", other),
    }
}

/// Read user_metadata via recall path (v2 layout: metadata.user.*).
/// Returns the JSON object under `metadata.user` for the most-recent
/// memory matching `query`.
fn recall_user_metadata(mem: &mut Memory, query: &str) -> serde_json::Value {
    // Use "*" to search across all namespaces — tests use distinct
    // namespace per test to avoid cross-test pollution, and default
    // recall() only searches the "default" namespace.
    let results = mem
        .recall_from_namespace(query, 3, None, None, Some("*"))
        .expect("recall ok");
    assert!(!results.is_empty(), "recall returned no results for {query:?}");
    let metadata = results[0]
        .record
        .metadata
        .as_ref()
        .expect("metadata populated");
    metadata
        .get("user")
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

#[test]
fn iss098_user_metadata_through_fact_path() {
    // Fact path (extractor produces facts): every persisted fact must
    // carry the caller's user_metadata keys verbatim. ISS-088 may add
    // `original_content` on top — that's an allowed addition — but
    // caller keys must never be dropped.
    let mut mem = new_mem();
    mem.set_extractor(Box::new(EchoExtractor));

    let caller_meta = caller_user_metadata();

    let meta = StorageMeta {
        importance_hint: Some(0.6),
        source: Some("iss098-test".into()),
        namespace: Some("test-ns-fact".into()),
        user_metadata: caller_meta.clone(),
        memory_type_hint: None,
        occurred_at: None,
        emotion: None,
        domain: None,
    };

    let query = "Caroline went to the LGBTQ support group yesterday.";
    let out = mem.store_raw(query, meta).expect("store_raw ok");
    let _id = first_inserted_id(&out);

    let user = recall_user_metadata(&mut mem, query);
    let user_obj = user
        .as_object()
        .unwrap_or_else(|| panic!("metadata.user must be an object on fact path; got {}", user));

    for (k, expected_v) in caller_meta.as_object().unwrap() {
        let got = user_obj.get(k).unwrap_or_else(|| {
            panic!(
                "metadata.user dropped caller key {:?} on fact path; got {}",
                k, user
            )
        });
        assert_eq!(
            got, expected_v,
            "metadata.user key {:?} mismatch on fact path: got {:?}, expected {:?}",
            k, got, expected_v
        );
    }
}

#[test]
fn iss098_user_metadata_through_no_fact_path() {
    // No-facts fallback (Path A, extractor returns []): the raw memory
    // must still carry caller's user_metadata.
    let mut mem = new_mem();
    mem.set_extractor(Box::new(NoFactsExtractor));

    let caller_meta = caller_user_metadata();

    let meta = StorageMeta {
        importance_hint: Some(0.5),
        source: Some("iss098-test".into()),
        namespace: Some("test-ns-nofact".into()),
        user_metadata: caller_meta.clone(),
        memory_type_hint: None,
        occurred_at: None,
        emotion: None,
        domain: None,
    };

    let query = "a short chitchat the extractor will skip";
    let out = mem.store_raw(query, meta).expect("store_raw ok");
    let _id = first_inserted_id(&out);

    let user = recall_user_metadata(&mut mem, query);
    let user_obj = user.as_object().unwrap_or_else(|| {
        panic!(
            "metadata.user must be an object on no-facts fallback; got {}",
            user
        )
    });
    for (k, expected_v) in caller_meta.as_object().unwrap() {
        let got = user_obj.get(k).unwrap_or_else(|| {
            panic!(
                "metadata.user dropped caller key {:?} on no-facts fallback; got {}",
                k, user
            )
        });
        assert_eq!(
            got, expected_v,
            "metadata.user key {:?} mismatch on no-facts fallback",
            k
        );
    }
}

#[test]
fn iss098_user_metadata_path_b_no_extractor() {
    // Path B (no extractor configured at all). Caller's user_metadata
    // must pass through unchanged.
    let mut mem = new_mem();
    let caller_meta = caller_user_metadata();

    let meta = StorageMeta {
        importance_hint: Some(0.5),
        source: Some("iss098-test".into()),
        namespace: Some("test-ns-pathb".into()),
        user_metadata: caller_meta.clone(),
        memory_type_hint: None,
        occurred_at: None,
        emotion: None,
        domain: None,
    };

    let query = "path B store with no extractor";
    let out = mem.store_raw(query, meta).expect("store_raw ok");
    let _id = first_inserted_id(&out);

    let user = recall_user_metadata(&mut mem, query);
    let user_obj = user
        .as_object()
        .unwrap_or_else(|| panic!("metadata.user must be an object on Path B; got {}", user));
    for (k, expected_v) in caller_meta.as_object().unwrap() {
        let got = user_obj.get(k).unwrap_or_else(|| {
            panic!("metadata.user dropped caller key {:?} on Path B; got {}", k, user)
        });
        assert_eq!(got, expected_v, "metadata.user key {:?} mismatch on Path B", k);
    }
}

#[test]
fn iss098_null_user_metadata_does_not_synthesize_keys() {
    // Sanity: passing serde_json::Value::Null must not invent caller
    // keys. (Fact path may add ISS-088's `original_content`; that's a
    // separate, documented addition validated elsewhere.)
    let mut mem = new_mem();
    mem.set_extractor(Box::new(NoFactsExtractor));

    let meta = StorageMeta {
        importance_hint: Some(0.5),
        source: Some("iss098-test".into()),
        namespace: Some("test-ns-null".into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: None,
        emotion: None,
        domain: None,
    };

    let query = "null um stays null";
    let out = mem.store_raw(query, meta).expect("store_raw ok");
    let _id = first_inserted_id(&out);

    let user = recall_user_metadata(&mut mem, query);
    if let Some(obj) = user.as_object() {
        assert!(
            obj.get("dia_id").is_none()
                && obj.get("session").is_none()
                && obj.get("speaker").is_none()
                && obj.get("custom_field").is_none(),
            "no-facts fallback must not invent caller keys when meta.user_metadata=Null, got {}",
            user
        );
    }
}
