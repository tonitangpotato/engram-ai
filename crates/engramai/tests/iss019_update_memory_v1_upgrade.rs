//! ISS-019 design §6 compliance test: `update_memory` must upgrade
//! v1 legacy-layout rows to v2 `{engram, user}` layout as a write-path
//! side effect — matching the contract already honored by
//! `merge_enriched_into`.
//!
//! Before this fix, `update_memory` wrote audit keys
//! (`previous_content`, `update_reason`, `updated_at`) at the top
//! level of whatever metadata shape the row had. For v1 rows this
//! polluted the flat layout; for v2 rows it broke the
//! `{engram, user}` namespace boundary.
//!
//! After the fix, audit data lives under `user.update_audit[]` as a
//! chronological list, and v1 rows are rewritten to v2 on first touch.

use chrono::Utc;
use engramai::memory::Memory;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use serde_json::json;

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

/// Construct a raw v1-layout `MemoryRecord` with flat dimensional
/// fields at the top level of `metadata` (participants, temporal,
/// causation, etc.). This is the shape written by engram versions
/// before ISS-019 Step 4.
fn make_v1_record(id: &str, content: &str) -> MemoryRecord {
    let v1_metadata = json!({
        // v1 flat layout: dimensional fields at top level.
        // Note: v1 `participants` is stored as a CSV string (not array) —
        // see Dimensions::participants: Option<String> and
        // Dimensions::from_legacy_metadata which uses get_string().
        "participants": "alice, bob",
        "temporal": "2026-04-22",
        "causation": "meeting scheduled",
        "domain": "communication",
        "tags": ["planning"],
        // Caller-supplied keys that should round-trip into user_metadata.
        "source_chat": "telegram",
        "session_id": "sess-42",
    });

    let now = Utc::now();
    MemoryRecord {
        id: id.to_string(),
        content: content.to_string(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Working,
        created_at: now,
        access_times: vec![now],
        working_strength: 1.0,
        core_strength: 0.0,
        importance: 0.6,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "test-fixture".to_string(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: Some(v1_metadata),
    }
}

/// Construct a v2-layout `MemoryRecord` with `{engram, user}` shape.
fn make_v2_record(id: &str, content: &str) -> MemoryRecord {
    let v2_metadata = json!({
        "engram": {
            "version": 2,
            "dimensions": {
                "participants": ["alice"],
                "temporal": "2026-04-22",
                "domain": "communication",
                "confidence": 0.8,
            },
            "merge_count": 0,
            "merge_history": [],
        },
        "user": {
            "session_id": "sess-99",
        },
    });

    let now = Utc::now();
    MemoryRecord {
        id: id.to_string(),
        content: content.to_string(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Working,
        created_at: now,
        access_times: vec![now],
        working_strength: 1.0,
        core_strength: 0.0,
        importance: 0.7,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "test-fixture".to_string(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: Some(v2_metadata),
    }
}

// ───────────────────────────────────────────────────────────────────
// Test 1: v1 row gets upgraded to v2 layout by update_memory
// ───────────────────────────────────────────────────────────────────

#[test]
fn update_upgrades_v1_row_to_v2_layout() {
    let mut mem = new_mem();

    // Insert a raw v1 row directly through Storage.
    let v1 = make_v1_record("mem-v1-001", "alice meets bob on 2026-04-22");
    mem.storage_mut()
        .add(&v1, "default")
        .expect("insert v1 record");

    // Call update_memory — this should rewrite to v2.
    mem.update_memory("mem-v1-001", "alice meets bob on 2026-04-23 (rescheduled)", "date corrected")
        .expect("update_memory succeeds on v1 row");

    // Read back and assert v2 shape.
    let after = mem
        .get("mem-v1-001")
        .expect("get ok")
        .expect("row still exists");
    let meta = after.metadata.expect("metadata present after update");

    // Shape assertion: top-level has `engram` + `user`, nothing else.
    let top = meta.as_object().expect("metadata is object");
    assert!(
        top.contains_key("engram"),
        "v2 layout requires `engram` namespace, got: {:#?}",
        top.keys().collect::<Vec<_>>()
    );
    assert!(
        top.contains_key("user"),
        "v2 layout requires `user` namespace, got: {:#?}",
        top.keys().collect::<Vec<_>>()
    );

    // Critical: audit keys must NOT sit at the top level anymore.
    assert!(
        !top.contains_key("previous_content"),
        "audit leak: `previous_content` must live under user.update_audit, not top-level"
    );
    assert!(
        !top.contains_key("update_reason"),
        "audit leak: `update_reason` must live under user.update_audit, not top-level"
    );
    assert!(
        !top.contains_key("updated_at"),
        "audit leak: `updated_at` must live under user.update_audit, not top-level"
    );

    // v1 flat dimensional fields must NOT sit at the top level after upgrade.
    for legacy_key in &["participants", "temporal", "causation", "domain", "tags"] {
        assert!(
            !top.contains_key(*legacy_key),
            "v1 leak: `{}` must live under engram.dimensions, not top-level (still v1 layout?)",
            legacy_key
        );
    }

    // engram.version must be 2.
    let engram = meta.get("engram").expect("engram namespace");
    assert_eq!(
        engram.get("version").and_then(|v| v.as_u64()),
        Some(2),
        "engram.version must be 2 after upgrade, got: {:?}",
        engram.get("version")
    );

    // Dimensions survived the round-trip (participants still carry alice, bob).
    // v1 participants is a CSV string; after upgrade it serializes as a
    // string under engram.dimensions.participants.
    let dims = engram
        .get("dimensions")
        .and_then(|d| d.as_object())
        .expect("engram.dimensions object");
    let parts_str = dims
        .get("participants")
        .and_then(|p| p.as_str())
        .expect("dimensions.participants string");
    assert!(
        parts_str.contains("alice") && parts_str.contains("bob"),
        "participants lost in upgrade: {:?}",
        parts_str
    );

    // Caller-supplied keys (source_chat, session_id) should round-trip into `user`.
    let user = meta
        .get("user")
        .and_then(|u| u.as_object())
        .expect("user namespace");
    assert_eq!(
        user.get("source_chat").and_then(|v| v.as_str()),
        Some("telegram"),
        "user_metadata lost source_chat during v1→v2 upgrade"
    );
    assert_eq!(
        user.get("session_id").and_then(|v| v.as_str()),
        Some("sess-42"),
        "user_metadata lost session_id during v1→v2 upgrade"
    );

    // Audit entry present under user.update_audit[0].
    let audit = user
        .get("update_audit")
        .and_then(|a| a.as_array())
        .expect("user.update_audit array present");
    assert_eq!(audit.len(), 1, "expected one audit entry, got {}", audit.len());
    let entry = audit[0].as_object().expect("audit entry is object");
    assert_eq!(
        entry.get("previous_content").and_then(|v| v.as_str()),
        Some("alice meets bob on 2026-04-22")
    );
    assert_eq!(
        entry.get("reason").and_then(|v| v.as_str()),
        Some("date corrected")
    );
    assert!(
        entry.get("updated_at").and_then(|v| v.as_str()).is_some(),
        "audit entry missing updated_at timestamp"
    );

    // Content column reflects the new text (and core_fact tracks it via EnrichedMemory invariant).
    assert_eq!(after.content, "alice meets bob on 2026-04-23 (rescheduled)");
    let core_fact = dims.get("core_fact").and_then(|v| v.as_str());
    assert_eq!(
        core_fact,
        Some("alice meets bob on 2026-04-23 (rescheduled)"),
        "core_fact invariant violated: must equal content"
    );
}

// ───────────────────────────────────────────────────────────────────
// Test 2: v2 row stays v2 after update, no namespace pollution
// ───────────────────────────────────────────────────────────────────

#[test]
fn update_preserves_v2_layout_without_pollution() {
    let mut mem = new_mem();

    let v2 = make_v2_record("mem-v2-001", "alice called bob");
    mem.storage_mut()
        .add(&v2, "default")
        .expect("insert v2 record");

    mem.update_memory("mem-v2-001", "alice called bob twice", "clarification")
        .expect("update_memory ok on v2 row");

    let after = mem
        .get("mem-v2-001")
        .expect("get ok")
        .expect("row exists");
    let meta = after.metadata.expect("metadata present");
    let top = meta.as_object().expect("object");

    // engram namespace still pure — no audit keys leaked in.
    let engram = top.get("engram").and_then(|v| v.as_object()).expect("engram ns");
    for polluting_key in &["previous_content", "update_reason", "updated_at", "reason"] {
        assert!(
            !engram.contains_key(*polluting_key),
            "engram namespace polluted with `{}`",
            polluting_key
        );
    }
    assert_eq!(
        engram.get("version").and_then(|v| v.as_u64()),
        Some(2),
        "engram.version must remain 2"
    );

    // Audit landed under user.update_audit.
    let user = top.get("user").and_then(|v| v.as_object()).expect("user ns");
    let audit = user
        .get("update_audit")
        .and_then(|a| a.as_array())
        .expect("audit array");
    assert_eq!(audit.len(), 1);
    assert_eq!(
        audit[0].get("previous_content").and_then(|v| v.as_str()),
        Some("alice called bob")
    );

    // Pre-existing user.session_id survived.
    assert_eq!(
        user.get("session_id").and_then(|v| v.as_str()),
        Some("sess-99"),
        "pre-existing user metadata lost during update"
    );
}

// ───────────────────────────────────────────────────────────────────
// Test 3: multiple updates accumulate audit history chronologically
// ───────────────────────────────────────────────────────────────────

#[test]
fn repeated_updates_accumulate_audit_history() {
    let mut mem = new_mem();

    let v2 = make_v2_record("mem-v2-002", "version 1 text");
    mem.storage_mut().add(&v2, "default").expect("insert");

    mem.update_memory("mem-v2-002", "version 2 text", "first edit").unwrap();
    mem.update_memory("mem-v2-002", "version 3 text", "second edit").unwrap();
    mem.update_memory("mem-v2-002", "version 4 text", "third edit").unwrap();

    let after = mem.get("mem-v2-002").unwrap().unwrap();
    let meta = after.metadata.unwrap();
    let audit = meta
        .pointer("/user/update_audit")
        .and_then(|a| a.as_array())
        .expect("audit array present");

    assert_eq!(audit.len(), 3, "expected 3 audit entries");

    // Chronological order: entry[0] captured v1→v2 transition, [1] v2→v3, [2] v3→v4.
    assert_eq!(
        audit[0].get("previous_content").and_then(|v| v.as_str()),
        Some("version 1 text")
    );
    assert_eq!(
        audit[0].get("reason").and_then(|v| v.as_str()),
        Some("first edit")
    );
    assert_eq!(
        audit[1].get("previous_content").and_then(|v| v.as_str()),
        Some("version 2 text")
    );
    assert_eq!(
        audit[2].get("previous_content").and_then(|v| v.as_str()),
        Some("version 3 text")
    );
    assert_eq!(
        audit[2].get("reason").and_then(|v| v.as_str()),
        Some("third edit")
    );

    // Final content reflects the last edit.
    assert_eq!(after.content, "version 4 text");
}

// ───────────────────────────────────────────────────────────────────
// Test 4: audit history capped at 10 entries (matches merge_history cap)
// ───────────────────────────────────────────────────────────────────

#[test]
fn audit_history_capped_at_ten_entries() {
    let mut mem = new_mem();

    let v2 = make_v2_record("mem-cap-001", "initial");
    mem.storage_mut().add(&v2, "default").unwrap();

    // 15 updates — cap should retain only the most recent 10.
    for i in 1..=15 {
        let new_content = format!("rev {}", i);
        let reason = format!("edit {}", i);
        mem.update_memory("mem-cap-001", &new_content, &reason).unwrap();
    }

    let after = mem.get("mem-cap-001").unwrap().unwrap();
    let meta = after.metadata.unwrap();
    let audit = meta
        .pointer("/user/update_audit")
        .and_then(|a| a.as_array())
        .expect("audit array");

    assert_eq!(audit.len(), 10, "audit should be capped at 10");

    // Most recent entry reflects edit 15 (previous was rev 14 → now rev 15).
    assert_eq!(
        audit.last().unwrap().get("reason").and_then(|v| v.as_str()),
        Some("edit 15")
    );
    assert_eq!(
        audit.last().unwrap().get("previous_content").and_then(|v| v.as_str()),
        Some("rev 14")
    );

    // Oldest retained entry should be for "edit 6" (edits 1..=5 got evicted).
    assert_eq!(
        audit.first().unwrap().get("reason").and_then(|v| v.as_str()),
        Some("edit 6")
    );
}
