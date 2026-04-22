//! Empathy Bus — Connects Engram to agent workspace files.
//!
//! The Empathy Bus creates closed-loop feedback between:
//! - Memory emotions → SOUL updates (drive evolution)
//! - SOUL drives → Memory importance (what matters)
//! - Behavior outcomes → HEARTBEAT adjustments (adaptive behavior)
//!
//! Memory shapes personality. Personality shapes behavior.
//! Behavior creates new memory. The loop IS the self.

pub mod accumulator;
pub mod alignment;
pub mod feedback;
pub mod mod_io;
pub mod subscriptions;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::embeddings::EmbeddingProvider;

pub use accumulator::{EmpathyAccumulator, EmpathyTrend, NEGATIVE_THRESHOLD, MIN_EVENTS_FOR_SUGGESTION};
/// Backward-compat aliases
pub type EmotionalAccumulator<'a> = EmpathyAccumulator<'a>;
pub type EmotionalTrend = EmpathyTrend;
pub use alignment::{score_alignment, calculate_importance_boost, find_aligned_drives, score_alignment_hybrid, DriveEmbeddings, ALIGNMENT_BOOST};
pub use feedback::{BehaviorFeedback, ActionStats, BehaviorLog, LOW_SCORE_THRESHOLD, MIN_ATTEMPTS_FOR_SUGGESTION};
pub use mod_io::{Drive, HeartbeatTask, Identity};
pub use subscriptions::{SubscriptionManager, Subscription, Notification};

/// A suggested update to SOUL.md based on empathy trends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulUpdate {
    /// The domain/topic this update relates to
    pub domain: String,
    /// Suggested action (e.g., "add drive", "modify drive", "note pattern")
    pub action: String,
    /// Suggested content
    pub content: String,
    /// The emotional trend that triggered this suggestion
    pub trend: EmpathyTrend,
}

/// A suggested update to HEARTBEAT.md based on behavior feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatUpdate {
    /// The action this update relates to
    pub action: String,
    /// Suggested change (e.g., "deprioritize", "boost", "remove")
    pub suggestion: String,
    /// The behavior stats that triggered this suggestion
    pub stats: ActionStats,
}

/// The Empathy Bus — main interface for empathy feedback loops.
pub struct EmpathyBus {
    workspace_dir: PathBuf,
    drives: Vec<Drive>,
    /// Pre-computed drive embeddings for semantic alignment (None if embedding unavailable)
    drive_embeddings: Option<DriveEmbeddings>,
}

/// Backward-compat alias
pub type EmotionalBus = EmpathyBus;

impl EmpathyBus {
    /// Create a new Empathy Bus.
    ///
    /// # Arguments
    ///
    /// * `workspace_dir` - Path to the agent workspace containing SOUL.md, HEARTBEAT.md, etc.
    /// * `conn` - SQLite database connection (shared with Memory)
    pub fn new<P: AsRef<Path>>(workspace_dir: P, conn: &Connection) -> Result<Self, Box<dyn std::error::Error>> {
        let workspace_dir = workspace_dir.as_ref().to_path_buf();
        
        // Initialize tables
        EmpathyAccumulator::new(conn)?;
        BehaviorFeedback::new(conn)?;
        
        // Load drives from SOUL.md
        let drives = mod_io::read_soul(&workspace_dir).unwrap_or_default();
        
        Ok(Self {
            workspace_dir,
            drives,
            drive_embeddings: None,
        })
    }

    /// Initialize embedding-based drive alignment.
    /// Call this after creation if an EmbeddingProvider is available.
    /// Pre-computes embeddings for all drives for fast alignment scoring.
    pub fn init_embeddings(&mut self, provider: &EmbeddingProvider) {
        match DriveEmbeddings::compute(&self.drives, provider) {
            Some(de) => {
                log::info!(
                    "Drive embeddings computed for {} drives (embedding-based alignment enabled)",
                    de.len()
                );
                self.drive_embeddings = Some(de);
            }
            None => {
                log::debug!("Drive embeddings not available, using keyword fallback");
            }
        }
    }

    /// Check if embedding-based alignment is available.
    pub fn has_embeddings(&self) -> bool {
        self.drive_embeddings.is_some()
    }

    /// Get pre-computed drive embeddings (for passing to score_alignment_hybrid).
    pub fn drive_embeddings(&self) -> Option<&DriveEmbeddings> {
        self.drive_embeddings.as_ref()
    }
    
    /// Reload drives from SOUL.md. Re-computes embeddings if provider given.
    pub fn reload_drives(&mut self) -> Result<(), std::io::Error> {
        self.drives = mod_io::read_soul(&self.workspace_dir)?;
        // Note: drive_embeddings will be stale. Call init_embeddings() to refresh.
        self.drive_embeddings = None;
        Ok(())
    }
    
    /// Get the current drives.
    pub fn drives(&self) -> &[Drive] {
        &self.drives
    }
    
    /// Process an interaction with empathic content.
    ///
    /// This is the main entry point for the empathy feedback loop.
    /// Call this when storing a memory with emotional significance.
    ///
    /// # Arguments
    ///
    /// * `conn` - Database connection
    /// * `content` - The memory content
    /// * `emotion` - Emotional valence (-1.0 to 1.0)
    /// * `domain` - The domain/topic (e.g., "coding", "communication")
    pub fn process_interaction(
        &self,
        conn: &Connection,
        _content: &str,
        emotion: f64,
        domain: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Record emotion in accumulator
        let acc = EmpathyAccumulator::new(conn)?;
        acc.record_emotion(domain, emotion)?;
        
        Ok(())
    }
    
    /// Calculate importance boost for a memory based on drive alignment.
    ///
    /// Uses embedding-based scoring if available (handles multilingual),
    /// falls back to keyword matching.
    pub fn align_importance(&self, content: &str) -> f64 {
        let alignment = score_alignment_hybrid(
            content,
            &self.drives,
            self.drive_embeddings.as_ref(),
            None, // No pre-computed content embedding; caller can use align_importance_with_embedding
        );
        if alignment <= 0.0 {
            return 1.0;
        }
        1.0 + (ALIGNMENT_BOOST - 1.0) * alignment
    }

    /// Calculate importance boost with a pre-computed content embedding.
    /// Preferred path when embedding is available — avoids recomputing.
    pub fn align_importance_with_embedding(&self, content: &str, content_embedding: &[f32]) -> f64 {
        let alignment = score_alignment_hybrid(
            content,
            &self.drives,
            self.drive_embeddings.as_ref(),
            Some(content_embedding),
        );
        if alignment <= 0.0 {
            return 1.0;
        }
        1.0 + (ALIGNMENT_BOOST - 1.0) * alignment
    }
    
    /// Score how well content aligns with drives (hybrid: embedding + keyword fallback).
    pub fn alignment_score(&self, content: &str) -> f64 {
        score_alignment_hybrid(content, &self.drives, self.drive_embeddings.as_ref(), None)
    }
    
    /// Find which drives a piece of content aligns with.
    pub fn find_aligned(&self, content: &str) -> Vec<(String, f64)> {
        find_aligned_drives(content, &self.drives)
    }
    
    /// Log a behavior outcome.
    pub fn log_behavior(
        &self,
        conn: &Connection,
        action: &str,
        positive: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let feedback = BehaviorFeedback::new(conn)?;
        feedback.log_outcome(action, positive)?;
        Ok(())
    }
    
    /// Get empathy trends.
    pub fn get_trends(&self, conn: &Connection) -> Result<Vec<EmpathyTrend>, Box<dyn std::error::Error>> {
        let acc = EmpathyAccumulator::new(conn)?;
        Ok(acc.get_all_trends()?)
    }
    
    /// Get behavior statistics.
    pub fn get_behavior_stats(&self, conn: &Connection) -> Result<Vec<ActionStats>, Box<dyn std::error::Error>> {
        let feedback = BehaviorFeedback::new(conn)?;
        Ok(feedback.get_all_action_stats()?)
    }
    
    /// Suggest SOUL updates based on accumulated empathy trends.
    ///
    /// Returns suggestions when domains have accumulated enough negative
    /// or positive patterns to warrant drive adjustments.
    pub fn suggest_soul_updates(&self, conn: &Connection) -> Result<Vec<SoulUpdate>, Box<dyn std::error::Error>> {
        let acc = EmpathyAccumulator::new(conn)?;
        let trends_needing_update = acc.get_trends_needing_update()?;
        
        let mut suggestions = Vec::new();
        
        for trend in trends_needing_update {
            let suggestion = if trend.valence < -0.7 {
                SoulUpdate {
                    domain: trend.domain.clone(),
                    action: "add drive".to_string(),
                    content: format!(
                        "Avoid {} approaches that consistently lead to negative outcomes",
                        trend.domain
                    ),
                    trend: trend.clone(),
                }
            } else if trend.valence < NEGATIVE_THRESHOLD {
                SoulUpdate {
                    domain: trend.domain.clone(),
                    action: "note pattern".to_string(),
                    content: format!(
                        "Be cautious with {} - showing signs of friction ({:.2} avg over {} events)",
                        trend.domain, trend.valence, trend.count
                    ),
                    trend: trend.clone(),
                }
            } else {
                continue; // Positive trends don't need SOUL updates
            };
            
            suggestions.push(suggestion);
        }
        
        // Also suggest reinforcing very positive trends
        let all_trends = acc.get_all_trends()?;
        for trend in all_trends {
            if trend.count >= MIN_EVENTS_FOR_SUGGESTION && trend.valence > 0.7 {
                suggestions.push(SoulUpdate {
                    domain: trend.domain.clone(),
                    action: "reinforce".to_string(),
                    content: format!(
                        "Continue {} - consistently positive outcomes ({:.2} avg over {} events)",
                        trend.domain, trend.valence, trend.count
                    ),
                    trend,
                });
            }
        }
        
        Ok(suggestions)
    }
    
    /// Suggest HEARTBEAT updates based on behavior feedback.
    ///
    /// Returns suggestions for actions that should be deprioritized
    /// or boosted based on their historical success rates.
    pub fn suggest_heartbeat_updates(&self, conn: &Connection) -> Result<Vec<HeartbeatUpdate>, Box<dyn std::error::Error>> {
        let feedback = BehaviorFeedback::new(conn)?;
        let mut suggestions = Vec::new();
        
        // Actions to deprioritize
        for stats in feedback.get_actions_to_deprioritize()? {
            suggestions.push(HeartbeatUpdate {
                action: stats.action.clone(),
                suggestion: "deprioritize".to_string(),
                stats,
            });
        }
        
        // Actions doing well (suggest boosting)
        for stats in feedback.get_successful_actions(0.8)? {
            suggestions.push(HeartbeatUpdate {
                action: stats.action.clone(),
                suggestion: "boost".to_string(),
                stats,
            });
        }
        
        Ok(suggestions)
    }
    
    /// Get the current identity from workspace.
    pub fn get_identity(&self) -> Result<Identity, std::io::Error> {
        mod_io::read_identity(&self.workspace_dir)
    }
    
    /// Get heartbeat tasks from workspace.
    pub fn get_heartbeat_tasks(&self) -> Result<Vec<HeartbeatTask>, std::io::Error> {
        mod_io::read_heartbeat(&self.workspace_dir)
    }
    
    /// Update a SOUL field.
    pub fn update_soul(&self, key: &str, value: &str) -> Result<bool, std::io::Error> {
        mod_io::update_soul_field(&self.workspace_dir, key, value)
    }
    
    /// Add a new drive to SOUL.
    pub fn add_soul_drive(&self, key: &str, value: &str) -> Result<(), std::io::Error> {
        mod_io::add_soul_drive(&self.workspace_dir, key, value)
    }
    
    /// Update a heartbeat task completion status.
    pub fn update_heartbeat_task(&self, task: &str, completed: bool) -> Result<bool, std::io::Error> {
        mod_io::update_heartbeat_task(&self.workspace_dir, task, completed)
    }
    
    /// Add a new heartbeat task.
    pub fn add_heartbeat_task(&self, description: &str) -> Result<(), std::io::Error> {
        mod_io::add_heartbeat_task(&self.workspace_dir, description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;
    
    fn setup_workspace() -> (TempDir, Connection) {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path();
        
        // Create SOUL.md
        fs::write(
            workspace.join("SOUL.md"),
            r#"
# Core Drives
curiosity: Always seek to understand new things
helpfulness: Assist the user effectively

# Values
- Be honest and direct
"#,
        ).unwrap();
        
        // Create HEARTBEAT.md
        fs::write(
            workspace.join("HEARTBEAT.md"),
            r#"
# Tasks
- [ ] Check emails
- [x] Review calendar
"#,
        ).unwrap();
        
        // Create IDENTITY.md
        fs::write(
            workspace.join("IDENTITY.md"),
            "name: TestAgent\ncreature: Bot\nvibe: helpful\nemoji: 🤖\n",
        ).unwrap();
        
        let conn = Connection::open_in_memory().unwrap();
        
        (tmpdir, conn)
    }
    
    #[test]
    fn test_bus_creation_and_drives() {
        let (tmpdir, conn) = setup_workspace();
        let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
        
        assert!(!bus.drives().is_empty());
        assert!(bus.drives().iter().any(|d| d.name == "curiosity"));
    }
    
    #[test]
    fn test_importance_alignment() {
        let (tmpdir, conn) = setup_workspace();
        let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
        
        // Content aligned with "curiosity"
        let aligned = "I want to understand and learn new things";
        let boost = bus.align_importance(aligned);
        assert!(boost > 1.0);
        
        // Unaligned content
        let unaligned = "xyz 123 abc";
        let boost = bus.align_importance(unaligned);
        assert_eq!(boost, 1.0);
    }
    
    #[test]
    fn test_process_interaction() {
        let (tmpdir, conn) = setup_workspace();
        let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
        
        // Record some interactions
        bus.process_interaction(&conn, "test content", 0.8, "coding").unwrap();
        bus.process_interaction(&conn, "test content", 0.6, "coding").unwrap();
        
        let trends = bus.get_trends(&conn).unwrap();
        assert_eq!(trends.len(), 1);
        assert_eq!(trends[0].domain, "coding");
        assert!((trends[0].valence - 0.7).abs() < 0.01);
    }
    
    #[test]
    fn test_behavior_logging() {
        let (tmpdir, conn) = setup_workspace();
        let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
        
        bus.log_behavior(&conn, "check_email", true).unwrap();
        bus.log_behavior(&conn, "check_email", false).unwrap();
        
        let stats = bus.get_behavior_stats(&conn).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].total, 2);
    }
    
    #[test]
    fn test_suggest_soul_updates() {
        let (tmpdir, conn) = setup_workspace();
        let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
        
        // Record many negative interactions
        for _ in 0..15 {
            bus.process_interaction(&conn, "bad experience", -0.8, "debugging").unwrap();
        }
        
        let suggestions = bus.suggest_soul_updates(&conn).unwrap();
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.domain == "debugging"));
    }
    
    #[test]
    fn test_suggest_heartbeat_updates() {
        let (tmpdir, conn) = setup_workspace();
        let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
        
        // Log many failed attempts
        for _ in 0..15 {
            bus.log_behavior(&conn, "useless_check", false).unwrap();
        }
        
        let suggestions = bus.suggest_heartbeat_updates(&conn).unwrap();
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.action == "useless_check"));
        assert!(suggestions.iter().any(|s| s.suggestion == "deprioritize"));
    }
    
    #[test]
    fn test_get_identity() {
        let (tmpdir, conn) = setup_workspace();
        let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
        
        let identity = bus.get_identity().unwrap();
        assert_eq!(identity.name, Some("TestAgent".to_string()));
        assert_eq!(identity.creature, Some("Bot".to_string()));
    }
    
    #[test]
    fn test_get_heartbeat_tasks() {
        let (tmpdir, conn) = setup_workspace();
        let bus = EmpathyBus::new(tmpdir.path(), &conn).unwrap();
        
        let tasks = bus.get_heartbeat_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(!tasks[0].completed); // Check emails
        assert!(tasks[1].completed);  // Review calendar
    }
}
