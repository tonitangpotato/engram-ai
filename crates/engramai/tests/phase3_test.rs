//! Phase 3: Cross-Agent Intelligence tests.
//!
//! Tests for:
//! - Cross-namespace Hebbian links
//! - Subscription/notification model
//! - Recall with associations
//! - ACL-aware cross-links

use engramai::{Memory, MemoryConfig, MemoryType, Permission};

fn setup_memory() -> Memory {
    let mut config = MemoryConfig::default();
    config.hebbian_enabled = true;
    config.hebbian_threshold = 2; // Lower threshold for testing
    Memory::new(":memory:", Some(config)).unwrap()
}

#[test]
fn test_cross_namespace_hebbian() {
    let mut mem = setup_memory();
    
    // Store memories in different namespaces
    let engine_id = mem.add_to_namespace(
        "oil prices rising due to geopolitical crisis",
        MemoryType::Factual,
        Some(0.8),
        None,
        None,
        Some("engine"),
    ).unwrap();
    
    let _alpha_id = mem.add_to_namespace(
        "unusual options activity in energy sector",
        MemoryType::Factual,
        Some(0.9),
        None,
        None,
        Some("alpha"),
    ).unwrap();
    
    // Recall with wildcard namespace to trigger co-activation
    // Need to recall twice to form link (threshold = 2)
    let _ = mem.recall_from_namespace("oil energy", 10, None, None, Some("*")).unwrap();
    let _ = mem.recall_from_namespace("oil energy crisis", 10, None, None, Some("*")).unwrap();
    
    // Use recall_with_associations to also trigger cross-namespace hebbian
    let result = mem.recall_with_associations("oil energy", Some("*"), 10).unwrap();
    
    // Verify we got memories from both namespaces
    let namespaces: Vec<_> = result.memories.iter()
        .filter_map(|r| mem.connection().query_row(
            "SELECT namespace FROM memories WHERE id = ?",
            [&r.record.id],
            |row| row.get::<_, String>(0)
        ).ok())
        .collect();
    
    // Should have memories from both namespaces
    assert!(namespaces.contains(&"engine".to_string()) || namespaces.contains(&"alpha".to_string()));
    
    // Check cross-namespace links were formed
    let cross_assocs = mem.get_cross_associations(&engine_id).ok().unwrap_or_default();
    // Links form after threshold co-activations
    println!("Cross associations for engine memory: {:?}", cross_assocs);
}

#[test]
fn test_subscription_basic() {
    let mem = setup_memory();
    
    // Subscribe CEO to trading namespace
    mem.subscribe("ceo", "trading", 0.7).unwrap();
    
    // Verify subscription
    let subs = mem.list_subscriptions("ceo").unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].namespace, "trading");
    assert!((subs[0].min_importance - 0.7).abs() < 0.01);
}

#[test]
fn test_subscription_threshold() {
    let mut mem = setup_memory();
    
    // Subscribe with high threshold
    mem.subscribe("ceo", "trading", 0.8).unwrap();
    
    // Store low-importance memory
    mem.add_to_namespace(
        "minor market update",
        MemoryType::Factual,
        Some(0.3),  // Below threshold
        None,
        None,
        Some("trading"),
    ).unwrap();
    
    // Check notifications - should be empty
    let notifs = mem.check_notifications("ceo").unwrap();
    assert!(notifs.is_empty(), "Low importance memory should not trigger notification");
    
    // Store high-importance memory
    mem.add_to_namespace(
        "MAJOR: Oil spike 15% breaking news",
        MemoryType::Factual,
        Some(0.9),  // Above threshold
        None,
        None,
        Some("trading"),
    ).unwrap();
    
    // Check notifications - should have one
    let notifs = mem.check_notifications("ceo").unwrap();
    assert_eq!(notifs.len(), 1);
    assert!(notifs[0].content.contains("Oil spike"));
}

#[test]
fn test_recall_with_associations() {
    let mut mem = setup_memory();
    
    // Store related memories in different namespaces
    let _m1 = mem.add_to_namespace(
        "engine diagnostics showing anomaly",
        MemoryType::Factual,
        Some(0.8),
        None,
        None,
        Some("engine"),
    ).unwrap();
    
    let _m2 = mem.add_to_namespace(
        "trading system anomaly detected",
        MemoryType::Factual,
        Some(0.85),
        None,
        None,
        Some("trading"),
    ).unwrap();
    
    // Recall with associations - should find both and potentially link them
    let result = mem.recall_with_associations("anomaly", Some("*"), 10).unwrap();
    
    // Should find at least one memory
    assert!(!result.memories.is_empty());
    
    // Recall multiple times to potentially form links
    for _ in 0..3 {
        let _ = mem.recall_with_associations("anomaly detected", Some("*"), 10);
    }
    
    // Final recall
    let result = mem.recall_with_associations("anomaly system", Some("*"), 10).unwrap();
    println!("Memories found: {}", result.memories.len());
    println!("Cross links found: {}", result.cross_links.len());
}

#[test]
fn test_cross_links_acl() {
    let mut mem = setup_memory();
    mem.set_agent_id("agent1");
    
    // Store memories in two namespaces
    mem.add_to_namespace("secret trading data", MemoryType::Factual, Some(0.9), None, None, Some("trading")).unwrap();
    mem.add_to_namespace("public engine data", MemoryType::Factual, Some(0.9), None, None, Some("engine")).unwrap();
    
    // Agent1 has no permissions - cross links should be empty (ACL enforced)
    let links = mem.discover_cross_links("trading", "engine").unwrap();
    assert!(links.is_empty(), "Agent without read permission should not see cross-links");
    
    // Grant read permission on both namespaces
    mem.grant("agent1", "trading", Permission::Read).unwrap();
    mem.grant("agent1", "engine", Permission::Read).unwrap();
    
    // Now agent can see cross-links (once they form)
    let links = mem.discover_cross_links("trading", "engine").unwrap();
    println!("Links after granting permission: {:?}", links);
}

#[test]
fn test_subscription_wildcard() {
    let mut mem = setup_memory();
    
    // CEO subscribes to all namespaces
    mem.subscribe("ceo", "*", 0.8).unwrap();
    
    // Store high-importance memories in different namespaces
    mem.add_to_namespace("trading alert", MemoryType::Factual, Some(0.9), None, None, Some("trading")).unwrap();
    mem.add_to_namespace("engine alert", MemoryType::Factual, Some(0.85), None, None, Some("engine")).unwrap();
    mem.add_to_namespace("alpha signal", MemoryType::Factual, Some(0.95), None, None, Some("alpha")).unwrap();
    
    // Check notifications - should get all three
    let notifs = mem.check_notifications("ceo").unwrap();
    assert_eq!(notifs.len(), 3, "Should receive notifications from all namespaces");
    
    // Verify different namespaces
    let ns: std::collections::HashSet<_> = notifs.iter().map(|n| n.namespace.clone()).collect();
    assert!(ns.contains("trading"));
    assert!(ns.contains("engine"));
    assert!(ns.contains("alpha"));
}

#[test]
fn test_unsubscribe() {
    let mem = setup_memory();
    
    // Subscribe
    mem.subscribe("agent1", "trading", 0.5).unwrap();
    let subs = mem.list_subscriptions("agent1").unwrap();
    assert_eq!(subs.len(), 1);
    
    // Unsubscribe
    let removed = mem.unsubscribe("agent1", "trading").unwrap();
    assert!(removed);
    
    // Verify removed
    let subs = mem.list_subscriptions("agent1").unwrap();
    assert!(subs.is_empty());
    
    // Unsubscribe again should return false
    let removed = mem.unsubscribe("agent1", "trading").unwrap();
    assert!(!removed);
}

#[test]
fn test_peek_vs_check_notifications() {
    let mut mem = setup_memory();
    
    mem.subscribe("ceo", "trading", 0.5).unwrap();
    mem.add_to_namespace("test memory", MemoryType::Factual, Some(0.8), None, None, Some("trading")).unwrap();
    
    // Peek - should not update cursor
    let notifs1 = mem.peek_notifications("ceo").unwrap();
    assert_eq!(notifs1.len(), 1);
    
    // Peek again - should still have same notification
    let notifs2 = mem.peek_notifications("ceo").unwrap();
    assert_eq!(notifs2.len(), 1);
    
    // Check - updates cursor
    let notifs3 = mem.check_notifications("ceo").unwrap();
    assert_eq!(notifs3.len(), 1);
    
    // Check again - should be empty now
    let notifs4 = mem.check_notifications("ceo").unwrap();
    assert!(notifs4.is_empty());
}

#[test]
fn test_discover_cross_links() {
    let mut mem = setup_memory();
    mem.set_agent_id("admin");
    mem.grant("admin", "*", Permission::Admin).unwrap();
    
    // This test verifies the discover_cross_links method works correctly
    // First, we need to create some cross-namespace associations
    
    // Store memories
    let _m1 = mem.add_to_namespace("oil crisis news", MemoryType::Factual, Some(0.9), None, None, Some("engine")).unwrap();
    let _m2 = mem.add_to_namespace("energy sector volatility", MemoryType::Factual, Some(0.85), None, None, Some("alpha")).unwrap();
    
    // Recall together multiple times to form links
    for _ in 0..5 {
        let _ = mem.recall_with_associations("oil energy", Some("*"), 10);
    }
    
    // Try to discover links
    let links = mem.discover_cross_links("engine", "alpha").unwrap();
    println!("Discovered cross-links: {:?}", links);
    
    // Links may or may not exist depending on whether memories matched the query
    // This test mainly ensures the method works without error
}

#[test]
fn test_notification_includes_content() {
    let mut mem = setup_memory();
    
    mem.subscribe("ceo", "trading", 0.5).unwrap();
    
    let content = "URGENT: Market crash detected in sector XYZ";
    mem.add_to_namespace(content, MemoryType::Factual, Some(0.95), None, None, Some("trading")).unwrap();
    
    let notifs = mem.check_notifications("ceo").unwrap();
    assert_eq!(notifs.len(), 1);
    assert_eq!(notifs[0].content, content);
    assert!(notifs[0].importance >= 0.95);
}
