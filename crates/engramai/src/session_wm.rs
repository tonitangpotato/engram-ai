//! Session Working Memory — Miller's Law-constrained active memory buffer.
//!
//! Based on George Miller's 7±2 rule: human working memory has limited capacity.
//! This module manages what's "currently active" in a session to avoid redundant
//! full-database searches when topic is continuous.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Working memory capacity (Miller's Law: 7±2 items).
const DEFAULT_CAPACITY: usize = 7;

/// Default decay time for working memory items (5 minutes).
const DEFAULT_DECAY_SECS: u64 = 300;

/// Scores cached from the full recall that populated a working memory item.
#[derive(Debug, Clone)]
pub struct CachedScore {
    pub confidence: f64,
    pub activation: f64,
}

/// A single session's working memory state.
#[derive(Debug, Clone)]
pub struct SessionWorkingMemory {
    /// Maximum number of items in working memory
    capacity: usize,
    /// Decay time for items
    decay_duration: Duration,
    /// Memory ID -> last activated time
    items: HashMap<String, Instant>,
    /// Cached scores from original full recall
    scores: HashMap<String, CachedScore>,
    /// Last topic/query for continuity check
    last_query: Option<String>,
}

impl Default for SessionWorkingMemory {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY, DEFAULT_DECAY_SECS)
    }
}

impl SessionWorkingMemory {
    /// Create a new session working memory with specified capacity and decay.
    pub fn new(capacity: usize, decay_secs: u64) -> Self {
        Self {
            capacity,
            decay_duration: Duration::from_secs(decay_secs),
            items: HashMap::new(),
            scores: HashMap::new(),
            last_query: None,
        }
    }
    
    /// Create with default settings (capacity=7, decay=300s).
    pub fn with_defaults() -> Self {
        Self::default()
    }
    
    /// Activate memory IDs in working memory.
    ///
    /// Adds new items and updates timestamps for existing ones.
    /// Triggers pruning if capacity is exceeded.
    pub fn activate(&mut self, memory_ids: &[String]) {
        let now = Instant::now();
        
        for id in memory_ids {
            self.items.insert(id.clone(), now);
        }
        
        self.prune();
    }
    
    /// Activate memory IDs with their scores for cached recall.
    ///
    /// Stores confidence and activation from the full recall so the cached
    /// path can reuse them instead of recomputing with zero signals.
    pub fn activate_with_scores(&mut self, entries: &[(String, f64, f64)]) {
        let now = Instant::now();
        for (id, confidence, activation) in entries {
            self.items.insert(id.clone(), now);
            self.scores.insert(id.clone(), CachedScore {
                confidence: *confidence,
                activation: *activation,
            });
        }
        self.prune();
    }
    
    /// Get cached score for a memory ID.
    pub fn get_score(&self, id: &str) -> Option<&CachedScore> {
        self.scores.get(id)
    }
    
    /// Set the last query for topic continuity checking.
    pub fn set_query(&mut self, query: &str) {
        self.last_query = Some(query.to_string());
    }
    
    /// Get the last query.
    pub fn last_query(&self) -> Option<&str> {
        self.last_query.as_deref()
    }
    
    /// Prune expired and over-capacity items.
    pub fn prune(&mut self) {
        let now = Instant::now();
        
        // Remove expired items
        self.items.retain(|_, activated_at| {
            now.duration_since(*activated_at) < self.decay_duration
        });
        
        // If still over capacity, remove oldest items
        while self.items.len() > self.capacity {
            // Find the oldest item
            let oldest = self.items
                .iter()
                .min_by_key(|(_, &t)| t)
                .map(|(k, _)| k.clone());
            
            if let Some(oldest_id) = oldest {
                self.items.remove(&oldest_id);
            } else {
                break;
            }
        }
        
        // Clean scores for pruned items
        self.scores.retain(|id, _| self.items.contains_key(id));
    }
    
    /// Get currently active memory IDs (after pruning).
    pub fn get_active_ids(&mut self) -> Vec<String> {
        self.prune();
        self.items.keys().cloned().collect()
    }
    
    /// Get number of active items.
    pub fn len(&self) -> usize {
        self.items.len()
    }
    
    /// Check if working memory is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    
    /// Clear all items from working memory.
    pub fn clear(&mut self) {
        self.items.clear();
        self.scores.clear();
        self.last_query = None;
    }
    
    /// Check if a memory is currently in working memory.
    pub fn contains(&self, memory_id: &str) -> bool {
        self.items.contains_key(memory_id)
    }
    
    /// Calculate overlap between current WM and a set of memory IDs.
    ///
    /// Returns (overlap_count, overlap_ratio) where ratio is 0.0-1.0.
    pub fn overlap(&mut self, memory_ids: &[String]) -> (usize, f64) {
        self.prune();
        
        if self.items.is_empty() || memory_ids.is_empty() {
            return (0, 0.0);
        }
        
        let overlap_count = memory_ids.iter()
            .filter(|id| self.items.contains_key(*id))
            .count();
        
        let union_size = self.items.len() + memory_ids.len() - overlap_count;
        let ratio = if union_size > 0 {
            overlap_count as f64 / union_size as f64
        } else {
            0.0
        };
        
        (overlap_count, ratio)
    }
    
    /// Check if topic is continuous (based on overlap ratio).
    ///
    /// If overlap with probe results >= threshold, topic is continuous.
    pub fn is_topic_continuous(&mut self, probe_ids: &[String], threshold: f64) -> bool {
        let (_, ratio) = self.overlap(probe_ids);
        ratio >= threshold
    }
}

/// Registry of session working memories.
///
/// Manages multiple sessions, each with their own working memory.
#[derive(Debug, Default)]
pub struct SessionRegistry {
    sessions: HashMap<String, SessionWorkingMemory>,
    /// Default capacity for new sessions
    default_capacity: usize,
    /// Default decay seconds for new sessions
    default_decay_secs: u64,
}

impl SessionRegistry {
    /// Create a new session registry with default settings.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            default_capacity: DEFAULT_CAPACITY,
            default_decay_secs: DEFAULT_DECAY_SECS,
        }
    }
    
    /// Create with custom default settings.
    pub fn with_defaults(capacity: usize, decay_secs: u64) -> Self {
        Self {
            sessions: HashMap::new(),
            default_capacity: capacity,
            default_decay_secs: decay_secs,
        }
    }
    
    /// Get or create a session's working memory.
    pub fn get_session(&mut self, session_id: &str) -> &mut SessionWorkingMemory {
        self.sessions.entry(session_id.to_string()).or_insert_with(|| {
            SessionWorkingMemory::new(self.default_capacity, self.default_decay_secs)
        })
    }
    
    /// Get a session's working memory if it exists.
    pub fn get_session_if_exists(&self, session_id: &str) -> Option<&SessionWorkingMemory> {
        self.sessions.get(session_id)
    }
    
    /// Clear a specific session.
    pub fn clear_session(&mut self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }
    
    /// List all active session IDs.
    pub fn list_sessions(&self) -> Vec<&str> {
        self.sessions.keys().map(|s| s.as_str()).collect()
    }
    
    /// Get count of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
    
    /// Prune all sessions (remove expired items from each).
    pub fn prune_all(&mut self) {
        for session in self.sessions.values_mut() {
            session.prune();
        }
    }
    
    /// Remove empty sessions.
    pub fn remove_empty_sessions(&mut self) -> usize {
        let before = self.sessions.len();
        self.sessions.retain(|_, wm| !wm.is_empty());
        before - self.sessions.len()
    }
}

/// Result of session-aware recall.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionRecallResult {
    /// The recall results
    pub results: Vec<crate::types::RecallResult>,
    /// Whether a full recall was performed (vs. using cached WM)
    pub full_recall: bool,
    /// Number of items in working memory after this recall
    pub wm_size: usize,
    /// Topic continuity ratio (0.0-1.0)
    pub continuity_ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;
    
    #[test]
    fn test_basic_activation() {
        let mut wm = SessionWorkingMemory::new(7, 300);
        
        assert!(wm.is_empty());
        
        wm.activate(&["a".to_string(), "b".to_string()]);
        
        assert_eq!(wm.len(), 2);
        assert!(wm.contains("a"));
        assert!(wm.contains("b"));
        assert!(!wm.contains("c"));
    }
    
    #[test]
    fn test_capacity_pruning() {
        let mut wm = SessionWorkingMemory::new(3, 300);
        
        // Activate more than capacity
        wm.activate(&["a".to_string(), "b".to_string(), "c".to_string()]);
        sleep(Duration::from_millis(10));
        wm.activate(&["d".to_string(), "e".to_string()]);
        
        // Should be pruned to capacity
        assert!(wm.len() <= 3);
    }
    
    #[test]
    fn test_overlap_calculation() {
        let mut wm = SessionWorkingMemory::new(7, 300);
        wm.activate(&["a".to_string(), "b".to_string(), "c".to_string()]);
        
        // 2 out of 3 overlap -> ratio = 2 / (3 + 3 - 2) = 0.5
        let probe = vec!["a".to_string(), "b".to_string(), "d".to_string()];
        let (count, ratio) = wm.overlap(&probe);
        
        assert_eq!(count, 2);
        assert!((ratio - 0.5).abs() < 0.01);
    }
    
    #[test]
    fn test_topic_continuity() {
        let mut wm = SessionWorkingMemory::new(7, 300);
        wm.activate(&["a".to_string(), "b".to_string(), "c".to_string()]);
        
        // High overlap -> continuous
        let continuous = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(wm.is_topic_continuous(&continuous, 0.6));
        
        // Low overlap -> not continuous
        let different = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        assert!(!wm.is_topic_continuous(&different, 0.6));
    }
    
    #[test]
    fn test_session_registry() {
        let mut registry = SessionRegistry::new();
        
        // Get or create sessions
        registry.get_session("session1").activate(&["a".to_string()]);
        registry.get_session("session2").activate(&["b".to_string()]);
        
        assert_eq!(registry.session_count(), 2);
        assert!(registry.list_sessions().contains(&"session1"));
        
        // Clear session
        assert!(registry.clear_session("session1"));
        assert_eq!(registry.session_count(), 1);
    }
    
    #[test]
    fn test_decay_pruning() {
        // Use a very short decay for testing
        let mut wm = SessionWorkingMemory::new(7, 0); // 0 seconds = immediate decay
        
        wm.items.insert("a".to_string(), Instant::now() - Duration::from_secs(1));
        
        // After prune, item should be gone
        wm.prune();
        assert!(wm.is_empty());
    }
}
