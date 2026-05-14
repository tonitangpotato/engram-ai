// ISS-121: soft_delete must dual-write deleted_at to both `memories`
// and `nodes` so liveness filters (`WHERE deleted_at IS NULL`) stay in
// lock-step across substrates.
//
// Before this fix soft_delete was a single-table UPDATE on `memories`
// — a Phase B dual-write gap surfaced during Phase D reader scoping
// (see .gid/features/v04-unified-substrate/PHASE-D-READER-AUDIT.md).

use engramai::storage::Storage;
use engramai::types::{MemoryRecord, MemoryType, MemoryLayer};
use chrono::Utc;
use rusqlite::params;

fn make_storage() -> Storage {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    Storage::new(tmp.path()).expect("open storage")
}

fn seed_memory(storage: &mut Storage, id: &str, ns: &str) {
    let rec = MemoryRecord {
        id: id.to_string(),
        content: format!("content for {id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Working,
        created_at: Utc::now(),
        occurred_at: None,
        access_times: vec![],
        working_strength: 1.0,
        core_strength: 0.0,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: String::new(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    };
    storage.add(&rec, ns).expect("add memory");
}

fn read_memories_deleted_at(storage: &Storage, id: &str) -> Option<String> {
    storage
        .conn()
        .query_row(
            "SELECT deleted_at FROM memories WHERE id = ?",
            params![id],
            |r: &rusqlite::Row| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
}

fn read_nodes_deleted_at(storage: &Storage, id: &str) -> Option<f64> {
    storage
        .conn()
        .query_row(
            "SELECT deleted_at FROM nodes WHERE id = ? AND node_kind = 'memory'",
            params![id],
            |r: &rusqlite::Row| r.get::<_, Option<f64>>(0),
        )
        .ok()
        .flatten()
}

#[test]
fn iss121_soft_delete_writes_both_substrates() {
    let mut s = make_storage();
    seed_memory(&mut s, "m1", "default");

    // Pre: both columns NULL.
    assert!(read_memories_deleted_at(&s, "m1").is_none(),
            "pre-condition: memories.deleted_at should be NULL");
    assert!(read_nodes_deleted_at(&s, "m1").is_none(),
            "pre-condition: nodes.deleted_at should be NULL");

    s.soft_delete("m1").expect("soft_delete");

    // Post: both populated.
    let memories_ts = read_memories_deleted_at(&s, "m1");
    let nodes_ts = read_nodes_deleted_at(&s, "m1");
    assert!(memories_ts.is_some(), "memories.deleted_at populated");
    assert!(nodes_ts.is_some(),    "nodes.deleted_at populated (THIS IS ISS-121)");
}

#[test]
fn iss121_soft_delete_timestamps_agree_to_one_second() {
    let mut s = make_storage();
    seed_memory(&mut s, "m1", "default");
    s.soft_delete("m1").expect("soft_delete");

    let memories_rfc = read_memories_deleted_at(&s, "m1").expect("memories.deleted_at set");
    let nodes_epoch  = read_nodes_deleted_at(&s, "m1").expect("nodes.deleted_at set");

    // RFC3339 → epoch and compare. Sub-second drift is allowed because
    // RFC3339 serialization rounds, but we expect them within 1 s of
    // each other — they were both derived from the same `Utc::now()`.
    let memories_dt = chrono::DateTime::parse_from_rfc3339(&memories_rfc)
        .expect("parse RFC3339")
        .with_timezone(&chrono::Utc);
    let memories_epoch_f = memories_dt.timestamp() as f64
        + (memories_dt.timestamp_subsec_nanos() as f64) / 1e9;

    let drift = (memories_epoch_f - nodes_epoch).abs();
    assert!(drift < 1.0,
            "soft_delete timestamps should agree within 1s (drift={drift})");
}

#[test]
fn iss121_liveness_filter_parity_after_soft_delete() {
    // Ingest 3 memories, soft-delete one, query liveness on both sides,
    // assert equal cardinality. This is the user-visible reason ISS-121
    // matters: count_memories_in_namespace (and friends) diverge between
    // substrates the moment any soft-delete happens.
    let mut s = make_storage();
    seed_memory(&mut s, "m_alive_a", "default");
    seed_memory(&mut s, "m_alive_b", "default");
    seed_memory(&mut s, "m_dead",    "default");

    s.soft_delete("m_dead").expect("soft_delete");

    let legacy_live: i64 = s.conn().query_row(
        "SELECT COUNT(*) FROM memories WHERE namespace = 'default' AND deleted_at IS NULL",
        [],
        |r| r.get(0),
    ).expect("legacy count");
    let unified_live: i64 = s.conn().query_row(
        "SELECT COUNT(*) FROM nodes WHERE node_kind = 'memory' AND namespace = 'default' AND deleted_at IS NULL",
        [],
        |r| r.get(0),
    ).expect("unified count");

    assert_eq!(legacy_live, 2, "legacy: 3 ingested, 1 deleted → 2 live");
    assert_eq!(unified_live, legacy_live,
               "liveness parity: legacy={legacy_live} unified={unified_live}");
}

#[test]
fn iss121_soft_delete_idempotent_on_already_deleted() {
    // Calling soft_delete twice on the same id should not error and
    // should leave both substrates populated. Second call overwrites
    // the timestamp; that's fine — same intent.
    let mut s = make_storage();
    seed_memory(&mut s, "m1", "default");
    s.soft_delete("m1").expect("first soft_delete");
    s.soft_delete("m1").expect("second soft_delete");

    assert!(read_memories_deleted_at(&s, "m1").is_some());
    assert!(read_nodes_deleted_at(&s, "m1").is_some());
}
