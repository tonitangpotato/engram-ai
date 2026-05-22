//! ISS-133: `Memory::add_episode` and `Memory::reextract_episodes` —
//! the v0.3 ingestion API surface for the `Episode` (ISS-041) and
//! `ReextractReport` (ISS-042) contracts.
//!
//! These tests pin the four acceptance criteria from the issue body:
//!
//! 1. **Idempotency key round-trip** — caller-supplied `Episode.id` is
//!    used as the stored memory id and returned verbatim from
//!    `add_episode`. A follow-up `Memory::get(id)` finds the row.
//! 2. **Default `when` fills in** — when `Episode.when` is `None` the
//!    storage layer stamps wall-clock time; when `Some(t)` it threads
//!    `t` into `MemoryRecord.occurred_at`.
//! 3. **`session_id` propagates** — `Episode.session_id` lands inside
//!    `MemoryRecord.metadata` under the `user.session_id` key (v2
//!    metadata layout per ISS-019 Step 7a).
//! 4. **`reextract_episodes` skipped_idempotent bucket** — calling on
//!    an already-Completed episode populates `skipped_idempotent`,
//!    *not* `succeeded`. This is the GOAL-2.1 anchor.
//!
//! Implementation knobs the tests rely on:
//!
//! * Dedup MUST be disabled when supplying `Episode.id` (ISS-133 Q2=(d)
//!   decision — see `Memory::add_episode` doc-comment). Tests 1 + 3 +
//!   4 disable it explicitly.
//! * Test 4 wires `with_pipeline_pool` with a deterministic mock
//!   `TripleExtractor` so the pipeline reaches `Completed` without
//!   live LLM calls. Same pattern as `iss037_pipeline_e2e_test`.

use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use engramai::config::MemoryConfig;
use engramai::memory::Memory;
use engramai::resolution::{Episode, ExtractionStatus, ResolutionConfig};
use engramai::triple::{Predicate, Triple};
use engramai::triple_extractor::TripleExtractor;
use tempfile::tempdir;
use uuid::Uuid;

/// Build a Memory with dedup disabled and no extractor — minimal config
/// for tests that only exercise the admission path.
fn mem_no_dedup() -> Memory {
    let mut config = MemoryConfig::default();
    config.dedup_enabled = false;
    Memory::new(":memory:", Some(config)).expect("memory boots")
}

/// Deterministic `TripleExtractor` for test 4. Always emits the same
/// triple so the pipeline reaches `Completed` regardless of input.
struct MockTripleExtractor {
    invocations: AtomicUsize,
}

impl MockTripleExtractor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            invocations: AtomicUsize::new(0),
        })
    }
}

impl TripleExtractor for MockTripleExtractor {
    fn extract_triples(
        &self,
        _content: &str,
    ) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Ok(vec![Triple::new(
            "Alice".to_string(),
            Predicate::RelatedTo,
            "Bob".to_string(),
            0.95,
        )])
    }
}

// ---------------------------------------------------------------------
// AC 1: idempotency-key round-trip
// ---------------------------------------------------------------------

#[test]
fn add_episode_honors_caller_supplied_id() {
    let mut mem = mem_no_dedup();
    let caller_id = Uuid::new_v4();

    let ep = Episode::new("Alice met Bob at the cafe.").with_id(caller_id);
    let returned = mem
        .add_episode(ep)
        .expect("add_episode admits the row and returns the caller id");

    assert_eq!(
        returned, caller_id,
        "add_episode must return the caller-supplied id verbatim"
    );

    // The stored row uses `caller_id.to_string()` as its primary key.
    // Memory::get takes &str.
    let stored = mem
        .get(&caller_id.to_string())
        .expect("get does not error")
        .expect("memory exists under the caller-supplied id");
    assert_eq!(stored.content, "Alice met Bob at the cafe.");
}

#[test]
fn add_episode_without_id_returns_uuid_derived_from_minted_hex() {
    // When the caller doesn't supply `Episode.id`, the engine mints a
    // fresh 8-char hex id (v0.2-compat). `add_episode` derives a Uuid
    // from those 32 bits and returns it. The returned Uuid is lossy —
    // it cannot be used to round-trip through `Memory::get` because
    // `memories.id` stores the 8-char form. Callers who need round-trip
    // MUST supply `Episode.id`. This test pins that contract.
    let mut mem = mem_no_dedup();

    let ep = Episode::new("Some text with no caller id.");
    let returned = mem.add_episode(ep).expect("admits");

    // Returned Uuid has the high bits as zero (we only filled 32 bits).
    assert_eq!(
        returned.as_u128() >> 32,
        0,
        "no-caller-id path produces a Uuid with high 96 bits zero"
    );

    // The stored memory does NOT live under `returned.to_string()` —
    // it lives under the 8-char hex. Memory::get with the Uuid string
    // therefore misses; the 8-char form is the source of truth.
    assert!(
        mem.get(&returned.to_string()).expect("ok").is_none(),
        "Uuid-formatted id MUST NOT collide with the 8-char hex storage key"
    );
}

#[test]
fn add_episode_rejects_caller_id_when_dedup_enabled() {
    // Q2=(d): with dedup ON, caller-supplied id and content-hash dedup
    // are semantically incompatible. add_raw returns an error rather
    // than silently picking one.
    let mut config = MemoryConfig::default();
    config.dedup_enabled = true; // default but be explicit
    let mut mem = Memory::new(":memory:", Some(config)).expect("memory boots");

    let ep = Episode::new("dedup-clash text").with_id(Uuid::new_v4());
    let err = mem.add_episode(ep).expect_err("must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("dedup"),
        "error must mention dedup; got: {msg}"
    );
}

// ---------------------------------------------------------------------
// AC 2: default `when` fills in
// ---------------------------------------------------------------------

#[test]
fn add_episode_when_none_uses_wallclock_for_created_at() {
    let mut mem = mem_no_dedup();
    let id = Uuid::new_v4();

    let before = Utc::now();
    let ep = Episode::new("event with no explicit when").with_id(id);
    mem.add_episode(ep).expect("admits");
    let after = Utc::now();

    let rec = mem.get(&id.to_string()).unwrap().unwrap();
    // `occurred_at` is None when caller didn't supply `when`. The
    // wall-clock timestamp lives in `created_at`.
    assert!(
        rec.occurred_at.is_none(),
        "no `when` supplied → occurred_at stays None (storage uses wall-clock for created_at)"
    );
    assert!(
        rec.created_at >= before && rec.created_at <= after,
        "created_at must be wall-clock-now; got {} not in [{}, {}]",
        rec.created_at,
        before,
        after
    );
}

#[test]
fn add_episode_when_some_propagates_to_occurred_at() {
    let mut mem = mem_no_dedup();
    let id = Uuid::new_v4();
    let backdated = Utc.with_ymd_and_hms(2023, 6, 15, 12, 0, 0).unwrap();

    let ep = Episode::new("2023 conversation backfill")
        .with_id(id)
        .with_when(backdated);
    mem.add_episode(ep).expect("admits");

    let rec = mem.get(&id.to_string()).unwrap().unwrap();
    assert_eq!(
        rec.occurred_at,
        Some(backdated),
        "Episode.when must thread through to MemoryRecord.occurred_at"
    );
}

// ---------------------------------------------------------------------
// AC 3: session_id propagates
// ---------------------------------------------------------------------

#[test]
fn add_episode_session_id_lands_in_user_metadata() {
    let mut mem = mem_no_dedup();
    let id = Uuid::new_v4();
    let session = Uuid::new_v4();

    let ep = Episode::new("turn in a session").with_id(id).with_session(session);
    mem.add_episode(ep).expect("admits");

    let rec = mem.get(&id.to_string()).unwrap().unwrap();
    // v2 metadata layout (ISS-019 Step 7a): user payload nested under
    // `metadata["user"]`.
    let meta = rec.metadata.as_ref().expect("metadata is set");
    let user = meta
        .get("user")
        .expect("metadata.user namespace exists when user_metadata was supplied");
    let session_in_meta = user
        .get("session_id")
        .expect("user.session_id was spliced in")
        .as_str()
        .expect("session_id is a string");
    assert_eq!(session_in_meta, session.to_string());
}

// ---------------------------------------------------------------------
// AC 4: reextract_episodes — skipped_idempotent bucket
// ---------------------------------------------------------------------

#[test]
fn reextract_episodes_buckets_already_completed_as_skipped_idempotent() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("iss133-reextract.db");
    let db_path_str = db_path.to_str().expect("utf-8");

    let triple_extractor: Arc<dyn TripleExtractor> = MockTripleExtractor::new();

    // Pipeline with a single worker so ordering is deterministic.
    let mut resolution = ResolutionConfig::default();
    resolution.worker_count = 1;
    resolution.queue_cap = 8;
    resolution.shutdown_drain = Duration::from_secs(2);
    resolution.worker_idle_poll = Duration::from_millis(10);

    let mut config = MemoryConfig::default();
    config.dedup_enabled = false;

    let mut mem = Memory::new(db_path_str, Some(config))
        .expect("memory boots")
        .with_pipeline_pool(&db_path, triple_extractor, resolution)
        .expect("pipeline pool wires up");

    let id = Uuid::new_v4();
    let ep = Episode::new("Alice met Bob in the park.").with_id(id);
    mem.add_episode(ep).expect("admit succeeds");

    // Poll until the first run reaches Completed (the worker pool
    // drains the queue asynchronously). Bounded so test doesn't hang
    // if something is wired wrong.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = mem.extraction_status(&id.to_string()).expect("status query");
        if matches!(status, ExtractionStatus::Completed { .. }) {
            break;
        }
        if Instant::now() > deadline {
            panic!(
                "extraction did not reach Completed within 5s; last status = {:?}",
                status
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // Now reextract — must land in skipped_idempotent, NOT succeeded.
    let report = mem.reextract_episodes(vec![id]).expect("reextract returns");
    assert_eq!(report.requested, 1);
    assert_eq!(
        report.skipped_idempotent,
        vec![id],
        "already-completed episode must bucket as skipped_idempotent"
    );
    assert!(
        report.succeeded.is_empty(),
        "skipped_idempotent must NOT also count as succeeded"
    );
    assert!(report.still_failed.is_empty());
    assert!(report.is_complete());
}

#[test]
fn reextract_episodes_missing_memory_lands_in_still_failed() {
    // No pipeline pool needed — the "memory not found" branch fires
    // before any queue interaction.
    let mut mem = mem_no_dedup();
    let ghost = Uuid::new_v4();

    let report = mem
        .reextract_episodes(vec![ghost])
        .expect("reextract returns Ok with per-id failures bucketed");

    assert_eq!(report.requested, 1);
    assert_eq!(report.still_failed.len(), 1);
    assert_eq!(report.still_failed[0].0, ghost);
    assert!(
        report.still_failed[0].1.contains("not found"),
        "missing-memory reason must be diagnostic; got {:?}",
        report.still_failed[0].1
    );
    assert!(report.succeeded.is_empty());
    assert!(report.skipped_idempotent.is_empty());
}
