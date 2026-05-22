//! T19 — Phase C backfill driver: memories → nodes.
//!
//! Acceptance per design.md §5.3 + invariants stated in
//! `crates/engramai/src/substrate/backfill.rs` module docs:
//!
//!   1. Every legacy `memories` row gets a matching `nodes` row with
//!      `node_kind='memory'`, byte-equal on the scalar fields covered
//!      by `Storage::insert_memory_node_row`.
//!   2. Self-referential supersession: rows with
//!      `memories.superseded_by = X` end up with
//!      `nodes.superseded_by = X` after Pass 2. The legacy `''`
//!      sentinel maps to SQL `NULL`.
//!   3. Idempotency: a second invocation inserts zero new rows
//!      (`rows_skipped_existing` equals `rows_read`).
//!   4. Field-mapping parity with the T12 dual-write path: a memory
//!      added via `Storage::add` (dual-written through the helper)
//!      and a memory inserted only into `memories` and then
//!      backfilled through T19 must produce identical `nodes` rows.
//!      This is the key invariant that justifies extracting
//!      `insert_memory_node_row` as a single source of truth.
//!   5. Audit row in `backfill_runs`: `started_at` < `finished_at`,
//!      counts non-NULL, and the sum invariant
//!      (`rows_read = inserted + skipped + failed`) holds.
//!   6. Namespace filter: when invoked with `Some(ns)`, rows in
//!      other namespaces are NOT touched (neither inserted nor
//!      updated in Pass 2).

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::backfill::{
    backfill_memories_to_nodes, fetch_backfill_run, BackfillRun,
};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

/// Build a fully-populated memory record. The fixed `created_at`
/// makes parity assertions deterministic.
fn sample_record(id: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 11, 0, 0).unwrap();
    let occurred = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: format!("content of {id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: Some(occurred),
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.7,
        pinned: false,
        consolidation_count: 1,
        last_consolidated: Some(created),
        source: "phase-c-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: Some(serde_json::json!({"tag": "phase-c"})),
    }
}

/// Bypass `Storage::add` to seed a memory ONLY in the legacy table.
/// We do this by deleting the matching `nodes` row right after the
/// normal dual-write — `add` is the only public path to populate
/// `memories`, and going through it ensures field encoding is
/// realistic. Phase B dual-write writes both `memories` and `nodes`;
/// stripping `nodes` afterwards simulates a row that pre-dates T12.
fn seed_legacy_only(storage: &mut Storage, record: &MemoryRecord, namespace: &str) {
    storage.add(record, namespace).expect("add");
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = ?", params![record.id])
        .expect("strip nodes row");
}

#[test]
fn t19_backfill_inserts_missing_rows_and_skips_existing() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Three memories: A and B are "legacy-only" (simulating
    // pre-T12 data); C goes through normal dual-write and so already
    // has a `nodes` row before backfill runs.
    let a = sample_record("mem-a");
    let b = sample_record("mem-b");
    let c = sample_record("mem-c");
    seed_legacy_only(&mut storage, &a, "default");
    seed_legacy_only(&mut storage, &b, "default");
    storage.add(&c, "default").unwrap(); // dual-written, nodes row present

    // Sanity: 3 rows in memories, 1 row in nodes (only C).
    {
        let conn = storage.conn();
        let mem_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        let nodes_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes WHERE node_kind='memory'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mem_count, 3, "fixture has 3 memories");
        assert_eq!(nodes_count, 1, "fixture has 1 pre-existing nodes row (C)");
    }

    let run = backfill_memories_to_nodes(&mut storage, None).expect("backfill");

    assert_eq!(run.legacy_table, "memories");
    assert_eq!(run.rows_read, 3, "every legacy memory should be iterated");
    assert_eq!(run.rows_inserted, 2, "A and B should be newly inserted (C already there)");
    assert_eq!(run.rows_skipped_existing, 1, "C should be skipped as existing");
    assert_eq!(run.rows_failed, 0);

    // After backfill: all 3 memories should have a matching nodes row.
    let conn = storage.conn();
    let nodes_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM nodes WHERE node_kind='memory'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(nodes_count, 3, "every memory must have a matching node after backfill");

    // Idempotency: a second run should insert zero rows.
    let run2 = backfill_memories_to_nodes(&mut storage, None).expect("backfill rerun");
    assert_eq!(run2.rows_read, 3);
    assert_eq!(run2.rows_inserted, 0, "re-run must be a no-op (idempotent)");
    assert_eq!(run2.rows_skipped_existing, 3);

    // Audit row was written and finished_at is non-NULL.
    let conn = storage.conn();
    let (started, finished, notes): (f64, Option<f64>, String) = conn
        .query_row(
            "SELECT started_at, finished_at, notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!(finished.is_some(), "finished_at must be set after a successful run");
    assert!(
        finished.unwrap() >= started,
        "finished_at ({}) must be >= started_at ({})",
        finished.unwrap(),
        started
    );
    let notes_json: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(notes_json["driver"], "backfill_memories_to_nodes");
    assert!(notes_json["namespace_filter"].is_null(), "no filter = JSON null");

    // fetch_backfill_run round-trip.
    let fetched = fetch_backfill_run(&storage, &run.run_id).unwrap().unwrap();
    assert_eq!(fetched.rows_read, run.rows_read);
    assert_eq!(fetched.rows_inserted, run.rows_inserted);
    assert_eq!(fetched.rows_skipped_existing, run.rows_skipped_existing);
}

#[test]
fn t19_pass2_propagates_supersession_and_converts_empty_to_null() {
    // memories.superseded_by uses '' as the not-superseded sentinel
    // and a real id when superseded. After Pass 2:
    //   '' (or NULL)  →  nodes.superseded_by = NULL
    //   real id       →  nodes.superseded_by = real id
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let new = sample_record("mem-new");
    let old = sample_record("mem-old");
    let plain = sample_record("mem-plain");
    seed_legacy_only(&mut storage, &new, "default");
    seed_legacy_only(&mut storage, &old, "default");
    seed_legacy_only(&mut storage, &plain, "default");

    // Manually establish supersession in the legacy table only
    // (mimicking a pre-T12 supersede). We bypass Storage::supersede
    // because that path now dual-updates `nodes`, which would defeat
    // the test — we want to verify backfill's Pass 2 can derive the
    // FK from `memories` alone.
    storage
        .conn()
        .execute(
            "UPDATE memories SET superseded_by = 'mem-new' WHERE id = 'mem-old'",
            [],
        )
        .unwrap();

    let _ = backfill_memories_to_nodes(&mut storage, None).expect("backfill");

    let conn = storage.conn();
    // mem-old → supersedes by mem-new
    let old_sup: Option<String> = conn
        .query_row(
            "SELECT superseded_by FROM nodes WHERE id = 'mem-old'",
            [],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap();
    assert_eq!(
        old_sup.as_deref(),
        Some("mem-new"),
        "Pass 2 must propagate memories.superseded_by into nodes.superseded_by"
    );
    // mem-new (the supersedor itself) and mem-plain are not superseded → NULL on nodes.
    for unmsup_id in ["mem-new", "mem-plain"] {
        let nsup: Option<String> = conn
            .query_row(
                "SELECT superseded_by FROM nodes WHERE id = ?",
                params![unmsup_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(
            nsup, None,
            "{unmsup_id} memories.superseded_by='' must convert to nodes.superseded_by=NULL"
        );
    }
}

#[test]
fn t19_parity_with_t12_dual_write_byte_equal_node_rows() {
    // The strongest invariant: a memory written via Storage::add
    // (Phase B dual-write) and an identical memory inserted only
    // into `memories` and then backfilled MUST produce equal `nodes`
    // rows for every scalar column covered by
    // `insert_memory_node_row`. If this drifts, Phase D read cutover
    // would see different state depending on whether each memory
    // was added before or after the backfill ran — exactly the kind
    // of silent inconsistency the helper extraction prevents.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let dual = sample_record("mem-dual"); // will go through Storage::add
    let mut back = sample_record("mem-back");
    // Make `back` differ only by id so that scalar fields are
    // comparable column-by-column.
    back.content = dual.content.clone();
    back.created_at = dual.created_at;
    back.occurred_at = dual.occurred_at;
    back.last_consolidated = dual.last_consolidated;

    // Path A: dual-write through Storage::add.
    storage.add(&dual, "ns-x").unwrap();

    // Path B: legacy-only, then backfill.
    seed_legacy_only(&mut storage, &back, "ns-x");
    let _ = backfill_memories_to_nodes(&mut storage, None).expect("backfill");

    // Compare every column relevant to the memory→nodes projection.
    let conn = storage.conn();
    let read_row = |id: &str| -> (
        String, String, String, String, String, String, String,
        Option<f64>, f64, f64, Option<f64>,
        f64, f64, f64,
        i64, i64, String, Option<String>,
    ) {
        conn.query_row(
            r#"
            SELECT node_kind, namespace, layer, memory_type, content, summary, attributes,
                   occurred_at, created_at, updated_at, last_consolidated,
                   working_strength, core_strength, importance,
                   consolidation_count, pinned,
                   source, superseded_by
            FROM nodes WHERE id = ?
            "#,
            params![id],
            |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?,
                    r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?,
                    r.get(11)?, r.get(12)?, r.get(13)?,
                    r.get(14)?, r.get(15)?, r.get(16)?, r.get(17)?,
                ))
            },
        )
        .unwrap()
    };

    let a = read_row("mem-dual");
    let b = read_row("mem-back");

    // Same projection contract for every field except `updated_at`,
    // which legitimately differs (dual-write sets it at add time;
    // backfill sets it at backfill time). All other columns must
    // match byte-for-byte.
    assert_eq!(a.0, b.0, "node_kind");
    assert_eq!(a.1, b.1, "namespace");
    assert_eq!(a.2, b.2, "layer");
    assert_eq!(a.3, b.3, "memory_type");
    assert_eq!(a.4, b.4, "content");
    assert_eq!(a.5, b.5, "summary");
    assert_eq!(a.6, b.6, "attributes JSON");
    assert_eq!(a.7, b.7, "occurred_at");
    assert_eq!(a.8, b.8, "created_at");
    // a.9 (updated_at) intentionally NOT asserted; see comment above.
    assert_eq!(a.10, b.10, "last_consolidated");
    assert_eq!(a.11, b.11, "working_strength");
    assert_eq!(a.12, b.12, "core_strength");
    assert_eq!(a.13, b.13, "importance");
    assert_eq!(a.14, b.14, "consolidation_count");
    assert_eq!(a.15, b.15, "pinned");
    assert_eq!(a.16, b.16, "source");
    assert_eq!(a.17, b.17, "superseded_by");
    // Both should be NULL — neither was superseded.
    assert_eq!(a.17, None);
}

#[test]
fn t19_namespace_filter_does_not_touch_other_namespaces() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let a = sample_record("mem-ns-a");
    let b = sample_record("mem-ns-b");
    seed_legacy_only(&mut storage, &a, "ns-a");
    seed_legacy_only(&mut storage, &b, "ns-b");

    let run = backfill_memories_to_nodes(&mut storage, Some("ns-a")).expect("backfill");
    assert_eq!(run.rows_read, 1, "filter should restrict iteration to ns-a only");
    assert_eq!(run.rows_inserted, 1);

    let conn = storage.conn();
    let ns_a_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = 'mem-ns-a' AND node_kind='memory'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let ns_b_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = 'mem-ns-b' AND node_kind='memory'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ns_a_present, 1, "ns-a row should be inserted");
    assert_eq!(ns_b_present, 0, "ns-b row must NOT be touched by a filtered backfill");

    // Audit row should record the filter.
    let conn = storage.conn();
    let notes: String = conn
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(parsed["namespace_filter"], "ns-a");
}

#[test]
fn t19_empty_table_completes_cleanly() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let run = backfill_memories_to_nodes(&mut storage, None).expect("backfill empty");
    assert_eq!(run.rows_read, 0);
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_skipped_existing, 0);
    assert_eq!(run.rows_failed, 0);

    // Audit row should still exist with finished_at set.
    let fetched = fetch_backfill_run(&storage, &run.run_id).unwrap();
    assert!(fetched.is_some(), "empty-table runs must still record an audit row");
}

#[test]
fn t19_pass2_skips_dangling_supersession_targets() {
    // R2.1 regression: legacy `memories.superseded_by` is not
    // namespace-constrained. A row in ns-foo can technically point at
    // a row in ns-bar (or at a deleted id). If Pass 2 blindly copied
    // such a reference into `nodes.superseded_by`, the FK to
    // `nodes(id)` would fail (FKs are enforced).
    //
    // The Pass 2 EXISTS guard must skip any edge whose target is not
    // (yet) in `nodes`. The skipped edge stays as the legacy default
    // (NULL) on the unified side and gets picked up on a subsequent
    // backfill that brings the target into scope.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let foo = sample_record("mem-foo");
    let bar = sample_record("mem-bar");
    seed_legacy_only(&mut storage, &foo, "ns-foo");
    seed_legacy_only(&mut storage, &bar, "ns-bar");
    storage
        .conn()
        .execute(
            "UPDATE memories SET superseded_by = 'mem-bar' WHERE id = 'mem-foo'",
            [],
        )
        .unwrap();

    let run = backfill_memories_to_nodes(&mut storage, Some("ns-foo"))
        .expect("filtered backfill must not violate FK on cross-NS supersession");
    assert_eq!(run.rows_inserted, 1);

    let foo_sup: Option<String> = storage
        .conn()
        .query_row(
            "SELECT superseded_by FROM nodes WHERE id = 'mem-foo'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        foo_sup, None,
        "Pass 2 must skip cross-NS supersession when target is not yet in nodes"
    );

    // Re-run unfiltered: mem-bar lands, then Pass 2 completes the edge.
    let _ = backfill_memories_to_nodes(&mut storage, None).expect("unfiltered backfill");
    let foo_sup_2: Option<String> = storage
        .conn()
        .query_row(
            "SELECT superseded_by FROM nodes WHERE id = 'mem-foo'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        foo_sup_2.as_deref(),
        Some("mem-bar"),
        "second (unfiltered) run must complete the deferred supersession edge"
    );
}

#[test]
fn t19_counter_invariant_holds() {
    // The BackfillRun internal assert_counter_invariant is the last
    // line of defense against miscounting. Exercise it through a
    // realistic mixed-state seed.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // 5 memories: 2 already in nodes (dual-written), 3 legacy-only.
    for i in 0..2 {
        let r = sample_record(&format!("mem-dual-{i}"));
        storage.add(&r, "default").unwrap();
    }
    for i in 0..3 {
        let r = sample_record(&format!("mem-legacy-{i}"));
        seed_legacy_only(&mut storage, &r, "default");
    }

    let run: BackfillRun = backfill_memories_to_nodes(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_read, 5);
    assert_eq!(run.rows_inserted, 3);
    assert_eq!(run.rows_skipped_existing, 2);
    assert_eq!(run.rows_failed, 0);
    // assert_counter_invariant is called inside the driver — getting
    // here means the sum invariant held.
}

// ---------------------------------------------------------------------------
// ISS-112 §B cross-driver test-gap audit applied to T19 (memories).
//
// The §B patterns (mutated-metadata rerun, reserved column-key shadowing,
// empty-string column, corrupt existing attributes) were first applied to
// T21 (entities). This block extends the relevant subset to T19. T19 is
// Pass-1-only for the data write (no metadata merge — `INSERT OR IGNORE`
// drops conflicts), so the mutated-metadata RERUN pattern collapses into
// the existing `t19_backfill_inserts_missing_rows_and_skips_existing`
// idempotency assertion. The remaining gap that DOES apply is the
// reserved-key shadowing pattern: `merge_legacy_memory_attributes` stamps
// `_legacy_contradicts` / `_legacy_contradicted_by` into the attributes
// JSON. We pin both directions of the shadowing contract:
//
//   1. When BOTH the legacy column and metadata supply the reserved key,
//      the column value wins (system-owned shim overrides user metadata).
//   2. When ONLY metadata supplies the reserved key (column is NULL), the
//      metadata value passes through unchanged.
//
// Direction (2) is the current "soft" behavior. If a future refactor adds
// a formal reserved-key gate (see storage.rs:1957–1973 module comment),
// this test will break loudly — which is the intent.
// ---------------------------------------------------------------------------

#[test]
fn iss112_b_t19_reserved_legacy_key_in_metadata_does_not_shadow_column() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // The supersession target must exist as a real `memories` row,
    // otherwise the legacy column would be unsettable through `add`.
    let target = sample_record("mem-real-target");
    storage.add(&target, "default").expect("seed target");

    // Construct a memory whose user-supplied `metadata` declares the
    // reserved `_legacy_contradicts` key with a bogus value, AND whose
    // real `contradicts` column points at a different (real) target.
    // After backfill, the column value must shadow the metadata value.
    let mut subject = sample_record("mem-with-reserved-key");
    subject.contradicts = Some("mem-real-target".to_string());
    subject.metadata = Some(serde_json::json!({
        "_legacy_contradicts": "bogus-fake-target",
        "tag": "phase-c",
    }));
    seed_legacy_only(&mut storage, &subject, "default");

    let run = backfill_memories_to_nodes(&mut storage, None).expect("backfill");
    assert!(run.rows_inserted >= 1, "subject must be inserted");

    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id = 'mem-with-reserved-key'",
            [],
            |r| r.get(0),
        )
        .expect("read attributes");
    let parsed: serde_json::Value = serde_json::from_str(&attrs).expect("attrs json");

    assert_eq!(
        parsed.get("_legacy_contradicts").and_then(|v| v.as_str()),
        Some("mem-real-target"),
        "column value must shadow user-supplied reserved key in metadata",
    );
    assert_eq!(
        parsed.get("tag").and_then(|v| v.as_str()),
        Some("phase-c"),
        "non-reserved user metadata keys must survive intact",
    );
}

#[test]
fn iss112_b_t19_metadata_legacy_key_passes_through_when_column_null() {
    // Pins the current "soft" behavior: when the legacy column is NULL
    // / empty, the user-supplied `_legacy_contradicts` value in metadata
    // is NOT stripped. This is documented in storage.rs:1969–1972 as a
    // known reserved-key shim limitation. If a future formal
    // reserved-key gate is added, this test will fail and the gate
    // implementation must update the assertion (and the docs).
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let mut subject = sample_record("mem-metadata-only-legacy");
    subject.contradicts = None;
    subject.contradicted_by = None;
    subject.metadata = Some(serde_json::json!({
        "_legacy_contradicts": "passthrough-from-metadata",
        "tag": "phase-c",
    }));
    seed_legacy_only(&mut storage, &subject, "default");

    let run = backfill_memories_to_nodes(&mut storage, None).expect("backfill");
    assert!(run.rows_inserted >= 1);

    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id = 'mem-metadata-only-legacy'",
            [],
            |r| r.get(0),
        )
        .expect("read attributes");
    let parsed: serde_json::Value = serde_json::from_str(&attrs).expect("attrs json");

    assert_eq!(
        parsed.get("_legacy_contradicts").and_then(|v| v.as_str()),
        Some("passthrough-from-metadata"),
        "metadata-supplied reserved key must pass through when column is NULL \
         (pins documented soft behavior — break this loudly if a formal \
         reserved-key gate is added; see storage.rs:1969)",
    );
    assert_eq!(
        parsed.get("tag").and_then(|v| v.as_str()),
        Some("phase-c"),
    );
}
