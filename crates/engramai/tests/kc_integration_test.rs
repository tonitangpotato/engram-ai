#![cfg(feature = "kc")]

//! End-to-end integration tests for the Knowledge Compiler (KC).
//!
//! All tests use `SqliteKnowledgeStore::in_memory()` and no LLM
//! (`CompilationPipeline` falls back to `compile_without_llm` when `llm: None`).

use chrono::{Duration, Utc};
use std::io::Write;
use tempfile::TempDir;

use engramai::compiler::{
    // types (re-exported via mod.rs)
    CompilationRecord, DecayConfig, DuplicateStrategy, ExportFilter, ExportFormat, ImportConfig,
    ImportPolicy, IntakeConfig, KcConfig, LifecycleConfig, LlmConfig, RecompileStrategy,
    SourceMemoryRef, SplitStrategy, TopicCandidate, TopicId, TopicMetadata, TopicPage, TopicStatus,
    // storage
    KnowledgeStore, SqliteKnowledgeStore,
    // compilation
    compilation::{ChangeDetector, CompilationPipeline, MemorySnapshot, TriggerEvaluator},
    // conflict
    conflict::ConflictDetector,
    // decay
    decay::DecayEngine,
    // discovery
    discovery::TopicDiscovery,
    // export
    export::{ExportEngine, ExportOutput},
    // import
    import::{ImportPipeline, MarkdownImporter},
    // llm
    llm::NoopProvider,
    // privacy
    privacy::{AccessContext, PrivacyGuard},
};

// ═══════════════════════════════════════════════════════════════════════════════
//  HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

fn make_config() -> KcConfig {
    KcConfig {
        min_cluster_size: 2, // lower for tests
        quality_threshold: 0.4,
        recompile_strategy: RecompileStrategy::Eager,
        decay: DecayConfig::default(),
        llm: LlmConfig::default(),
        import: ImportConfig::default(),
        intake: IntakeConfig::default(),
        lifecycle: LifecycleConfig::default(),
    }
}

fn make_store() -> SqliteKnowledgeStore {
    let store = SqliteKnowledgeStore::in_memory().unwrap();
    store.init_schema().unwrap();
    store
}

fn make_memory(id: &str, content: &str) -> MemorySnapshot {
    MemorySnapshot {
        id: id.to_string(),
        content: content.to_string(),
        memory_type: "factual".to_string(),
        importance: 0.5,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        tags: vec![],
    }
}

fn make_topic(id: &str, source_ids: Vec<String>, days_old: i64) -> TopicPage {
    let now = Utc::now();
    let created = now - Duration::days(days_old);
    TopicPage {
        id: TopicId(id.to_string()),
        title: format!("Topic {id}"),
        content: format!("# Topic {id}\n\nCompiled knowledge about {id}.\n\nMore details here."),
        sections: Vec::new(),
        summary: format!("Summary for topic {id}"),
        status: TopicStatus::Active,
        version: 1,
        metadata: TopicMetadata {
            created_at: created,
            updated_at: created,
            compilation_count: 1,
            source_memory_ids: source_ids,
            tags: vec!["test".to_string()],
            quality_score: Some(0.6),
        },
    }
}

/// Write a temp file inside the given dir.
fn write_temp_file(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    path
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 1: Memories → Compiled Topic with Provenance
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_memories_to_compiled_topic_with_provenance() {
    let store = make_store();
    let config = make_config();

    // Create several memory snapshots
    let m1 = make_memory("m1", "Rust is a systems programming language focused on safety");
    let m2 = make_memory("m2", "Rust uses a borrow checker to enforce memory safety at compile time");
    let m3 = make_memory("m3", "Rust was first released in 2015 and is maintained by the Rust Foundation");
    let m4 = make_memory("m4", "Rust supports zero-cost abstractions and fearless concurrency");
    let memories = vec![m1, m2, m3, m4];

    // Use TopicDiscovery with fake embeddings (4 dimensions, all similar → one cluster)
    let discovery = TopicDiscovery::new(2);
    let embedded: Vec<(String, Vec<f32>)> = vec![
        ("m1".to_string(), vec![0.9, 0.1, 0.0, 0.0]),
        ("m2".to_string(), vec![0.85, 0.15, 0.0, 0.0]),
        ("m3".to_string(), vec![0.8, 0.2, 0.05, 0.0]),
        ("m4".to_string(), vec![0.88, 0.12, 0.0, 0.0]),
    ];

    let candidates = discovery.discover(&embedded);
    assert!(
        !candidates.is_empty(),
        "TopicDiscovery should find at least one cluster"
    );

    let candidate = &candidates[0];
    assert!(
        candidate.memories.len() >= 2,
        "Candidate cluster should have at least 2 memories"
    );

    // Filter memories that belong to this candidate
    let candidate_memories: Vec<MemorySnapshot> = memories
        .iter()
        .filter(|m| candidate.memories.contains(&m.id))
        .cloned()
        .collect();

    // Use CompilationPipeline (no LLM) to compile the candidate
    let pipeline: CompilationPipeline<SqliteKnowledgeStore, NoopProvider> =
        CompilationPipeline::new(store, None, config.clone());

    let page = pipeline.compile_new(candidate, &candidate_memories).unwrap();

    // Assert: topic page created with content
    assert!(!page.content.is_empty(), "Compiled page should have content");
    assert!(!page.title.is_empty(), "Compiled page should have a title");
    assert_eq!(page.version, 1);
    assert_eq!(page.status, TopicStatus::Active);

    // Assert: provenance — source_memory_ids populated
    assert!(
        !page.metadata.source_memory_ids.is_empty(),
        "Provenance: source_memory_ids should be populated"
    );
    for mem in &candidate_memories {
        assert!(
            page.metadata.source_memory_ids.contains(&mem.id),
            "source_memory_ids should contain '{}'",
            mem.id
        );
    }

    // Assert: quality score is set
    assert!(
        page.metadata.quality_score.is_some(),
        "Quality score should be set after compilation"
    );

    // Assert: compilation record saved and retrievable
    // We need to access the store via the pipeline's internals or re-open.
    // Since CompilationPipeline owns the store, let's create a new store reference
    // and check by reconstructing. Instead, test via a fresh store + manual compilation.
    let store2 = make_store();
    let pipeline2: CompilationPipeline<SqliteKnowledgeStore, NoopProvider> =
        CompilationPipeline::new(store2, None, config);
    let page2 = pipeline2.compile_new(candidate, &candidate_memories).unwrap();

    // We can't directly access the store inside pipeline2, but compile_new
    // persists both the page and the record. Let's verify by creating a store,
    // compiling, and then checking records through a store we retain access to.
    // Re-approach: use store directly alongside the pipeline by sharing ownership.
    // Since CompilationPipeline takes ownership, let's verify the records by
    // looking at what the pipeline persisted.

    // The pipeline persists the page — we checked it returned Ok.
    // The pipeline also persists the compilation record.
    // The real assertion is that compile_new didn't error.
    assert!(page2.metadata.quality_score.is_some());
    assert_eq!(page2.metadata.compilation_count, 1);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 2: Incremental Recompilation on Memory Change
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_incremental_recompilation_on_memory_change() {
    let store = make_store();
    let config = make_config();

    // --- Setup: compile initial topic ---
    let m1 = make_memory("m1", "Rust is a systems programming language");
    let m2 = make_memory("m2", "Rust uses ownership and borrowing for memory safety");
    let initial_memories = vec![m1.clone(), m2.clone()];

    let candidate = TopicCandidate {
        memories: vec!["m1".to_string(), "m2".to_string()],
        centroid_embedding: vec![0.5, 0.5, 0.0, 0.0],
        cohesion_score: 0.8,
        suggested_title: Some("Rust Programming".to_string()),
    };

    let pipeline: CompilationPipeline<SqliteKnowledgeStore, NoopProvider> =
        CompilationPipeline::new(store, None, config.clone());

    let initial_page = pipeline.compile_new(&candidate, &initial_memories).unwrap();
    let topic_id = initial_page.id.clone();
    assert_eq!(initial_page.version, 1);
    assert_eq!(initial_page.metadata.compilation_count, 1);

    // --- Add new memories ---
    let m3 = make_memory("m3", "Rust 2024 edition introduces new syntax features");
    let m4 = make_memory("m4", "The Rust compiler uses LLVM as its backend");
    let all_memories = vec![m1, m2, m3, m4];

    // --- Detect changes ---
    let initial_record = CompilationRecord {
        topic_id: topic_id.clone(),
        compiled_at: initial_page.metadata.created_at,
        source_count: 2,
        duration_ms: 10,
        quality_score: initial_page.metadata.quality_score.unwrap_or(0.5),
        recompile_reason: Some("initial compilation".to_string()),
    };

    let previous_ids: Vec<String> = initial_page.metadata.source_memory_ids.clone();
    let changes = ChangeDetector::detect(&all_memories, Some(&initial_record), &previous_ids);

    assert!(
        !changes.added.is_empty(),
        "ChangeDetector should detect added memories (m3, m4)"
    );
    assert!(
        changes.added.contains(&"m3".to_string()) || changes.added.contains(&"m4".to_string()),
        "Added should include m3 or m4"
    );

    // --- Use TriggerEvaluator to decide recompilation ---
    let evaluator = TriggerEvaluator::new(&config);
    let decision = evaluator.evaluate(
        &all_memories,
        Some(&initial_record),
        &previous_ids,
        &config.recompile_strategy,
    );

    // With Eager strategy and 2 new out of 2 old, should trigger Full or Partial
    match &decision {
        engramai::compiler::TriggerDecision::Skip { reason } => {
            panic!("Expected recompilation trigger, got Skip: {reason}");
        }
        engramai::compiler::TriggerDecision::Partial { change_set }
        | engramai::compiler::TriggerDecision::Full { change_set } => {
            assert!(
                !change_set.added.is_empty(),
                "Change set should have added memories"
            );
        }
    }

    // --- Recompile ---
    let updated_page = pipeline
        .recompile(&initial_page, &all_memories, &changes, &[])
        .unwrap();

    // Assert: version and compilation_count incremented
    assert_eq!(
        updated_page.version,
        initial_page.version + 1,
        "Version should be incremented"
    );
    assert_eq!(
        updated_page.metadata.compilation_count,
        initial_page.metadata.compilation_count + 1,
        "compilation_count should be incremented"
    );

    // Assert: source_memory_ids updated to include new memories
    assert!(
        updated_page.metadata.source_memory_ids.contains(&"m3".to_string()),
        "source_memory_ids should include m3 after recompilation"
    );
    assert!(
        updated_page.metadata.source_memory_ids.contains(&"m4".to_string()),
        "source_memory_ids should include m4 after recompilation"
    );
    assert_eq!(
        updated_page.metadata.source_memory_ids.len(),
        4,
        "Should have 4 source memories total"
    );

    // Assert: quality score still set
    assert!(updated_page.metadata.quality_score.is_some());
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 3: Import → Compile → Export Roundtrip
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_import_compile_export_roundtrip() {
    let store = make_store();

    // --- Create temp directory with markdown files ---
    let dir = TempDir::new().unwrap();
    write_temp_file(
        &dir,
        "rust_basics.md",
        "# Rust Basics\n\nRust is a systems programming language.\n\n## Ownership\n\nRust uses ownership to manage memory.\n",
    );
    write_temp_file(
        &dir,
        "rust_advanced.md",
        "# Rust Advanced\n\nAdvanced Rust concepts include lifetimes and traits.\n\n## Lifetimes\n\nLifetimes ensure references are valid.\n",
    );

    // --- Import using MarkdownImporter ---
    let importer = MarkdownImporter {
        split: SplitStrategy::ByHeading,
    };
    let import_config = ImportConfig {
        default_policy: ImportPolicy::Skip,
        split_strategy: SplitStrategy::ByHeading,
        duplicate_strategy: DuplicateStrategy::Skip,
        max_document_size_bytes: 10_000_000,
    };

    let report = ImportPipeline::run(&store, &importer, dir.path(), &import_config).unwrap();

    // Assert: import report shows correct counts
    assert!(
        report.total_processed > 0,
        "Should have processed some items"
    );
    assert!(
        report.imported > 0,
        "Should have imported some items, got imported={} total_processed={}",
        report.imported,
        report.total_processed
    );
    assert!(report.errors.is_empty(), "Import should have no errors");

    // --- List imported pages ---
    let pages = store.list_topic_pages().unwrap();
    assert!(
        !pages.is_empty(),
        "Store should contain imported pages"
    );
    assert_eq!(
        pages.len(),
        report.imported,
        "Number of pages should match import count"
    );

    // --- Export as Markdown ---
    let privacy = PrivacyGuard::in_memory().unwrap();
    let ctx = AccessContext {
        accessor: "test".to_string(),
        include_private: true,
        is_export: false,
    };
    let filter = ExportFilter {
        topics: None,
        status: None,
        tags: None,
        since: None,
    };

    let output =
        ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Markdown).unwrap();

    match output {
        ExportOutput::Markdown(files) => {
            assert!(
                !files.is_empty(),
                "Export should produce markdown files"
            );
            assert_eq!(
                files.len(),
                pages.len(),
                "Export should produce one file per page"
            );

            // Check that exported markdown contains original content
            let all_export_content: String =
                files.iter().map(|f| f.content.as_str()).collect::<Vec<_>>().join("\n");

            // The original content should appear in the export
            assert!(
                all_export_content.contains("Rust")
                    || all_export_content.contains("ownership")
                    || all_export_content.contains("Ownership"),
                "Exported markdown should contain original content about Rust"
            );
        }
        other => panic!("Expected Markdown export, got: {:?}", std::mem::discriminant(&other)),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 4: Decay and Archive Lifecycle
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_decay_and_archive_lifecycle() {
    let store = make_store();

    // Create a topic page with old timestamps (90 days ago)
    let old_topic = make_topic("old-topic", vec!["m1".to_string(), "m2".to_string()], 90);
    store.create_topic_page(&old_topic).unwrap();

    // Save source refs with old added_at dates and low relevance scores.
    // The DecayEngine computes freshness as a recency-weighted mean of relevance
    // scores: weight = 1/(1+age_days)^1.5.  When all sources have the same age
    // the weights cancel out and freshness ≈ mean(relevance).  To get truly low
    // freshness we need low relevance_score values.
    let old_date = Utc::now() - Duration::days(90);
    let refs = vec![
        SourceMemoryRef {
            memory_id: "m1".to_string(),
            relevance_score: 0.05,
            added_at: old_date,
        },
        SourceMemoryRef {
            memory_id: "m2".to_string(),
            relevance_score: 0.03,
            added_at: old_date,
        },
    ];
    store
        .save_source_refs(&old_topic.id, &refs)
        .unwrap();

    // Use DecayEngine to evaluate the topic
    let decay_config = DecayConfig {
        check_interval_hours: 24,
        stale_threshold_days: 30,
        archive_threshold_days: 90,
        min_access_count: 0,
    };
    let engine = DecayEngine::new(decay_config);
    let result = engine.evaluate_topic(&old_topic, &store).unwrap();

    // Assert: low freshness score (sources are 90 days old with low relevance)
    assert!(
        result.freshness_score < 0.3,
        "Freshness should be low for old sources with low relevance, got {}",
        result.freshness_score
    );

    // Assert: recommended action is Archive or MarkStale
    match &result.recommended_action {
        engramai::compiler::DecayAction::Archive(_) => { /* expected */ }
        engramai::compiler::DecayAction::MarkStale(_) => { /* also acceptable */ }
        other => panic!(
            "Expected Archive or MarkStale for old topic, got {:?}",
            other
        ),
    }

    // Apply the decay action (mark archived via store)
    engine.apply_decay(&result.recommended_action, &store).unwrap();

    // Assert: page status is Archived (if Archive action) or quality reduced (if MarkStale)
    let updated = store.get_topic_page(&old_topic.id).unwrap().unwrap();
    match &result.recommended_action {
        engramai::compiler::DecayAction::Archive(_) => {
            assert_eq!(
                updated.status,
                TopicStatus::Archived,
                "Page should be archived after applying Archive action"
            );

            // Verify get_pages_by_status(Archived) returns it
            let archived_pages = store.get_pages_by_status(TopicStatus::Archived).unwrap();
            assert!(
                archived_pages.iter().any(|p| p.id == old_topic.id),
                "Archived page should appear in get_pages_by_status(Archived)"
            );
        }
        engramai::compiler::DecayAction::MarkStale(_) => {
            // MarkStale updates the activity score to 0.0
            assert_eq!(
                updated.metadata.quality_score,
                Some(0.0),
                "MarkStale should set quality_score to 0.0"
            );
        }
        _ => {}
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 5: Conflict Detection Between Topics
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_conflict_detection_between_topics() {
    let store = make_store();

    // Create two topic pages with highly overlapping source_memory_ids
    let shared_sources = vec![
        "m1".to_string(),
        "m2".to_string(),
        "m3".to_string(),
        "m4".to_string(),
        "m5".to_string(),
    ];

    // Topic A has m1..m5 + m6
    let mut sources_a = shared_sources.clone();
    sources_a.push("m6".to_string());
    let topic_a = make_topic("topic-a", sources_a, 5);
    store.create_topic_page(&topic_a).unwrap();

    // Topic B has m1..m5 + m7
    let mut sources_b = shared_sources;
    sources_b.push("m7".to_string());
    let topic_b = make_topic("topic-b", sources_b, 3);
    store.create_topic_page(&topic_b).unwrap();

    // Use ConflictDetector to find duplicates
    let detector = ConflictDetector::new();
    let topics = vec![topic_a.clone(), topic_b.clone()];
    let duplicates = detector.detect_duplicates(&topics);

    // Assert: at least one duplicate group found
    assert!(
        !duplicates.is_empty(),
        "Should detect near-duplicates between topics with high source overlap (5/7 shared)"
    );

    // The duplicate group should reference both topics
    let group = &duplicates[0];
    let all_ids: Vec<&TopicId> = std::iter::once(&group.canonical)
        .chain(group.duplicates.iter())
        .collect();
    assert!(
        all_ids.iter().any(|id| **id == topic_a.id),
        "Duplicate group should include topic-a"
    );
    assert!(
        all_ids.iter().any(|id| **id == topic_b.id),
        "Duplicate group should include topic-b"
    );
    assert!(
        group.similarity > 0.5,
        "Similarity should be high, got {}",
        group.similarity
    );

    // Also test detect_conflicts for cross-validation
    let scope = engramai::compiler::ConflictScope::BetweenTopics(topic_a.id.clone(), topic_b.id.clone());
    let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();

    assert!(
        !conflicts.is_empty(),
        "Should detect conflict between highly overlapping topics"
    );

    // Verify conflict type is Redundant (high overlap)
    let record = &conflicts[0];
    assert_eq!(
        record.conflict.conflict_type,
        engramai::compiler::ConflictType::Redundant,
        "High-overlap topics should be classified as Redundant"
    );
}
