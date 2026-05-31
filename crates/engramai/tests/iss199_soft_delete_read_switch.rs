//! ISS-199 (Phase E read-cutover) — `soft_delete` / `get_deleted_at`
//! read-switch contract, including the §8.6 TEXT/REAL `deleted_at`
//! reconciliation.
//!
//! `memories.deleted_at` is TEXT (RFC3339); `nodes.deleted_at` is REAL
//! (epoch f64). Under `unified_substrate = true` (T34a) the legacy
//! `memories` row is absent, so:
//!   - `soft_delete` writes only `nodes.deleted_at` (the legacy UPDATE
//!     is gated off — it was a 0-row no-op anyway).
//!   - `get_deleted_at` reads `nodes.deleted_at` (REAL epoch) and
//!     converts epoch → RFC3339 to preserve its `Option<String>`
//!     contract.
//!
//! Acceptance contract:
//!
//!   1. Live memory: `get_deleted_at` returns `None` on both substrates.
//!   2. After `soft_delete`: `get_deleted_at` returns `Some(rfc3339)` on
//!      both substrates, and the returned string parses as a valid
//!      RFC3339 instant.
//!   3. The unified-mode RFC3339 round-trips to the same instant the
//!      `nodes.deleted_at` REAL epoch encodes (no format drift).

use chrono::{DateTime, TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use tempfile::tempdir;

fn rec(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 31, 4, 30, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: content.into(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss199-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

#[test]
fn iss199_live_memory_get_deleted_at_none_both() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");
    // Legacy add writes both substrates so both arms have a readable row.
    let mut s = Storage::new(&db).unwrap();
    s.add(&rec("m-1", "live"), "default").unwrap();

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .get_deleted_at("m-1")
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .get_deleted_at("m-1")
        .unwrap();

    assert!(legacy.is_none(), "legacy live row → None");
    assert!(unified.is_none(), "unified live row → None");
}

#[test]
fn iss199_soft_delete_then_get_deleted_at_unified() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    // Add + soft-delete under unified mode (no `memories` row exists).
    let now_epoch_before;
    {
        let mut s = Storage::with_unified_substrate(&db, true).unwrap();
        s.add(&rec("m-1", "to delete"), "default").unwrap();

        // No legacy memories row under unified add.
        let mem_count: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM memories WHERE id = 'm-1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(mem_count, 0);

        now_epoch_before = Utc::now().timestamp() as f64;
        s.soft_delete("m-1").unwrap();
    }

    // get_deleted_at on unified reads nodes.deleted_at (REAL) → RFC3339.
    let unified = Storage::with_unified_substrate(&db, true).unwrap();
    let got = unified.get_deleted_at("m-1").unwrap();
    let rfc = got.expect("soft-deleted unified row → Some(rfc3339)");

    // Parses as a valid RFC3339 instant.
    let parsed: DateTime<Utc> = DateTime::parse_from_rfc3339(&rfc)
        .expect("get_deleted_at must return valid RFC3339")
        .with_timezone(&Utc);

    // The returned instant matches the REAL epoch stored in nodes
    // (round-trip, no format drift). Allow 2s slack for the call window.
    let raw_epoch: f64 = unified
        .conn()
        .query_row(
            "SELECT deleted_at FROM nodes WHERE id = 'm-1' AND node_kind = 'memory'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        (parsed.timestamp() as f64 - raw_epoch).abs() < 2.0,
        "RFC3339 ({}) must round-trip the REAL epoch ({raw_epoch})",
        parsed.timestamp()
    );
    assert!(
        raw_epoch >= now_epoch_before - 2.0,
        "deleted_at epoch must be recent"
    );
}

#[test]
fn iss199_soft_delete_get_deleted_at_legacy_unchanged() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let mut s = Storage::new(&db).unwrap();
    s.add(&rec("m-1", "to delete"), "default").unwrap();
    s.soft_delete("m-1").unwrap();

    let legacy = Storage::with_unified_substrate(&db, false).unwrap();
    let rfc = legacy
        .get_deleted_at("m-1")
        .unwrap()
        .expect("legacy soft-deleted → Some(rfc3339)");
    DateTime::parse_from_rfc3339(&rfc).expect("legacy returns RFC3339 (memories TEXT)");
}
