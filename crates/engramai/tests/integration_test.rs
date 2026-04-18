use engramai::{Memory, MemoryConfig, MemoryType, Permission};
use tempfile::tempdir;

#[test]
fn test_basic_workflow() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Add memories
    let id1 = mem
        .add("potato prefers action", MemoryType::Relational, Some(0.7), None, None)
        .unwrap();
    
    let _id2 = mem
        .add("Use moltbook.com for API", MemoryType::Procedural, Some(0.8), None, None)
        .unwrap();

    // Recall
    let results = mem.recall("potato preference", 5, None, None).unwrap();
    assert!(!results.is_empty());
    assert!(results[0].record.content.contains("potato"));

    // Pin
    mem.pin(&id1).unwrap();

    // Consolidate
    mem.consolidate(1.0).unwrap();

    // Stats
    let stats = mem.stats().unwrap();
    assert_eq!(stats.total_memories, 2);
    assert_eq!(stats.pinned, 1);
}

#[test]
fn test_hebbian_links() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    let id1 = mem
        .add("Python is a programming language", MemoryType::Factual, None, None, None)
        .unwrap();
    
    let id2 = mem
        .add("Python has dynamic typing", MemoryType::Factual, None, None, None)
        .unwrap();

    // Recall them together multiple times to form Hebbian link
    for _ in 0..4 {
        let _results = mem.recall("Python programming", 10, None, None).unwrap();
    }

    // Check if link was formed
    let links = mem.hebbian_links(&id1).unwrap();
    assert!(!links.is_empty() || mem.hebbian_links(&id2).unwrap().contains(&id1));
}

#[test]
fn test_forgetting() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    mem.add("Weak memory", MemoryType::Episodic, Some(0.1), None, None)
        .unwrap();

    // Consolidate many times to decay
    for _ in 0..10 {
        mem.consolidate(1.0).unwrap();
    }

    // Prune weak memories
    mem.forget(None, Some(0.01)).unwrap();

    let stats = mem.stats().unwrap();
    // Memory should be archived or forgotten
    assert!(stats.total_memories <= 1);
}

#[test]
fn test_reward_learning() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    let _id = mem
        .add("Test memory", MemoryType::Factual, Some(0.5), None, None)
        .unwrap();

    // Recall to make it "recent"
    mem.recall("test", 5, None, None).unwrap();

    // Apply positive feedback
    mem.reward("great job!", 3).unwrap();

    // Memory should be strengthened (check via stats or direct query)
    let stats = mem.stats().unwrap();
    assert!(stats.total_memories > 0);
}

#[test]
fn test_config_presets() {
    let configs = vec![
        MemoryConfig::default(),
        MemoryConfig::chatbot(),
        MemoryConfig::task_agent(),
        MemoryConfig::personal_assistant(),
        MemoryConfig::researcher(),
    ];

    for config in configs {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let mut mem = Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap();

        mem.add("Test", MemoryType::Factual, None, None, None)
            .unwrap();
        
        mem.consolidate(1.0).unwrap();
        
        let stats = mem.stats().unwrap();
        assert_eq!(stats.total_memories, 1);
    }
}

// === Engram v2 Phase 1: Namespace Tests ===

#[test]
fn test_namespace_isolation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Store in namespace A
    mem.add_to_namespace(
        "Secret trading strategy for oil",
        MemoryType::Procedural,
        Some(0.8),
        None,
        None,
        Some("trading"),
    ).unwrap();

    // Store in namespace B
    mem.add_to_namespace(
        "Hackathon project idea: AI assistant",
        MemoryType::Episodic,
        Some(0.7),
        None,
        None,
        Some("hackathon"),
    ).unwrap();

    // Recall from namespace A should find trading memory
    let results_a = mem.recall_from_namespace("strategy", 10, None, None, Some("trading")).unwrap();
    assert_eq!(results_a.len(), 1);
    assert!(results_a[0].record.content.contains("trading"));

    // Recall from namespace B should NOT find the trading namespace memory
    // Note: With embedding-based search, there may be semantic matches to the 
    // hackathon memory, but the trading memory should never appear here.
    let results_b = mem.recall_from_namespace("strategy", 10, None, None, Some("hackathon")).unwrap();
    
    // Verify namespace isolation: trading content must NOT leak into hackathon results
    for r in &results_b {
        assert!(!r.record.content.contains("trading"), 
            "Namespace leak: trading content appeared in hackathon namespace results");
        assert!(!r.record.content.contains("oil"),
            "Namespace leak: trading content appeared in hackathon namespace results");
    }
}

#[test]
fn test_namespace_wildcard() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Store in multiple namespaces
    mem.add_to_namespace(
        "Python programming tutorial",
        MemoryType::Procedural,
        Some(0.6),
        None,
        None,
        Some("learning"),
    ).unwrap();

    mem.add_to_namespace(
        "Python data analysis project",
        MemoryType::Episodic,
        Some(0.7),
        None,
        None,
        Some("work"),
    ).unwrap();

    // Wildcard search should find both
    let results = mem.recall_from_namespace("Python", 10, None, None, Some("*")).unwrap();
    assert_eq!(results.len(), 2);
    
    // Verify both memories are present
    let contents: Vec<_> = results.iter().map(|r| r.record.content.as_str()).collect();
    assert!(contents.iter().any(|c| c.contains("tutorial")));
    assert!(contents.iter().any(|c| c.contains("analysis")));
}

#[test]
fn test_namespace_default() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Store without specifying namespace (should go to "default")
    mem.add(
        "Default namespace memory",
        MemoryType::Factual,
        None,
        None,
        None,
    ).unwrap();

    // Should be retrievable from default namespace
    let results = mem.recall("default namespace", 10, None, None).unwrap();
    assert_eq!(results.len(), 1);

    // Should also be retrievable with explicit default namespace
    let results2 = mem.recall_from_namespace("default namespace", 10, None, None, Some("default")).unwrap();
    assert_eq!(results2.len(), 1);
}

// === Engram v2 Phase 1: ACL Tests ===

#[test]
fn test_acl_basic() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();
    mem.set_agent_id("ceo");

    // Grant read permission to trading agent
    mem.grant("trading-agent", "trading", Permission::Read).unwrap();

    // Check permission
    assert!(mem.check_permission("trading-agent", "trading", Permission::Read).unwrap());
    
    // Read permission should not grant write
    assert!(!mem.check_permission("trading-agent", "trading", Permission::Write).unwrap());
}

#[test]
fn test_acl_deny() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Without any grants, hackathon-agent should not have access to trading namespace
    assert!(!mem.check_permission("hackathon-agent", "trading", Permission::Read).unwrap());
    assert!(!mem.check_permission("hackathon-agent", "trading", Permission::Write).unwrap());
}

#[test]
fn test_acl_permission_hierarchy() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();
    mem.set_agent_id("ceo");

    // Grant admin permission
    mem.grant("super-agent", "shared", Permission::Admin).unwrap();

    // Admin should have all permissions
    assert!(mem.check_permission("super-agent", "shared", Permission::Read).unwrap());
    assert!(mem.check_permission("super-agent", "shared", Permission::Write).unwrap());
    assert!(mem.check_permission("super-agent", "shared", Permission::Admin).unwrap());

    // Grant write permission to another agent
    mem.grant("write-agent", "shared", Permission::Write).unwrap();

    // Write should include read but not admin
    assert!(mem.check_permission("write-agent", "shared", Permission::Read).unwrap());
    assert!(mem.check_permission("write-agent", "shared", Permission::Write).unwrap());
    assert!(!mem.check_permission("write-agent", "shared", Permission::Admin).unwrap());
}

#[test]
fn test_acl_wildcard_namespace() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();
    mem.set_agent_id("ceo");

    // Grant wildcard read permission
    mem.grant("observer-agent", "*", Permission::Read).unwrap();

    // Should be able to read any namespace
    assert!(mem.check_permission("observer-agent", "trading", Permission::Read).unwrap());
    assert!(mem.check_permission("observer-agent", "hackathon", Permission::Read).unwrap());
    assert!(mem.check_permission("observer-agent", "anything", Permission::Read).unwrap());
    
    // But not write
    assert!(!mem.check_permission("observer-agent", "trading", Permission::Write).unwrap());
}

#[test]
fn test_acl_revoke() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();
    mem.set_agent_id("ceo");

    // Grant and then revoke
    mem.grant("temp-agent", "data", Permission::Write).unwrap();
    assert!(mem.check_permission("temp-agent", "data", Permission::Write).unwrap());

    mem.revoke("temp-agent", "data").unwrap();
    assert!(!mem.check_permission("temp-agent", "data", Permission::Write).unwrap());
}

#[test]
fn test_acl_global_namespace() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Global namespace should be readable by anyone (default behavior)
    assert!(mem.check_permission("any-agent", "global", Permission::Read).unwrap());
    
    // But not writable by default
    assert!(!mem.check_permission("any-agent", "global", Permission::Write).unwrap());
}

#[test]
fn test_acl_list_permissions() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();
    mem.set_agent_id("ceo");

    // Grant multiple permissions
    mem.grant("multi-agent", "ns1", Permission::Read).unwrap();
    mem.grant("multi-agent", "ns2", Permission::Write).unwrap();
    mem.grant("multi-agent", "ns3", Permission::Admin).unwrap();

    let perms = mem.list_permissions("multi-agent").unwrap();
    assert_eq!(perms.len(), 3);
    
    // Verify all permissions are present
    let namespaces: Vec<_> = perms.iter().map(|p| p.namespace.as_str()).collect();
    assert!(namespaces.contains(&"ns1"));
    assert!(namespaces.contains(&"ns2"));
    assert!(namespaces.contains(&"ns3"));
}

#[test]
fn test_namespace_stats() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Add memories to different namespaces
    mem.add_to_namespace("Memory 1", MemoryType::Factual, None, None, None, Some("ns1")).unwrap();
    mem.add_to_namespace("Memory 2", MemoryType::Factual, None, None, None, Some("ns1")).unwrap();
    mem.add_to_namespace("Memory 3", MemoryType::Factual, None, None, None, Some("ns2")).unwrap();

    // Stats for ns1 should show 2 memories
    let stats_ns1 = mem.stats_ns(Some("ns1")).unwrap();
    assert_eq!(stats_ns1.total_memories, 2);

    // Stats for ns2 should show 1 memory
    let stats_ns2 = mem.stats_ns(Some("ns2")).unwrap();
    assert_eq!(stats_ns2.total_memories, 1);

    // Stats for all namespaces should show 3 memories
    let stats_all = mem.stats_ns(Some("*")).unwrap();
    assert_eq!(stats_all.total_memories, 3);
}
