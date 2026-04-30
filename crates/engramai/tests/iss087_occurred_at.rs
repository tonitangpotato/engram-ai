//! ISS-087: end-to-end persistence test for `occurred_at` override.
//!
//! Verifies that a logical event time supplied via `add_with_emotion_at`
//! lands on the stored `MemoryRecord.created_at`, and that omitting it
//! (legacy path) defaults to wall-clock now. This is the test that
//! actually proves the override is wired through the SQLite insert path,
//! not just present in `StorageMeta`.

use chrono::{TimeZone, Utc};
use engramai::{Memory, MemoryType};

#[test]
fn occurred_at_override_persists_to_created_at() {
    let mut mem = Memory::new(":memory:", None).expect("open in-memory db");

    let anchor = Utc.with_ymd_and_hms(2023, 5, 8, 0, 0, 0).unwrap();

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

    let record = mem
        .get(&id)
        .expect("recall persisted record")
        .expect("record should exist");

    assert_eq!(
        record.created_at, anchor,
        "ISS-087: created_at must equal the supplied occurred_at, not wall-clock now"
    );
}

#[test]
fn occurred_at_none_falls_back_to_wall_clock_now() {
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

    assert!(
        record.created_at >= before && record.created_at <= after,
        "ISS-087: when occurred_at=None, created_at must be wall-clock now (got {})",
        record.created_at
    );
}
