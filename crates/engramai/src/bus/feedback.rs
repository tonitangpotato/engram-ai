//! Behavior Feedback — Track heartbeat action outcomes.
//!
//! Monitors which actions succeed or fail over time, suggesting behavioral adjustments.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Convert a Unix float (seconds since epoch) to `DateTime<Utc>`.
fn f64_to_datetime(ts: f64) -> DateTime<Utc> {
    let secs = ts.floor() as i64;
    let nanos = ((ts - secs as f64) * 1_000_000_000.0).max(0.0) as u32;
    Utc.timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Get the current time as a Unix float (seconds since epoch).
fn now_f64() -> f64 {
    let now = Utc::now();
    now.timestamp() as f64 + now.timestamp_subsec_nanos() as f64 / 1_000_000_000.0
}

/// Default window size for action scoring (recent N attempts).
pub const DEFAULT_SCORE_WINDOW: usize = 20;
/// Threshold for low action score that triggers deprioritization.
pub const LOW_SCORE_THRESHOLD: f64 = 0.2;
/// Minimum attempts before suggesting deprioritization.
pub const MIN_ATTEMPTS_FOR_SUGGESTION: i32 = 10;

/// A logged behavior outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorLog {
    /// Action name/description
    pub action: String,
    /// Whether the outcome was positive (true) or negative (false)
    pub outcome: bool,
    /// When this outcome was recorded
    pub timestamp: DateTime<Utc>,
}

/// Statistics for an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionStats {
    /// Action name
    pub action: String,
    /// Total attempts
    pub total: i32,
    /// Positive outcomes
    pub positive: i32,
    /// Negative outcomes
    pub negative: i32,
    /// Success rate (positive / total)
    pub score: f64,
}

impl ActionStats {
    /// Check if this action should be deprioritized.
    pub fn should_deprioritize(&self) -> bool {
        self.total >= MIN_ATTEMPTS_FOR_SUGGESTION && self.score < LOW_SCORE_THRESHOLD
    }
    
    /// Describe the action performance in human-readable terms.
    pub fn describe(&self) -> String {
        let rating = if self.score >= 0.8 {
            "excellent"
        } else if self.score >= 0.5 {
            "moderate"
        } else if self.score >= 0.2 {
            "poor"
        } else {
            "very poor"
        };
        
        format!(
            "{}: {} ({:.0}% success rate, {}/{} positive)",
            self.action, rating, self.score * 100.0, self.positive, self.total
        )
    }
}

/// Behavior feedback tracker.
pub struct BehaviorFeedback<'a> {
    conn: &'a Connection,
}

impl<'a> BehaviorFeedback<'a> {
    /// Create a new behavior feedback tracker.
    pub fn new(conn: &'a Connection) -> Result<Self, rusqlite::Error> {
        Self::ensure_table(conn)?;
        Ok(Self { conn })
    }
    
    /// Ensure the behavior_log table exists.
    fn ensure_table(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS behavior_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                outcome INTEGER NOT NULL,
                timestamp REAL NOT NULL
            );
            
            CREATE INDEX IF NOT EXISTS idx_behavior_action ON behavior_log(action);
            CREATE INDEX IF NOT EXISTS idx_behavior_timestamp ON behavior_log(timestamp);
            "#,
        )?;
        Ok(())
    }
    
    /// Log an action outcome.
    ///
    /// # Arguments
    ///
    /// * `action` - Name of the action (e.g., "check_email", "run_consolidation")
    /// * `positive` - Whether the outcome was positive
    pub fn log_outcome(&self, action: &str, positive: bool) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO behavior_log (action, outcome, timestamp) VALUES (?, ?, ?)",
            params![action, positive as i32, now_f64()],
        )?;
        Ok(())
    }
    
    /// Get the success score for an action.
    ///
    /// Returns the positive rate over the recent window of attempts.
    /// Returns None if the action has no history.
    pub fn get_action_score(&self, action: &str) -> Result<Option<f64>, rusqlite::Error> {
        self.get_action_score_window(action, DEFAULT_SCORE_WINDOW)
    }
    
    /// Get the success score for an action over a specific window.
    pub fn get_action_score_window(
        &self,
        action: &str,
        window: usize,
    ) -> Result<Option<f64>, rusqlite::Error> {
        // Get recent outcomes
        let mut stmt = self.conn.prepare(
            "SELECT outcome FROM behavior_log WHERE action = ? ORDER BY timestamp DESC LIMIT ?"
        )?;
        
        let outcomes: Vec<bool> = stmt
            .query_map(params![action, window as i64], |row| {
                let outcome: i32 = row.get(0)?;
                Ok(outcome != 0)
            })?
            .filter_map(|r| r.ok())
            .collect();
        
        if outcomes.is_empty() {
            return Ok(None);
        }
        
        let positive_count = outcomes.iter().filter(|&&o| o).count();
        Ok(Some(positive_count as f64 / outcomes.len() as f64))
    }
    
    /// Get full statistics for an action.
    pub fn get_action_stats(&self, action: &str) -> Result<Option<ActionStats>, rusqlite::Error> {
        let result: Option<(i32, i32)> = self.conn
            .query_row(
                "SELECT COUNT(*), SUM(outcome) FROM behavior_log WHERE action = ?",
                params![action],
                |row| {
                    let total: i32 = row.get(0)?;
                    let positive: i32 = row.get::<_, Option<i32>>(1)?.unwrap_or(0);
                    Ok((total, positive))
                },
            )
            .optional()?;
        
        match result {
            Some((total, positive)) if total > 0 => {
                Ok(Some(ActionStats {
                    action: action.to_string(),
                    total,
                    positive,
                    negative: total - positive,
                    score: positive as f64 / total as f64,
                }))
            }
            _ => Ok(None),
        }
    }
    
    /// Get all action statistics.
    pub fn get_all_action_stats(&self) -> Result<Vec<ActionStats>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT action, COUNT(*), SUM(outcome) FROM behavior_log GROUP BY action ORDER BY COUNT(*) DESC"
        )?;
        
        let rows = stmt.query_map([], |row| {
            let action: String = row.get(0)?;
            let total: i32 = row.get(1)?;
            let positive: i32 = row.get::<_, Option<i32>>(2)?.unwrap_or(0);
            Ok(ActionStats {
                action,
                total,
                positive,
                negative: total - positive,
                score: if total > 0 { positive as f64 / total as f64 } else { 0.0 },
            })
        })?;
        
        rows.collect()
    }
    
    /// Get actions that should be deprioritized.
    pub fn get_actions_to_deprioritize(&self) -> Result<Vec<ActionStats>, rusqlite::Error> {
        let all = self.get_all_action_stats()?;
        Ok(all.into_iter().filter(|a| a.should_deprioritize()).collect())
    }
    
    /// Get actions with high success rate.
    pub fn get_successful_actions(&self, min_score: f64) -> Result<Vec<ActionStats>, rusqlite::Error> {
        let all = self.get_all_action_stats()?;
        Ok(all.into_iter()
            .filter(|a| a.total >= MIN_ATTEMPTS_FOR_SUGGESTION && a.score >= min_score)
            .collect())
    }
    
    /// Get recent behavior logs for an action.
    pub fn get_recent_logs(&self, action: &str, limit: usize) -> Result<Vec<BehaviorLog>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT action, outcome, timestamp FROM behavior_log WHERE action = ? ORDER BY timestamp DESC LIMIT ?"
        )?;
        
        let rows = stmt.query_map(params![action, limit as i64], |row| {
            let timestamp_f64: f64 = row.get(2)?;
            let outcome: i32 = row.get(1)?;
            Ok(BehaviorLog {
                action: row.get(0)?,
                outcome: outcome != 0,
                timestamp: f64_to_datetime(timestamp_f64),
            })
        })?;
        
        rows.collect()
    }
    
    /// Clear all logs for an action (e.g., after adjusting HEARTBEAT).
    pub fn clear_action(&self, action: &str) -> Result<usize, rusqlite::Error> {
        let deleted = self.conn.execute(
            "DELETE FROM behavior_log WHERE action = ?",
            params![action],
        )?;
        Ok(deleted)
    }
    
    /// Prune old logs (keep only recent N per action).
    pub fn prune_old_logs(&self, keep_per_action: usize) -> Result<usize, rusqlite::Error> {
        // SQLite doesn't support DELETE with ROW_NUMBER directly,
        // so we use a subquery approach
        let deleted = self.conn.execute(
            r#"
            DELETE FROM behavior_log WHERE id NOT IN (
                SELECT id FROM (
                    SELECT id, ROW_NUMBER() OVER (PARTITION BY action ORDER BY timestamp DESC) as rn
                    FROM behavior_log
                )
                WHERE rn <= ?
            )
            "#,
            params![keep_per_action as i64],
        )?;
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_log_and_get_score() {
        let conn = Connection::open_in_memory().unwrap();
        let feedback = BehaviorFeedback::new(&conn).unwrap();
        
        // Log some outcomes
        feedback.log_outcome("check_email", true).unwrap();
        feedback.log_outcome("check_email", true).unwrap();
        feedback.log_outcome("check_email", false).unwrap();
        feedback.log_outcome("check_email", true).unwrap();
        
        let score = feedback.get_action_score("check_email").unwrap().unwrap();
        // 3 positive out of 4 = 0.75
        assert!((score - 0.75).abs() < 0.01);
    }
    
    #[test]
    fn test_low_score_flagging() {
        let conn = Connection::open_in_memory().unwrap();
        let feedback = BehaviorFeedback::new(&conn).unwrap();
        
        // Log many negative outcomes
        for _ in 0..12 {
            feedback.log_outcome("bad_action", false).unwrap();
        }
        
        let stats = feedback.get_action_stats("bad_action").unwrap().unwrap();
        assert!(stats.should_deprioritize());
        assert!(stats.score < LOW_SCORE_THRESHOLD);
    }
    
    #[test]
    fn test_get_all_stats() {
        let conn = Connection::open_in_memory().unwrap();
        let feedback = BehaviorFeedback::new(&conn).unwrap();
        
        feedback.log_outcome("action_a", true).unwrap();
        feedback.log_outcome("action_b", false).unwrap();
        feedback.log_outcome("action_c", true).unwrap();
        
        let all = feedback.get_all_action_stats().unwrap();
        assert_eq!(all.len(), 3);
    }
    
    #[test]
    fn test_unknown_action() {
        let conn = Connection::open_in_memory().unwrap();
        let feedback = BehaviorFeedback::new(&conn).unwrap();
        
        let score = feedback.get_action_score("nonexistent").unwrap();
        assert!(score.is_none());
    }
    
    #[test]
    fn test_clear_action() {
        let conn = Connection::open_in_memory().unwrap();
        let feedback = BehaviorFeedback::new(&conn).unwrap();
        
        feedback.log_outcome("to_clear", true).unwrap();
        feedback.log_outcome("to_clear", false).unwrap();
        
        let deleted = feedback.clear_action("to_clear").unwrap();
        assert_eq!(deleted, 2);
        
        let score = feedback.get_action_score("to_clear").unwrap();
        assert!(score.is_none());
    }
    
    #[test]
    fn test_get_recent_logs() {
        let conn = Connection::open_in_memory().unwrap();
        let feedback = BehaviorFeedback::new(&conn).unwrap();
        
        feedback.log_outcome("test", true).unwrap();
        feedback.log_outcome("test", false).unwrap();
        feedback.log_outcome("test", true).unwrap();
        
        let logs = feedback.get_recent_logs("test", 10).unwrap();
        assert_eq!(logs.len(), 3);
        // Most recent first
        assert!(logs[0].outcome); // Last logged was true
    }
}
