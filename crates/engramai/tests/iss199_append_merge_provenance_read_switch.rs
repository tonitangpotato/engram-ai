//! ISS-199 (Phase E read-cutover) — `append_merge_provenance`
//! read-modify-write read-switch contract.
//!
//! `append_merge_provenance` RMWs the memory's free-form JSON
//! (`engram.merge_history`). Under `unified_substrate = true` (T34a) the
//! legacy `memories` row is absent, so the RMW targets `nodes.attributes`
//! — the same JSON object as `memories.metadata` plus the reserved
//! `_legacy_contradicts`/`_legacy_contradicted_by` keys. The merge-history
//! path is disjoint from those reserved keys, so the round-trip preserves
//! them.
//!
//! Acceptance contract:
//!
//!   1. Unified RMW without a `memories` row succeeds and the appended
//!      merge_history entry lands in `nodes.attributes`.
//!   2. Reserved keys (`_legacy_contradicts`) survive the RMW on unified.
//!   3. Legacy RMW still writes `memories.metadata` (regression guard).
//!   4. Bounded history: more than 10 appends keeps only the last 10.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use serde_json::Value;
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

fn merge_history(json: &str) -> Vec<Value> {
    let v: Value = serde_json::from_str(json).unwrap();
    v.get("engram")
        .and_then(|e| e.get("merge_history"))
        .and_then(|h| h.as_array())
        .cloned()
        .unwrap_or_default()
}

#[test]
fn iss199_unified_rmw_without_memories_row() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let mut s = Storage::with_unified_substrate(&db, true).unwrap();
    s.add(&rec("m-1", "prov test"), "default").unwrap();
    // No legacy memories row.
    let mem_count: i64 = s
        .conn()
        .query_row("SELECT COUNT(*) FROM memories WHERE id='m-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(mem_count, 0);

    s.append_merge_provenance("m-1", "donor-1", 0.91, true)
        .expect("unified RMW must succeed without memories row");

    let attrs: String = s
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id='m-1' AND node_kind='memory'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let hist = merge_history(&attrs);
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0]["source_id"], "donor-1");
    assert_eq!(hist[0]["content_updated"], true);
}

#[test]
fn iss199_reserved_keys_survive_rmw_on_unified() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let mut s = Storage::with_unified_substrate(&db, true).unwrap();
    let mut r = rec("m-1", "with contradiction");
    r.contradicts = Some("other-mem".into());
    s.add(&r, "default").unwrap();

    s.append_merge_provenance("m-1", "donor-1", 0.8, false)
        .unwrap();

    let attrs: String = s
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id='m-1' AND node_kind='memory'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let v: Value = serde_json::from_str(&attrs).unwrap();
    // Reserved legacy-contradicts key must survive the merge-history RMW.
    assert_eq!(
        v.get("_legacy_contradicts").and_then(|x| x.as_str()),
        Some("other-mem"),
        "reserved key must survive RMW; attrs={attrs}"
    );
    assert_eq!(merge_history(&attrs).len(), 1);
}

#[test]
fn iss199_legacy_rmw_still_writes_memories() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let mut s = Storage::new(&db).unwrap();
    s.add(&rec("m-1", "prov test"), "default").unwrap();
    s.append_merge_provenance("m-1", "donor-1", 0.91, true)
        .unwrap();

    let meta: Option<String> = s
        .conn()
        .query_row("SELECT metadata FROM memories WHERE id='m-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let hist = merge_history(meta.as_deref().unwrap());
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0]["source_id"], "donor-1");
}

#[test]
fn iss199_history_bounded_to_10_on_unified() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let mut s = Storage::with_unified_substrate(&db, true).unwrap();
    s.add(&rec("m-1", "prov test"), "default").unwrap();

    for i in 0..15 {
        s.append_merge_provenance("m-1", &format!("donor-{i}"), 0.5, false)
            .unwrap();
    }

    let attrs: String = s
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id='m-1' AND node_kind='memory'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let hist = merge_history(&attrs);
    assert_eq!(hist.len(), 10, "history must be capped at 10");
    // Oldest dropped: the first surviving entry is donor-5.
    assert_eq!(hist[0]["source_id"], "donor-5");
    assert_eq!(hist[9]["source_id"], "donor-14");
}
