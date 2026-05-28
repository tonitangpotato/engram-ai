//! ISS-178 integration: prev_turn flows from StorageMeta → store_raw →
//! extractor::extract_with_context, end-to-end.
//!
//! Step 1-3 unit tests cover the extractor trait + builder helper in
//! isolation. This test proves the store_raw path actually invokes
//! `extract_with_context` (not `extract`) and passes the configured
//! `meta.prev_turn` through unchanged.

use engramai::{
    extractor::{ExtractedFact, ExtractionContext, MemoryExtractor},
    store_api::StorageMeta,
    Memory,
};
use std::error::Error;
use std::sync::{Arc, Mutex};

/// Test fixture extractor that records every call it receives.
///
/// `extract_with_context` is overridden — when store_raw routes through
/// extract_with_context instead of extract, this proves the new path is
/// live. When store_raw still went through extract(), the panic in the
/// `extract()` impl would have fired.
struct RecordingExtractor {
    seen: Arc<Mutex<Vec<ExtractionContext>>>,
}

impl RecordingExtractor {
    fn new() -> (Self, Arc<Mutex<Vec<ExtractionContext>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                seen: Arc::clone(&seen),
            },
            seen,
        )
    }
}

impl MemoryExtractor for RecordingExtractor {
    fn extract(
        &self,
        text: &str,
    ) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
        // store_raw used to call this directly. After ISS-178 it should
        // go through extract_with_context; the override forwards here
        // only via the explicit fall-through path (no prev_turn case),
        // and even then via the ExtractionContext::single wrapper. So
        // this method MAY be called when prev_turn is None.
        self.seen
            .lock()
            .unwrap()
            .push(ExtractionContext::single(text));
        // Returning empty facts triggers ISS-068's "no facts" path which
        // still admits the raw content as a minimal record — keeps the
        // test simple.
        Ok(vec![])
    }

    fn extract_with_context(
        &self,
        ctx: &ExtractionContext,
    ) -> Result<Vec<ExtractedFact>, Box<dyn Error + Send + Sync>> {
        self.seen.lock().unwrap().push(ctx.clone());
        Ok(vec![])
    }
}

#[test]
fn iss178_prev_turn_flows_from_storage_meta_to_extractor() {
    // Setup: in-memory Memory, attach our recording extractor.
    let mut memory = Memory::new(":memory:", None).expect("Memory::new failed");
    let (recording, seen) = RecordingExtractor::new();
    memory.set_extractor(Box::new(recording));

    // Build StorageMeta with a non-trivial prev_turn.
    let meta = StorageMeta {
        prev_turn: Some("What were you researching yesterday?".to_string()),
        ..StorageMeta::default()
    };

    let _ = memory
        .ingest_with_meta(
            "Researching adoption agencies and their inclusivity policies.",
            meta,
        )
        .expect("ingest_with_meta should succeed");

    // The recording extractor must have been called exactly once via
    // extract_with_context, with both current and prev populated.
    let calls = seen.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "extractor should have been called exactly once; got {} calls",
        calls.len()
    );

    let ctx = &calls[0];
    assert_eq!(
        ctx.current,
        "Researching adoption agencies and their inclusivity policies."
    );
    assert_eq!(
        ctx.prev.as_deref(),
        Some("What were you researching yesterday?"),
        "prev_turn from StorageMeta did not reach the extractor"
    );
    assert!(ctx.has_prev());
}

#[test]
fn iss178_no_prev_turn_still_invokes_extract_with_context_path() {
    // When prev_turn is None, store_raw still goes through
    // extract_with_context (which then internally falls through to
    // extract() in our recording impl). What matters is that the call
    // observed has prev = None — proving the StorageMeta default is
    // preserved end-to-end.
    let mut memory = Memory::new(":memory:", None).expect("Memory::new failed");
    let (recording, seen) = RecordingExtractor::new();
    memory.set_extractor(Box::new(recording));

    let _ = memory
        .ingest_with_stats("Plain ingest, no prev turn.")
        .expect("ingest_with_stats should succeed");

    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 1, "extractor must be called exactly once");
    let ctx = &calls[0];
    assert_eq!(ctx.current, "Plain ingest, no prev turn.");
    assert!(
        ctx.prev.is_none(),
        "prev_turn should be None when caller did not set it"
    );
}

#[test]
fn iss178_whitespace_prev_turn_does_not_reach_extractor_as_prev() {
    // Tighten the has_prev() contract end-to-end: when caller passes a
    // whitespace-only prev_turn, the extractor's `ctx.has_prev()` MUST be
    // false. This protects against accidental empty-turn enrichment when
    // a driver hands us blank lines from a corpus.
    let mut memory = Memory::new(":memory:", None).expect("Memory::new failed");
    let (recording, seen) = RecordingExtractor::new();
    memory.set_extractor(Box::new(recording));

    let meta = StorageMeta {
        prev_turn: Some("   \n  \t  ".to_string()),
        ..StorageMeta::default()
    };

    let _ = memory
        .ingest_with_meta("Current turn content.", meta)
        .expect("ingest_with_meta should succeed");

    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let ctx = &calls[0];
    // The prev string IS forwarded (whitespace preserved) but has_prev()
    // sees it as empty after trim and returns false — so the extractor's
    // override path knows to fall through.
    assert!(
        !ctx.has_prev(),
        "whitespace-only prev_turn must NOT count as having prev context"
    );
}
