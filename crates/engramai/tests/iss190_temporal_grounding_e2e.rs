//! ISS-190 AC-2 end-to-end: store-time temporal grounding.
//!
//! The q29/q0 topology: an episode states a duration ("owned for 3 years")
//! and the ingest path knows the reference date (occurred_at = 2023-03-27).
//! The fix threads that reference into `MemoryExtractor::extract`, so the
//! (LLM) extractor can resolve the duration to an absolute year (~2020) at
//! STORE time — not at answer time in some downstream consumer.
//!
//! These tests use a deterministic mock extractor that mimics the LLM's
//! resolution behaviour: given a reference date it converts "owned for N
//! years" into "~(reference_year - N)". This isolates the *plumbing* under
//! test (reference threading + temporal mark survival) from the LLM itself,
//! which is exercised separately by the conv-44 benchmark (AC-6).

use chrono::{Datelike, TimeZone};
use engramai::enriched::parse_temporal_mark;
use engramai::extractor::{ExtractedFact, MemoryExtractor};
use engramai::dimensions::TemporalMark;
use engramai::memory::Memory;
use engramai::store_api::{RawStoreOutcome, StorageMeta, StoreOutcome};
use std::error::Error as StdError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Mock extractor that resolves "owned for N years" against the supplied
/// reference date — exactly the derivation the real LLM is now asked to do.
/// Also records whether a reference was actually received (AC-1 plumbing).
struct ResolvingExtractor {
    saw_reference: Arc<AtomicBool>,
}

impl MemoryExtractor for ResolvingExtractor {
    fn extract(
        &self,
        text: &str,
        reference: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<ExtractedFact>, Box<dyn StdError + Send + Sync>> {
        let temporal = match reference {
            Some(r) => {
                self.saw_reference.store(true, Ordering::SeqCst);
                // Parse "owned for N years" and resolve to an approximate year.
                let n: i32 = text
                    .split_whitespace()
                    .find_map(|w| w.parse::<i32>().ok())
                    .unwrap_or(0);
                if text.contains("year") && n > 0 {
                    Some(format!("~{}", r.year() - n))
                } else {
                    None // no time cue → omit, do NOT fabricate
                }
            }
            None => None,
        };

        let fact = ExtractedFact {
            core_fact: text.to_string(),
            temporal,
            importance: 0.6,
            confidence: "confident".into(),
            domain: "general".into(),
            ..Default::default()
        };
        Ok(vec![fact])
    }
}

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

#[test]
fn iss190_ac2_duration_resolves_to_absolute_year_at_store_time() {
    let saw_reference = Arc::new(AtomicBool::new(false));
    let mut mem = new_mem();
    mem.set_extractor(Box::new(ResolvingExtractor {
        saw_reference: saw_reference.clone(),
    }));

    // The q29 topology: duration phrase + a known reference date.
    let reference = chrono::Utc.with_ymd_and_hms(2023, 3, 27, 0, 0, 0).unwrap();
    let meta = StorageMeta {
        occurred_at: Some(reference),
        ..StorageMeta::default()
    };

    let out = mem
        .store_raw("Audrey owned Pepper for 3 years", meta)
        .expect("store_raw ok");

    let id = match out {
        RawStoreOutcome::Stored(outcomes) => match outcomes.first().expect("one outcome") {
            StoreOutcome::Inserted { id } => id.clone(),
            StoreOutcome::Merged { id, .. } => id.clone(),
        },
        other => panic!("expected Stored, got {:?}", other),
    };

    // AC-1 plumbing: the reference actually reached the extractor.
    assert!(
        saw_reference.load(Ordering::SeqCst),
        "extractor must receive the per-episode reference date"
    );

    // AC-2: the stored memory carries the derived absolute year. We assert on
    // the persisted content/metadata so the answer LLM can read "2020"
    // regardless of how the temporal dimension is encoded.
    let record = mem.get(&id).expect("get ok").expect("record exists");
    let blob = format!("{} {:?}", record.content, record.metadata);
    assert!(
        blob.contains("2020"),
        "stored memory must carry the derived year 2020 (3 years before 2023); got: {blob}"
    );
}

#[test]
fn iss190_ac2_approx_year_string_survives_parse_temporal_mark() {
    // ISS-190 preserved the year string; ISS-191 AC-2 (commit bb3f5ac) then
    // promoted it from free-text Vague to a structured year-granular interval
    // so a downstream interval scorer (AC-3) reads bounds instead of
    // re-parsing a string. "~2020" therefore lands in TemporalMark::Approx
    // with the fuzz marker recorded — the year is still preserved (the
    // original ISS-190 intent), just in structured form.
    let mark = parse_temporal_mark("~2020");
    match mark {
        TemporalMark::Approx {
            start,
            end,
            approximate,
            ..
        } => {
            assert_eq!(start.year(), 2020, "Approx start must carry the year 2020");
            assert_eq!(
                end.map(|d| d.year()),
                Some(2020),
                "year-only Approx spans the full calendar year"
            );
            assert!(approximate, "the `~` fuzz marker must set approximate=true");
        }
        other => panic!("expected Approx year-granular interval, got {other:?}"),
    }
}

#[test]
fn iss190_ac2_no_time_cue_is_not_fabricated() {
    // Negative guard at the store-path level: a fact with no time reference
    // must NOT acquire a temporal value even when a reference date exists.
    let mut mem = new_mem();
    mem.set_extractor(Box::new(ResolvingExtractor {
        saw_reference: Arc::new(AtomicBool::new(false)),
    }));

    let reference = chrono::Utc.with_ymd_and_hms(2023, 3, 27, 0, 0, 0).unwrap();
    let meta = StorageMeta {
        occurred_at: Some(reference),
        ..StorageMeta::default()
    };

    let out = mem
        .store_raw("Audrey likes the colour blue", meta)
        .expect("store_raw ok");

    let id = match out {
        RawStoreOutcome::Stored(outcomes) => match outcomes.first().expect("one outcome") {
            StoreOutcome::Inserted { id } => id.clone(),
            StoreOutcome::Merged { id, .. } => id.clone(),
        },
        other => panic!("expected Stored, got {:?}", other),
    };

    let record = mem.get(&id).expect("get ok").expect("record exists");
    let blob = format!("{} {:?}", record.content, record.metadata);
    // No year should have been invented from the 2023 reference.
    assert!(
        !blob.contains("2023") && !blob.contains("2020"),
        "no time cue must not fabricate a date; got: {blob}"
    );
}
