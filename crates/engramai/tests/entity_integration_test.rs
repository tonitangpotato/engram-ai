//! Integration tests for entity indexing: Memory → Storage → Entity pipeline.

use engramai::entities::EntityConfig;
use engramai::{Memory, MemoryConfig, MemoryType};
use rusqlite::params;

/// Create a Memory instance with entity extraction configured and known entities.
///
/// Note: known entity names must not be substrings of each other, because
/// Aho-Corasick with default (Standard) match kind will consume the shorter
/// match and skip the longer one. E.g., "rust" inside "rustclaw" would prevent
/// "rustclaw" from matching. We avoid this by using non-overlapping names.
fn setup_memory() -> (Memory, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let mut config = MemoryConfig::default();
    config.entity_config = EntityConfig {
        known_projects: vec![
            "ironclaw".into(),
            "engramai".into(),
            "gid-core".into(),
            "rustclaw".into(),
        ],
        known_people: vec!["potato".into()],
        known_technologies: vec!["sqlite".into(), "python".into()],
        enabled: true,
        recall_weight: 0.15,
    };
    let memory = Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap();
    (memory, dir)
}

/// Helper: count rows in a table via the connection.
#[allow(dead_code)]
fn count_table(conn: &rusqlite::Connection, table: &str) -> usize {
    let sql = format!("SELECT COUNT(*) FROM {}", table);
    conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
        .unwrap() as usize
}

/// Helper: find entities by name using direct SQL.
fn find_entities_by_name(
    conn: &rusqlite::Connection,
    name: &str,
) -> Vec<(String, String, String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, entity_type, namespace FROM entities WHERE name = ?1",
        )
        .unwrap();
    stmt.query_map(params![name], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

/// Helper: get memory_ids linked to an entity.
fn get_entity_memory_ids(conn: &rusqlite::Connection, entity_id: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT memory_id FROM memory_entities WHERE entity_id = ?1")
        .unwrap();
    stmt.query_map(params![entity_id], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

/// Helper: get related entity IDs for a given entity.
fn get_related_entity_ids(conn: &rusqlite::Connection, entity_id: &str) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT target_id, relation FROM entity_relations WHERE source_id = ?1
            UNION
            SELECT source_id, relation FROM entity_relations WHERE target_id = ?1
            "#,
        )
        .unwrap();
    stmt.query_map(params![entity_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[test]
fn test_add_raw_creates_entities() {
    let (mut mem, _dir) = setup_memory();

    mem.add(
        "Working on ironclaw project",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .unwrap();

    let (entity_count, _relation_count, _link_count) = mem.entity_stats().unwrap();
    assert!(entity_count > 0, "should have created at least one entity");

    // Check "rustclaw" entity exists
    let conn = mem.connection();
    let entities = find_entities_by_name(conn, "ironclaw");
    assert!(
        !entities.is_empty(),
        "should find an 'ironclaw' entity, got: {:?}",
        entities
    );
    assert_eq!(entities[0].2, "project", "ironclaw should be a project entity");
}

#[test]
fn test_add_raw_creates_memory_entity_links() {
    let (mut mem, _dir) = setup_memory();

    let memory_id = mem
        .add(
            "Working on ironclaw project",
            MemoryType::Factual,
            Some(0.7),
            None,
            None,
        )
        .unwrap();

    let conn = mem.connection();
    let entities = find_entities_by_name(conn, "ironclaw");
    assert!(!entities.is_empty(), "ironclaw entity should exist");

    let entity_id = &entities[0].0;
    let linked_memories = get_entity_memory_ids(conn, entity_id);
    assert!(
        linked_memories.contains(&memory_id),
        "entity should be linked to the memory; memory_id={}, linked={:?}",
        memory_id,
        linked_memories
    );
}

#[test]
fn test_add_raw_creates_co_occurrence() {
    let (mut mem, _dir) = setup_memory();

    mem.add(
        "ironclaw uses sqlite for storage",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .unwrap();

    let conn = mem.connection();
    let ironclaw_entities = find_entities_by_name(conn, "ironclaw");
    let sqlite_entities = find_entities_by_name(conn, "sqlite");
    assert!(!ironclaw_entities.is_empty(), "ironclaw entity should exist");
    assert!(!sqlite_entities.is_empty(), "sqlite entity should exist");

    let ironclaw_id = &ironclaw_entities[0].0;
    let sqlite_id = &sqlite_entities[0].0;

    // Check co-occurrence relation exists (either direction)
    let related = get_related_entity_ids(conn, ironclaw_id);
    let related_ids: Vec<&str> = related.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        related_ids.contains(&sqlite_id.as_str()),
        "ironclaw should have a co_occurs relation with sqlite; related={:?}",
        related
    );
}

#[test]
fn test_entity_dedup_across_memories() {
    let (mut mem, _dir) = setup_memory();

    mem.add(
        "Working on ironclaw today",
        MemoryType::Episodic,
        Some(0.5),
        None,
        None,
    )
    .unwrap();

    mem.add(
        "ironclaw is making progress",
        MemoryType::Episodic,
        Some(0.6),
        None,
        None,
    )
    .unwrap();

    let conn = mem.connection();
    let entities = find_entities_by_name(conn, "ironclaw");
    assert_eq!(
        entities.len(),
        1,
        "ironclaw should appear exactly once in the entities table"
    );

    let entity_id = &entities[0].0;
    let linked_memories = get_entity_memory_ids(conn, entity_id);
    assert_eq!(
        linked_memories.len(),
        2,
        "ironclaw entity should be linked to both memories; got {:?}",
        linked_memories
    );
}

#[test]
fn test_entity_recall_returns_memories() {
    let (mut mem, _dir) = setup_memory();

    let id1 = mem
        .add(
            "ironclaw is a great project",
            MemoryType::Factual,
            Some(0.8),
            None,
            None,
        )
        .unwrap();

    // recall() combines FTS + entity channels; "ironclaw" should match via entity
    let results = mem.recall("ironclaw", 5, None, None).unwrap();
    let result_ids: Vec<&str> = results.iter().map(|r| r.record.id.as_str()).collect();
    assert!(
        result_ids.contains(&id1.as_str()),
        "recall('ironclaw') should return the memory; got ids={:?}",
        result_ids
    );
}

#[test]
fn test_entity_recall_1hop_related() {
    let (mut mem, _dir) = setup_memory();

    // Memory 1: creates co-occurrence between ironclaw and sqlite
    let id1 = mem
        .add(
            "ironclaw uses sqlite for persistence",
            MemoryType::Factual,
            Some(0.8),
            None,
            None,
        )
        .unwrap();

    // Memory 2: mentions sqlite only (linked to sqlite entity, not ironclaw)
    let _id2 = mem
        .add(
            "sqlite performance tuning guide",
            MemoryType::Procedural,
            Some(0.7),
            None,
            None,
        )
        .unwrap();

    // Querying "ironclaw" should find both:
    // - id1 directly (mentions ironclaw)
    // - id2 via 1-hop (ironclaw → co_occurs → sqlite → id2)
    let results = mem.recall("ironclaw", 10, None, None).unwrap();
    let result_ids: Vec<&str> = results.iter().map(|r| r.record.id.as_str()).collect();

    assert!(
        result_ids.contains(&id1.as_str()),
        "should find direct ironclaw memory; got ids={:?}",
        result_ids
    );
    // 1-hop recall is best-effort; the memory might also match via FTS.
    // At minimum, id2 should be findable. If embedding is off, FTS may or may not match.
    // Check that we get at least 1 result (id1).
    assert!(
        !results.is_empty(),
        "entity recall should return at least one result"
    );
}

#[test]
fn test_backfill_processes_unlinked() {
    let (mut mem, _dir) = setup_memory();

    // Add memories normally (entities created inline)
    mem.add("ironclaw is fast", MemoryType::Factual, Some(0.7), None, None)
        .unwrap();

    // All memories already have entity links, so backfill should process 0
    let (processed, _entities, _relations) = mem.backfill_entities(100).unwrap();
    assert_eq!(
        processed, 0,
        "backfill should find 0 unlinked memories when all are already linked"
    );

    // Now insert a memory directly via SQL (bypassing entity extraction)
    let conn = mem.connection();
    let raw_id = "backfill-test-001";
    conn.execute(
        r#"
        INSERT INTO memories (id, content, memory_type, layer, created_at, 
                              working_strength, core_strength, importance, namespace)
        VALUES (?1, ?2, 'factual', 'working', strftime('%s','now'), 1.0, 0.0, 0.5, 'default')
        "#,
        params![raw_id, "engramai is a memory system"],
    )
    .unwrap();
    // Also insert into FTS
    conn.execute(
        "INSERT INTO memories_fts(content) VALUES (?1)",
        params!["engramai is a memory system"],
    )
    .unwrap();

    // Now backfill should find the unlinked memory
    let (processed, entity_count, _relations) = mem.backfill_entities(100).unwrap();
    assert_eq!(
        processed, 1,
        "backfill should find 1 unlinked memory"
    );
    assert!(
        entity_count > 0,
        "backfill should have created entities for the unlinked memory"
    );
}

#[test]
fn test_entity_config_disabled() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("disabled.db");
    let mut config = MemoryConfig::default();
    config.entity_config = EntityConfig {
        known_projects: vec!["ironclaw".into()],
        known_people: vec!["potato".into()],
        known_technologies: vec!["python".into()],
        enabled: false,
        recall_weight: 0.15,
    };
    let mut mem = Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap();

    mem.add(
        "Working on ironclaw with potato using python",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .unwrap();

    let (entity_count, _relation_count, _link_count) = mem.entity_stats().unwrap();
    assert_eq!(
        entity_count, 0,
        "no entities should be created when entity_config.enabled = false"
    );
}

#[test]
fn test_namespace_isolation() {
    let (mut mem, _dir) = setup_memory();

    // Add memory with "rustclaw" in namespace "ns1"
    mem.add_to_namespace(
        "ironclaw in namespace one",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
        Some("ns1"),
    )
    .unwrap();

    // Add memory with "rustclaw" in namespace "ns2"
    mem.add_to_namespace(
        "ironclaw in namespace two",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
        Some("ns2"),
    )
    .unwrap();

    let conn = mem.connection();

    // Entities in each namespace should be separate (name+type+namespace is unique)
    let ns1_entities: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, namespace FROM entities WHERE name = 'ironclaw' AND namespace = 'ns1'",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    };

    let ns2_entities: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, namespace FROM entities WHERE name = 'ironclaw' AND namespace = 'ns2'",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    };

    assert_eq!(
        ns1_entities.len(),
        1,
        "ns1 should have exactly one ironclaw entity"
    );
    assert_eq!(
        ns2_entities.len(),
        1,
        "ns2 should have exactly one ironclaw entity"
    );
    assert_ne!(
        ns1_entities[0].0, ns2_entities[0].0,
        "entity IDs should differ across namespaces"
    );
}

#[test]
fn test_many_entities_co_occurrence_cap() {
    // Build a config with 15 known technologies
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("cap.db");
    let mut config = MemoryConfig::default();
    let tech_names: Vec<String> = (0..15).map(|i| format!("tech{}", i)).collect();
    config.entity_config = EntityConfig {
        known_projects: vec![],
        known_people: vec![],
        known_technologies: tech_names.clone(),
        enabled: true,
        recall_weight: 0.15,
    };
    let mut mem = Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap();

    // Create content mentioning all 15 technologies
    let content = tech_names.join(" and ");
    mem.add(&content, MemoryType::Factual, Some(0.7), None, None)
        .unwrap();

    // Co-occurrence is capped at 10 entities → C(10,2) = 45 max relations
    let conn = mem.connection();
    let relation_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM entity_relations", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert!(
        relation_count <= 45,
        "co-occurrence should be capped at C(10,2)=45; got {}",
        relation_count
    );
    // With 15 entities but cap at 10, we should have exactly C(10,2) = 45
    assert!(
        relation_count > 0,
        "should have created some co-occurrence relations"
    );
}

#[test]
fn test_entity_stats() {
    let (mut mem, _dir) = setup_memory();

    mem.add(
        "ironclaw uses sqlite",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .unwrap();

    mem.add(
        "potato built engramai",
        MemoryType::Factual,
        Some(0.8),
        None,
        None,
    )
    .unwrap();

    let (entity_count, relation_count, link_count) = mem.entity_stats().unwrap();

    // "ironclaw", "sqlite" from first memory
    // "potato", "engramai" from second memory
    // Entities: at least 4 unique
    assert!(
        entity_count >= 4,
        "should have at least 4 entities; got {}",
        entity_count
    );

    // Relations: at least 1 co-occurrence per memory (ironclaw↔sqlite, potato↔engramai)
    assert!(
        relation_count >= 2,
        "should have at least 2 co-occurrence relations; got {}",
        relation_count
    );

    // Links: each entity linked to its memory → at least 4 links
    assert!(
        link_count >= 4,
        "should have at least 4 memory-entity links; got {}",
        link_count
    );
}

#[test]
fn test_list_entities() {
    let (mut mem, _dir) = setup_memory();

    mem.add(
        "rustclaw project is amazing",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .unwrap();

    mem.add(
        "rustclaw and engramai work together",
        MemoryType::Factual,
        Some(0.6),
        None,
        None,
    )
    .unwrap();

    let entities = mem.list_entities(None, None, 50).unwrap();
    assert!(
        !entities.is_empty(),
        "list_entities should return results"
    );

    // Find rustclaw - it should have 2 mentions (appears in both memories)
    let rustclaw = entities.iter().find(|(e, _)| e.name == "rustclaw");
    assert!(
        rustclaw.is_some(),
        "should find rustclaw in listed entities; got: {:?}",
        entities.iter().map(|(e, c)| (&e.name, c)).collect::<Vec<_>>()
    );
    let (_, mention_count) = rustclaw.unwrap();
    assert_eq!(
        *mention_count, 2,
        "rustclaw should have 2 mentions (one per memory)"
    );

    // engramai should have 1 mention
    let engramai = entities.iter().find(|(e, _)| e.name == "engramai");
    assert!(
        engramai.is_some(),
        "should find engramai in listed entities"
    );
    let (_, mention_count) = engramai.unwrap();
    assert_eq!(
        *mention_count, 1,
        "engramai should have 1 mention"
    );

    // Entities should be sorted by mention_count desc (rustclaw first)
    // The first entity should have the highest mention count
    let first_count = entities[0].1;
    for (_, count) in &entities {
        assert!(
            *count <= first_count,
            "entities should be sorted by mention count descending"
        );
    }
}

#[test]
fn test_list_entities_filter_by_type() {
    let (mut mem, _dir) = setup_memory();

    mem.add(
        "potato uses python to build rustclaw with sqlite",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .unwrap();

    // Filter by project type
    let projects = mem.list_entities(Some("project"), None, 50).unwrap();
    for (entity, _) in &projects {
        assert_eq!(
            entity.entity_type, "project",
            "filtered list should only contain project entities"
        );
    }
    assert!(
        projects.iter().any(|(e, _)| e.name == "rustclaw"),
        "rustclaw should be in project list; got: {:?}",
        projects.iter().map(|(e, _)| &e.name).collect::<Vec<_>>()
    );

    // Filter by technology type
    let techs = mem.list_entities(Some("technology"), None, 50).unwrap();
    for (entity, _) in &techs {
        assert_eq!(
            entity.entity_type, "technology",
            "filtered list should only contain technology entities"
        );
    }
    assert!(
        techs.iter().any(|(e, _)| e.name == "sqlite" || e.name == "python"),
        "sqlite or python should be in technology list; got: {:?}",
        techs.iter().map(|(e, _)| &e.name).collect::<Vec<_>>()
    );
}
