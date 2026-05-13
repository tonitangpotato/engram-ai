//! ISS-087 / ISS-103: end-to-end persistence test for `occurred_at`.
//!
//! Originally (ISS-087) this test asserted that a caller-supplied
//! `occurred_at` was written to `created_at`. ISS-103 reversed that
//! contract: `created_at` is wall-clock ingest time (drives lifecycle
//! decay), `occurred_at` is the logical event time (drives temporal
//! grounding). The mass-soft-delete bug in RUN-0017 is the proof.
//!
//! These tests now verify the ISS-103 contract end-to-end through the
//! `add_with_emotion_at` path:
//!   - Caller-supplied event time → `record.occurred_at`
//!   - `record.created_at` is always wall-clock at insert time
//!   - When no event time is supplied, `occurred_at` is None and
//!     `event_time()` falls back to `created_at`.

use chrono::{TimeZone, Utc};
use engramai::{Memory, MemoryType};

#[test]
fn occurred_at_override_persists_to_record_field_not_created_at() {
    let mut mem = Memory::new(":memory:", None).expect("open in-memory db");

    let anchor = Utc.with_ymd_and_hms(2023, 5, 8, 0, 0, 0).unwrap();
    let before = Utc::now();

    let id = mem
        .add_with_emotion_at(
            "Caroline attended a LGBTQ support group yesterday",
            MemoryType::Factual,
            None,
            None,
            None,
            None,
            0.0,
            "default",
            Some(anchor),
        )
        .expect("store with occurred_at");

    let after = Utc::now();
    let record = mem
        .get(&id)
        .expect("recall persisted record")
        .expect("record should exist");

    // ISS-103: occurred_at gets the caller's event time.
    assert_eq!(
        record.occurred_at,
        Some(anchor),
        "ISS-103: occurred_at must round-trip the caller-supplied event time",
    );

    // ISS-103: created_at is wall-clock ingest time, NOT the event time.
    // This is the regression-proof against ISS-087's overload.
    assert!(
        record.created_at >= before && record.created_at <= after,
        "ISS-103: created_at must be wall-clock at insert (got {}, before={}, after={})",
        record.created_at,
        before,
        after,
    );
    assert_ne!(
        record.created_at, anchor,
        "ISS-103: created_at must NOT equal occurred_at — that overload is the RUN-0017 bug",
    );

    // event_time() — the canonical accessor for temporal queries —
    // returns the event time.
    assert_eq!(record.event_time(), anchor);
}

#[test]
fn occurred_at_none_leaves_field_unset_and_event_time_falls_back() {
    let mut mem = Memory::new(":memory:", None).expect("open in-memory db");

    let before = Utc::now();
    let id = mem
        .add_with_emotion_at(
            "control case — no anchor",
            MemoryType::Factual,
            None,
            None,
            None,
            None,
            0.0,
            "default",
            None,
        )
        .expect("store without occurred_at");
    let after = Utc::now();

    let record = mem.get(&id).unwrap().unwrap();

    // ISS-103: no caller event time → occurred_at stays None.
    assert_eq!(
        record.occurred_at, None,
        "ISS-103: occurred_at must be None when caller didn't supply one"
    );

    // created_at is wall-clock now.
    assert!(
        record.created_at >= before && record.created_at <= after,
        "created_at must be wall-clock now (got {})",
        record.created_at
    );

    // event_time() falls back to created_at when occurred_at is None.
    assert_eq!(record.event_time(), record.created_at);
}
