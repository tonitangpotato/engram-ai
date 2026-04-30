//! End-to-end regression test for ISS-072 / RUN-0007 GOAL-3:
//! `subject_kind_hint` / `object_kind_hint` on a `Triple` must round-trip
//! through the resolution pipeline into the persisted `graph_entities.kind`
//! column, and `attributes.kind_source` must record the breadcrumb
//! (`"TripleHint"` when a hint was supplied, `"Default"` when not).
//!
//! ## What this guards
//!
//! RUN-0007 found that the LLM never emits kind hints (because the
//! prompt doesn't ask for them) and even hypothetical hints were not
//! plumbed through to disk: 85% of `graph_entities` rows landed as
//! `Other("unknown")` with no `kind_source` breadcrumb. The fix is
//! two-part:
//!
//! - **(a)** ask the LLM for kind hints in the extraction prompt
//!   (changes `triple_extractor.rs`);
//! - **(b)** plumb hints from `Triple` → `DraftEntity.kind` →
//!   `Entity.kind` → `graph_entities.kind` column, and write the
//!   `kind_source` breadcrumb into `attributes`.
//!
//! Part (b) is the regression-prone one: any future refactor of
//! `build_delta`, `apply_graph_delta`, or the `Entity` schema can
//! silently break it. This test pins down the on-disk contract.
//!
//! ## Why not the worker pool
//!
//! An earlier draft of this test went through `Memory::with_pipeline_pool`
//! and `store_raw`. That path currently routes most jobs through the
//! `DeferToLlm` / entity-extraction codepath, which has its own
//! orthogonal concerns (entity extractor heuristics, decision
//! thresholds, namespace plumbing). Mixing those concerns with the
//! kind-plumbing concern muddies the test's purpose. The path that
//! actually owns kind plumbing is `build_delta` → `apply_graph_delta`,
//! so we drive that pair directly here.
//!
//! ## What this does
//!
//! 1. Construct a small set of `EntityResolution { decision: CreateNew }`
//!    draft entities, each carrying a specific `kind` and `kind_source`
//!    that mirror what would arrive from a `Triple` with hints set.
//! 2. Run `build_delta` to produce a `GraphDelta` of `Entity` rows.
//! 3. Apply that delta to a real `SqliteGraphStore` against a tempdir
//!    SQLite file.
//! 4. Re-open the same SQLite file with a plain `rusqlite::Connection`
//!    and `SELECT canonical_name, kind, attributes FROM graph_entities`,
//!    asserting:
//!    - the `kind` column round-trips via serde to the original
//!      `EntityKind` variant (including `Other("location")`);
//!    - `attributes.kind_source` matches the source we set on the draft;
//!    - the `Other("unknown") + Default` ratio stays bounded — guards
//!      against regressions where everything collapses back to unknown.
//!
//! ## What this does NOT do
//!
//! - Does not call a real LLM or any `TripleExtractor`. Hint propagation
//!   from `Triple` → `DraftEntity` is exercised by `triple_integration.rs`
//!   already; this test starts at the `DraftEntity` boundary so it can
//!   focus on the persist path.
//! - Does not enforce merge-precedence
//!   (`EnrichmentLLM > DictionaryMatch > TripleHint > Default`). That
//!   policy is locked-but-not-enforced at the time of writing; a
//!   separate test will exercise it once the B-stage lands.

use std::collections::HashMap;

use chrono::Utc;
use engramai::graph::store::{GraphWrite, SqliteGraphStore};
use engramai::graph::{init_graph_tables, EntityKind};
use engramai::resolution::context::{DraftEntity, KindSource, PipelineContext};
use engramai::resolution::decision::Decision;
use engramai::resolution::stage_persist::{build_delta, EntityResolution};
use engramai::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::Connection;
use tempfile::tempdir;
use uuid::Uuid;

/// Build a minimal `MemoryRecord` carrying just the fields `build_delta`
/// reads (`id`, `created_at`). Mirrors the `fixture_memory` helper inside
/// `stage_persist_tests.rs` — duplicated here because that one is
/// `pub(crate)`.
fn fixture_memory(id: &str) -> MemoryRecord {
    MemoryRecord {
        id: id.into(),
        content: "ISS-072 regression fixture".into(),
        memory_type: MemoryType::Episodic,
        layer: MemoryLayer::Working,
        created_at: Utc::now(),
        access_times: Vec::new(),
        working_strength: 1.0,
        core_strength: 0.0,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

fn ctx(memory_id: &str) -> PipelineContext {
    PipelineContext::new(fixture_memory(memory_id), Uuid::new_v4(), None, String::new())
}

/// Build a `DraftEntity` carrying `kind` + `kind_source` exactly as
/// `triples_to_drafts` would produce when given a `Triple` with the
/// matching `*_kind_hint` set.
fn draft(name: &str, kind: EntityKind, source: KindSource) -> DraftEntity {
    let now = Utc::now();
    DraftEntity {
        canonical_name: name.into(),
        kind,
        aliases: vec![name.to_lowercase()],
        subtype_hint: None,
        kind_source: source,
        first_seen: now,
        last_seen: now,
        somatic_fingerprint: None,
        embedding: None,
    }
}

/// Wrap a draft in an `EntityResolution { decision: CreateNew }`.
fn create(idx: usize, draft: DraftEntity) -> EntityResolution {
    EntityResolution::for_test(idx, draft, Decision::CreateNew, None)
}

#[test]
fn kind_and_kind_source_round_trip_through_apply_graph_delta() {
    // ── Arrange ─────────────────────────────────────────────────────────
    // Nine drafts, mirroring the kind-mix that RUN-0007 should have
    // produced if hint plumbing worked. Exercises:
    //   - canonical-variant hints                   (Person, Concept, Event, Topic)
    //   - non-canonical Other(_) hint               ("location") via TripleHint
    //   - no hint at all → Default + unknown        (the "opaque thing" path)
    let drafts: Vec<EntityResolution> = vec![
        create(0, draft("Alice",            EntityKind::Person,             KindSource::TripleHint)),
        create(1, draft("Bob",              EntityKind::Person,             KindSource::TripleHint)),
        create(2, draft("Rust",             EntityKind::Concept,            KindSource::TripleHint)),
        create(3, draft("memory safety",    EntityKind::Concept,            KindSource::TripleHint)),
        create(4, draft("RustConf",         EntityKind::Event,              KindSource::TripleHint)),
        create(5, draft("Portland",         EntityKind::other("location"),  KindSource::TripleHint)),
        create(6, draft("category theory",  EntityKind::Topic,              KindSource::TripleHint)),
        create(7, draft("mathematics",      EntityKind::Topic,              KindSource::TripleHint)),
        create(8, draft("opaque thing",     EntityKind::other("unknown"),   KindSource::Default)),
    ];

    let context = ctx("mem-iss072");
    let delta = build_delta(&context, &drafts, &[]);

    assert_eq!(
        delta.entities.len(),
        drafts.len(),
        "build_delta should produce one Entity per CreateNew draft"
    );

    // ── Act ─────────────────────────────────────────────────────────────
    // Apply against a real on-disk SQLite store, then re-open with a
    // fresh connection so we're reading committed bytes (not Rust-side
    // in-memory state).
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("iss072.db");
    {
        let mut conn = Connection::open(&db_path).expect("open db");
        // Mirror the test-harness setup in `store.rs::fresh_conn`: enable
        // foreign keys, create the v0.2 `memories` table that the graph
        // mention rows FK back to, then init the v0.3 graph schema.
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT NOT NULL);",
        )
        .unwrap();
        init_graph_tables(&conn).expect("init graph tables");
        // Insert the memory row that the GraphDelta references, so the
        // mention-row FK on `graph_memory_entity_mentions.memory_id`
        // resolves cleanly.
        conn.execute(
            "INSERT INTO memories (id, content) VALUES (?1, ?2)",
            rusqlite::params!["mem-iss072", "ISS-072 regression fixture"],
        )
        .expect("insert fixture memory");

        let mut store = SqliteGraphStore::new(&mut conn);
        let report = store.apply_graph_delta(&delta).expect("apply ok");
        assert!(!report.already_applied, "first apply must run");
        assert_eq!(
            report.entities_upserted as usize,
            drafts.len(),
            "every draft should have produced one new entity row"
        );
    }

    // ── Assert ──────────────────────────────────────────────────────────
    let conn = Connection::open(&db_path).expect("re-open db");
    let mut stmt = conn
        .prepare("SELECT canonical_name, kind, attributes FROM graph_entities")
        .expect("prepare");
    let rows: Vec<(String, EntityKind, serde_json::Value)> = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let kind_text: String = row.get(1)?;
            let attrs_text: String = row.get(2)?;
            let kind: EntityKind = serde_json::from_str(&kind_text).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let attrs: serde_json::Value = serde_json::from_str(&attrs_text).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok((name, kind, attrs))
        })
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect");

    assert_eq!(rows.len(), drafts.len(), "row count mismatch after apply");

    let by_name: HashMap<String, (EntityKind, serde_json::Value)> =
        rows.into_iter().map(|(n, k, a)| (n, (k, a))).collect();

    // ── Assertion 1: every draft round-trips with its exact kind
    //                  *and* the matching kind_source breadcrumb.
    let expectations: &[(&str, EntityKind, &str)] = &[
        ("Alice",            EntityKind::Person,             "TripleHint"),
        ("Bob",              EntityKind::Person,             "TripleHint"),
        ("Rust",             EntityKind::Concept,            "TripleHint"),
        ("memory safety",    EntityKind::Concept,            "TripleHint"),
        ("RustConf",         EntityKind::Event,              "TripleHint"),
        ("Portland",         EntityKind::other("location"),  "TripleHint"),
        ("category theory",  EntityKind::Topic,              "TripleHint"),
        ("mathematics",      EntityKind::Topic,              "TripleHint"),
        ("opaque thing",     EntityKind::other("unknown"),   "Default"),
    ];

    for (name, expected_kind, expected_source) in expectations {
        let (actual_kind, actual_attrs) = by_name
            .get(*name)
            .unwrap_or_else(|| panic!("entity {name:?} missing from graph_entities"));
        assert_eq!(
            actual_kind, expected_kind,
            "kind mismatch for {name}: expected {expected_kind:?}, got {actual_kind:?}"
        );
        let actual_source = actual_attrs
            .get("kind_source")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "attributes.kind_source missing for {name}; full attrs: {actual_attrs}"
                )
            });
        assert_eq!(
            actual_source, *expected_source,
            "kind_source mismatch for {name}: expected {expected_source}, got {actual_source}"
        );
    }

    // ── Assertion 2: kind_source values are all from the closed set.
    //                  Catches future regressions that introduce a new
    //                  variant on disk without wiring the deserializer.
    let allowed_sources = ["Default", "TripleHint", "DictionaryMatch", "EnrichmentLlm"];
    for (name, (_, attrs)) in &by_name {
        let src = attrs.get("kind_source").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            allowed_sources.contains(&src),
            "{name}: invalid attributes.kind_source = {src:?} \
             (allowed: {allowed_sources:?})"
        );
    }

    // ── Assertion 3: unknown ratio bound. Guards against the RUN-0007
    //                  regression mode where everything collapses to
    //                  Other("unknown") + Default. With this fixture
    //                  exactly 1/9 (~11%) is unknown; we assert ≤ 25%
    //                  so future fixture additions have headroom but a
    //                  real regression still flips the test red.
    let unknown_count = by_name
        .iter()
        .filter(|(_, (k, _))| matches!(k, EntityKind::Other(s) if s == "unknown"))
        .count();
    let unknown_ratio = unknown_count as f64 / by_name.len() as f64;
    assert!(
        unknown_ratio <= 0.25,
        "Other(\"unknown\") ratio is {:.0}% ({} / {}) — exceeds 25% ceiling. \
         Likely regression in kind plumbing between Triple and graph_entities.",
        unknown_ratio * 100.0,
        unknown_count,
        by_name.len(),
    );
}
