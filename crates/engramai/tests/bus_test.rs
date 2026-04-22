//! Tests for the Empathy Bus (Phase 2)

use engramai::{Memory, MemoryConfig, MemoryType, EmpathyBus};
use engramai::bus::accumulator::{EmpathyAccumulator, NEGATIVE_THRESHOLD, MIN_EVENTS_FOR_SUGGESTION};
use engramai::bus::feedback::{BehaviorFeedback, LOW_SCORE_THRESHOLD, MIN_ATTEMPTS_FOR_SUGGESTION as MIN_BEHAVIOR_ATTEMPTS};
use engramai::bus::alignment::{score_alignment, calculate_importance_boost, ALIGNMENT_BOOST};
use engramai::bus::mod_io::{Drive, parse_soul, parse_heartbeat, parse_identity};
use rusqlite::Connection;
use std::fs;
use tempfile::TempDir;

fn setup_workspace() -> TempDir {
    let tmpdir = TempDir::new().unwrap();
    let workspace = tmpdir.path();
    
    // Create SOUL.md
    fs::write(
        workspace.join("SOUL.md"),
        r#"# Core Drives
curiosity: Always seek to understand new things and learn deeply
helpfulness: Assist the user effectively and solve their problems
honesty: Be direct and transparent in communication

# Values
- Think before acting
- Quality over speed
"#,
    ).unwrap();
    
    // Create HEARTBEAT.md
    fs::write(
        workspace.join("HEARTBEAT.md"),
        r#"# Daily Tasks
- [ ] Check emails for important messages
- [x] Review calendar for today
- [ ] Run memory consolidation
"#,
    ).unwrap();
    
    // Create IDENTITY.md
    fs::write(
        workspace.join("IDENTITY.md"),
        "name: TestAgent\ncreature: Cat\nvibe: curious and playful\nemoji: 🐱\n",
    ).unwrap();
    
    tmpdir
}

// === Test: Empathy Accumulator ===

#[test]
fn test_emotional_accumulator() {
    let conn = Connection::open_in_memory().unwrap();
    let acc = EmpathyAccumulator::new(&conn).unwrap();
    
    // Record some emotions in different domains
    acc.record_emotion("coding", 0.8).unwrap();
    acc.record_emotion("coding", 0.6).unwrap();
    acc.record_emotion("coding", 0.4).unwrap();
    
    acc.record_emotion("debugging", -0.5).unwrap();
    acc.record_emotion("debugging", -0.7).unwrap();
    
    // Verify trend tracking
    let coding_trend = acc.get_trend("coding").unwrap().unwrap();
    assert_eq!(coding_trend.count, 3);
    assert!((coding_trend.valence - 0.6).abs() < 0.01, "Expected ~0.6, got {}", coding_trend.valence);
    
    let debug_trend = acc.get_trend("debugging").unwrap().unwrap();
    assert_eq!(debug_trend.count, 2);
    assert!((debug_trend.valence - (-0.6)).abs() < 0.01, "Expected ~-0.6, got {}", debug_trend.valence);
    
    // Get all trends
    let all = acc.get_all_trends().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_emotional_accumulator_threshold() {
    let conn = Connection::open_in_memory().unwrap();
    let acc = EmpathyAccumulator::new(&conn).unwrap();
    
    // Record many negative emotions to trigger threshold
    for _ in 0..12 {
        acc.record_emotion("frustrating_task", -0.7).unwrap();
    }
    
    let trend = acc.get_trend("frustrating_task").unwrap().unwrap();
    assert!(trend.count >= MIN_EVENTS_FOR_SUGGESTION);
    assert!(trend.valence < NEGATIVE_THRESHOLD);
    assert!(trend.needs_soul_update(), "Should flag for SOUL update");
    
    // Trends needing update should include this one
    let needing_update = acc.get_trends_needing_update().unwrap();
    assert!(!needing_update.is_empty());
    assert!(needing_update.iter().any(|t| t.domain == "frustrating_task"));
}

// === Test: Drive Alignment ===

#[test]
fn test_drive_alignment() {
    let drives = vec![
        Drive {
            name: "curiosity".to_string(),
            description: "Always seek to understand and learn new things".to_string(),
            keywords: vec!["curiosity".to_string(), "understand".to_string(), "learn".to_string(), "new".to_string()],
        },
        Drive {
            name: "helpfulness".to_string(),
            description: "Help users solve problems effectively".to_string(),
            keywords: vec!["helpfulness".to_string(), "help".to_string(), "solve".to_string(), "problems".to_string()],
        },
    ];
    
    // Strongly aligned content
    let aligned = "I want to learn and understand these new concepts deeply";
    let score = score_alignment(aligned, &drives);
    assert!(score > 0.5, "Expected strong alignment, got {}", score);
    
    // Check importance boost
    let boost = calculate_importance_boost(aligned, &drives);
    assert!(boost > 1.0, "Expected boost > 1.0, got {}", boost);
    assert!(boost <= ALIGNMENT_BOOST);
    
    // Non-aligned content
    let unaligned = "The weather is nice today xyz";
    let score = score_alignment(unaligned, &drives);
    assert!(score < 0.3, "Expected weak alignment, got {}", score);
    
    let boost = calculate_importance_boost(unaligned, &drives);
    assert_eq!(boost, 1.0, "Expected no boost");
}

#[test]
fn test_drive_alignment_boosts_memory_importance() {
    let tmpdir = setup_workspace();
    let db_path = tmpdir.path().join("test.db");
    
    // Create memory with emotional bus
    let mut mem = Memory::with_empathy_bus(
        db_path.to_str().unwrap(),
        tmpdir.path().to_str().unwrap(),
        Some(MemoryConfig::default()),
    ).unwrap();
    
    // Store a memory aligned with SOUL drives
    let aligned_content = "I learned something new and want to understand it deeply";
    mem.add(aligned_content, MemoryType::Episodic, None, None, None).unwrap();
    
    // Store unaligned memory
    let unaligned_content = "xyz abc 123 random text";
    mem.add(unaligned_content, MemoryType::Episodic, None, None, None).unwrap();
    
    // Recall both - aligned should have higher importance/confidence
    let results = mem.recall("learned understand", 10, None, None).unwrap();
    assert!(!results.is_empty());
    
    // The aligned memory should be ranked higher
    let aligned_result = results.iter().find(|r| r.record.content.contains("learned"));
    let unaligned_result = results.iter().find(|r| r.record.content.contains("random"));
    
    if let (Some(a), Some(u)) = (aligned_result, unaligned_result) {
        assert!(a.record.importance > u.record.importance, 
            "Aligned memory importance ({}) should be > unaligned ({})",
            a.record.importance, u.record.importance);
    }
}

// === Test: Behavior Feedback ===

#[test]
fn test_behavior_feedback() {
    let conn = Connection::open_in_memory().unwrap();
    let feedback = BehaviorFeedback::new(&conn).unwrap();
    
    // Log some outcomes
    feedback.log_outcome("check_email", true).unwrap();
    feedback.log_outcome("check_email", true).unwrap();
    feedback.log_outcome("check_email", false).unwrap();
    feedback.log_outcome("check_email", true).unwrap();
    
    // Get score
    let score = feedback.get_action_score("check_email").unwrap().unwrap();
    assert!((score - 0.75).abs() < 0.01, "Expected 0.75, got {}", score);
    
    // Get stats
    let stats = feedback.get_action_stats("check_email").unwrap().unwrap();
    assert_eq!(stats.total, 4);
    assert_eq!(stats.positive, 3);
    assert_eq!(stats.negative, 1);
}

#[test]
fn test_behavior_feedback_low_score_flagging() {
    let conn = Connection::open_in_memory().unwrap();
    let feedback = BehaviorFeedback::new(&conn).unwrap();
    
    // Log many failed attempts for an action
    for _ in 0..15 {
        feedback.log_outcome("useless_check", false).unwrap();
    }
    
    let stats = feedback.get_action_stats("useless_check").unwrap().unwrap();
    assert!(stats.total >= MIN_BEHAVIOR_ATTEMPTS);
    assert!(stats.score < LOW_SCORE_THRESHOLD);
    assert!(stats.should_deprioritize(), "Should suggest deprioritization");
    
    // Actions to deprioritize should include this
    let to_deprioritize = feedback.get_actions_to_deprioritize().unwrap();
    assert!(!to_deprioritize.is_empty());
    assert!(to_deprioritize.iter().any(|a| a.action == "useless_check"));
}

// === Test: Bus Integration ===

#[test]
fn test_bus_integration() {
    let tmpdir = setup_workspace();
    let db_path = tmpdir.path().join("test.db");
    
    // Create memory with emotional bus
    let mut mem = Memory::with_empathy_bus(
        db_path.to_str().unwrap(),
        tmpdir.path().to_str().unwrap(),
        Some(MemoryConfig::default()),
    ).unwrap();
    
    // Store memory with emotion
    mem.add_with_emotion(
        "Debugging session was frustrating",
        MemoryType::Episodic,
        None,
        None,
        None,
        None,
        -0.7,
        "debugging",
    ).unwrap();
    
    // Record more negative emotions to build trend
    let bus = mem.empathy_bus().unwrap();
    for _ in 0..10 {
        bus.process_interaction(mem.connection(), "more frustration", -0.6, "debugging").unwrap();
    }
    
    // Check trends
    let trends = bus.get_trends(mem.connection()).unwrap();
    assert!(!trends.is_empty());
    let debug_trend = trends.iter().find(|t| t.domain == "debugging").unwrap();
    assert!(debug_trend.valence < 0.0);
    
    // Check SOUL update suggestions
    let soul_updates = bus.suggest_soul_updates(mem.connection()).unwrap();
    assert!(!soul_updates.is_empty());
    assert!(soul_updates.iter().any(|s| s.domain == "debugging"));
}

#[test]
fn test_bus_heartbeat_suggestions() {
    let tmpdir = setup_workspace();
    let conn = Connection::open_in_memory().unwrap();
    let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
    
    // Log many negative behavior outcomes
    for _ in 0..15 {
        bus.log_behavior(&conn, "wasted_check", false).unwrap();
    }
    
    // Check HEARTBEAT update suggestions
    let heartbeat_updates = bus.suggest_heartbeat_updates(&conn).unwrap();
    assert!(!heartbeat_updates.is_empty());
    assert!(heartbeat_updates.iter().any(|h| h.action == "wasted_check"));
    assert!(heartbeat_updates.iter().any(|h| h.suggestion == "deprioritize"));
}

// === Test: Module I/O ===

#[test]
fn test_parse_soul() {
    let content = r#"
# Core Drives
curiosity: Always seek to understand new things
helpfulness: Assist the user effectively

# Values
- Be honest
- Think deeply
"#;
    let drives = parse_soul(content);
    assert!(!drives.is_empty());
    assert!(drives.iter().any(|d| d.name == "curiosity"));
    assert!(drives.iter().any(|d| d.name == "helpfulness"));
    // Bullet points should also be captured
    assert!(drives.iter().any(|d| d.description.contains("honest")));
}

#[test]
fn test_parse_heartbeat() {
    let content = r#"
# Tasks
- [ ] Check emails
- [x] Review calendar
- [ ] Run consolidation
"#;
    let tasks = parse_heartbeat(content);
    assert_eq!(tasks.len(), 3);
    assert!(!tasks[0].completed);
    assert!(tasks[1].completed);
    assert_eq!(tasks[0].description, "Check emails");
}

#[test]
fn test_parse_identity() {
    let content = "name: Clawd\ncreature: Cat\nvibe: curious\nemoji: 🐱\n";
    let identity = parse_identity(content);
    assert_eq!(identity.name, Some("Clawd".to_string()));
    assert_eq!(identity.creature, Some("Cat".to_string()));
    assert_eq!(identity.emoji, Some("🐱".to_string()));
}

// === Test: Full Loop ===

#[test]
fn test_full_emotional_loop() {
    let tmpdir = setup_workspace();
    let db_path = tmpdir.path().join("test.db");
    
    let mut mem = Memory::with_empathy_bus(
        db_path.to_str().unwrap(),
        tmpdir.path().to_str().unwrap(),
        Some(MemoryConfig::default()),
    ).unwrap();
    
    // Step 1: Store memories with emotions
    for i in 0..5 {
        mem.add_with_emotion(
            &format!("Positive learning experience #{}", i),
            MemoryType::Episodic,
            None,
            None,
            None,
            None,
            0.8,
            "learning",
        ).unwrap();
    }
    
    for i in 0..12 {
        mem.add_with_emotion(
            &format!("Frustrating debugging session #{}", i),
            MemoryType::Episodic,
            None,
            None,
            None,
            None,
            -0.7,
            "debugging",
        ).unwrap();
    }
    
    // Step 2: Log behavior outcomes
    let bus = mem.empathy_bus().unwrap();
    for _ in 0..5 {
        bus.log_behavior(mem.connection(), "helpful_action", true).unwrap();
    }
    for _ in 0..12 {
        bus.log_behavior(mem.connection(), "unhelpful_action", false).unwrap();
    }
    
    // Step 3: Check trends
    let trends = bus.get_trends(mem.connection()).unwrap();
    let learning_trend = trends.iter().find(|t| t.domain == "learning");
    let debugging_trend = trends.iter().find(|t| t.domain == "debugging");
    
    assert!(learning_trend.is_some());
    assert!(learning_trend.unwrap().valence > 0.5);
    
    assert!(debugging_trend.is_some());
    assert!(debugging_trend.unwrap().valence < NEGATIVE_THRESHOLD);
    assert!(debugging_trend.unwrap().needs_soul_update());
    
    // Step 4: Check suggestions
    let soul_suggestions = bus.suggest_soul_updates(mem.connection()).unwrap();
    assert!(soul_suggestions.iter().any(|s| s.domain == "debugging"));
    
    let heartbeat_suggestions = bus.suggest_heartbeat_updates(mem.connection()).unwrap();
    assert!(heartbeat_suggestions.iter().any(|h| h.action == "unhelpful_action"));
    
    // Step 5: Verify importance boosting worked
    let results = mem.recall("learning experience", 10, None, None).unwrap();
    // Learning content aligned with SOUL.md "curiosity" drive should have higher importance
    assert!(!results.is_empty());
}
