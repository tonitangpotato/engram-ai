//! T29.6 Phase D contract tests: FTS read-switch.
//!
//! When `unified_substrate=true`, `search_fts` and `search_fts_ns`
//! must read from `nodes_fts` (keyed by `nodes.fts_rowid`) instead
//! of legacy `memories_fts`. The returned `MemoryRecord` shape is
//! unchanged — only the inverted index used differs.
//!
//! Each test seeds via `Storage::add` which dual-writes to both
//! `memories` and `nodes` (T12), so `memories_fts` AND `nodes_fts`
//! get populated by their respective AFTER INSERT triggers. We
//! then open the DB with `unified_substrate=true` and assert the
//! returned IDs.
//!
//! Production caveat (see storage.rs `search_fts` rustdoc): the
//! flag stays opt-in until T26c production backfill closes the
//! pre-dual-write gap. These tests never touch prod data.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use tempfile::tempdir;

fn rec(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 4, 30, 0).unwrap();
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
        importance: 0.7,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "t29_6-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

fn seed(storage: &mut Storage, id: &str, ns: &str, content: &str) {
    let r = rec(id, content);
    storage.add(&r, ns).expect("add memory");
}

#[test]
fn t29_6_search_fts_default_ns_parity() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t29_6_a.db");

    // Seed: writer dual-writes to both legacy and unified FTS via triggers.
    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-1", "default", "the quick brown fox");
        seed(&mut s, "m-2", "default", "the lazy dog sleeps");
        seed(&mut s, "m-3", "default", "fox and hound");
    }

    // Read on legacy path (memories_fts).
    let legacy_hits = {
        let s = Storage::with_unified_substrate(&path, false).unwrap();
        let mut ids: Vec<String> =
            s.search_fts("fox", 10).expect("legacy").into_iter().map(|r| r.id).collect();
        ids.sort();
        ids
    };

    // Read on unified path (nodes_fts).
    let unified_hits = {
        let s = Storage::with_unified_substrate(&path, true).unwrap();
        let mut ids: Vec<String> =
            s.search_fts("fox", 10).expect("unified").into_iter().map(|r| r.id).collect();
        ids.sort();
        ids
    };

    assert_eq!(legacy_hits, vec!["m-1".to_string(), "m-3".to_string()]);
    assert_eq!(
        unified_hits, legacy_hits,
        "T29.6: unified search_fts must agree with legacy on dual-written corpus"
    );
}

#[test]
fn t29_6_search_fts_skips_deleted() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t29_6_b.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-1", "default", "alpha beta gamma");
        seed(&mut s, "m-2", "default", "alpha delta");
        s.soft_delete("m-1").expect("soft delete");
    }

    let unified = {
        let s = Storage::with_unified_substrate(&path, true).unwrap();
        s.search_fts("alpha", 10)
            .expect("unified")
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        unified,
        vec!["m-2".to_string()],
        "T29.6: unified search_fts must respect deleted_at"
    );
}

#[test]
fn t29_6_search_fts_skips_superseded() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t29_6_c.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-old", "default", "ancient lore");
        seed(&mut s, "m-new", "default", "modern lore");
        s.supersede("m-old", "m-new").expect("supersede");
    }

    let unified = {
        let s = Storage::with_unified_substrate(&path, true).unwrap();
        s.search_fts("lore", 10)
            .expect("unified")
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        unified,
        vec!["m-new".to_string()],
        "T29.6: unified search_fts must skip superseded heads"
    );
}

#[test]
fn t29_6_search_fts_ns_specific_namespace() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t29_6_d.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-a", "alpha", "shared keyword");
        seed(&mut s, "m-b", "beta", "shared keyword");
        seed(&mut s, "m-c", "alpha", "unique alpha");
    }

    let unified = {
        let s = Storage::with_unified_substrate(&path, true).unwrap();
        let mut ids: Vec<String> = s
            .search_fts_ns("shared", 10, Some("alpha"))
            .expect("unified ns")
            .into_iter()
            .map(|r| r.id)
            .collect();
        ids.sort();
        ids
    };

    assert_eq!(
        unified,
        vec!["m-a".to_string()],
        "T29.6: unified search_fts_ns must respect namespace filter"
    );
}

#[test]
fn t29_6_search_fts_ns_star_searches_all() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t29_6_e.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-a", "alpha", "needle in haystack");
        seed(&mut s, "m-b", "beta", "another needle");
        seed(&mut s, "m-c", "gamma", "no match here");
    }

    let unified = {
        let s = Storage::with_unified_substrate(&path, true).unwrap();
        let mut ids: Vec<String> = s
            .search_fts_ns("needle", 10, Some("*"))
            .expect("unified star")
            .into_iter()
            .map(|r| r.id)
            .collect();
        ids.sort();
        ids
    };

    assert_eq!(
        unified,
        vec!["m-a".to_string(), "m-b".to_string()],
        "T29.6: unified search_fts_ns with '*' must search all namespaces"
    );
}

#[test]
fn t29_6_search_fts_limit_honoured() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t29_6_f.db");

    {
        let mut s = Storage::new(&path).unwrap();
        for i in 0..5 {
            seed(&mut s, &format!("m-{}", i), "default", "repeated keyword");
        }
    }

    let unified = {
        let s = Storage::with_unified_substrate(&path, true).unwrap();
        s.search_fts("keyword", 3).expect("unified limit")
    };

    assert_eq!(
        unified.len(),
        3,
        "T29.6: unified search_fts must honour LIMIT clause"
    );
}

#[test]
fn t29_6_search_fts_empty_query_returns_empty() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t29_6_g.db");

    {
        let mut s = Storage::new(&path).unwrap();
        seed(&mut s, "m-1", "default", "anything");
    }

    let unified = {
        let s = Storage::with_unified_substrate(&path, true).unwrap();
        s.search_fts("", 10).expect("unified empty")
    };

    assert!(
        unified.is_empty(),
        "T29.6: empty query must short-circuit to empty result"
    );
}
