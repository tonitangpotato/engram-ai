//! Subscription and Notification Model for Cross-Agent Intelligence.
//!
//! Agents can subscribe to namespaces to receive notifications when new
//! high-importance memories are stored. This enables the CEO pattern where
//! a supervisor agent monitors all specialist agents without polling.
//!
//! Example: CEO subscribes to all namespaces with min_importance=0.8
//! → Gets notified of high-importance events from any service agent.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};

/// Convert a `DateTime<Utc>` to a Unix float (seconds since epoch).
fn datetime_to_f64(dt: &DateTime<Utc>) -> f64 {
    dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1_000_000_000.0
}

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
    datetime_to_f64(&Utc::now())
}

/// A notification about a new memory that exceeded a subscription threshold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// Memory ID
    pub memory_id: String,
    /// Namespace the memory was stored in
    pub namespace: String,
    /// Memory content (for convenience)
    pub content: String,
    /// Memory importance
    pub importance: f64,
    /// When the memory was created
    pub created_at: DateTime<Utc>,
    /// The subscription that triggered this notification
    pub subscription_namespace: String,
    /// The threshold that was exceeded
    pub threshold: f64,
}

/// A subscription entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    /// Agent ID of the subscriber
    pub subscriber_id: String,
    /// Namespace to watch ("*" = all namespaces)
    pub namespace: String,
    /// Minimum importance to trigger notification
    pub min_importance: f64,
    /// When this subscription was created
    pub created_at: DateTime<Utc>,
}

/// Manages subscriptions and notifications.
pub struct SubscriptionManager<'a> {
    conn: &'a Connection,
}

impl<'a> SubscriptionManager<'a> {
    /// Create a new SubscriptionManager, initializing tables if needed.
    pub fn new(conn: &'a Connection) -> Result<Self, Box<dyn std::error::Error>> {
        Self::init_tables(conn)?;
        Ok(Self { conn })
    }
    
    /// Initialize subscription tables.
    fn init_tables(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS subscriptions (
                subscriber_id TEXT NOT NULL,
                namespace TEXT NOT NULL,
                min_importance REAL NOT NULL,
                created_at REAL NOT NULL,
                PRIMARY KEY (subscriber_id, namespace)
            );
            
            CREATE TABLE IF NOT EXISTS notification_cursor (
                agent_id TEXT PRIMARY KEY,
                last_checked REAL NOT NULL
            );
            
            CREATE INDEX IF NOT EXISTS idx_subscriptions_ns ON subscriptions(namespace);
            "#,
        )?;
        Ok(())
    }
    
    /// Subscribe an agent to a namespace.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - The subscribing agent's ID
    /// * `namespace` - Namespace to watch ("*" for all)
    /// * `min_importance` - Minimum importance threshold (0.0-1.0)
    pub fn subscribe(
        &self,
        agent_id: &str,
        namespace: &str,
        min_importance: f64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let clamped = min_importance.max(0.0).min(1.0);
        
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO subscriptions (subscriber_id, namespace, min_importance, created_at)
            VALUES (?, ?, ?, ?)
            "#,
            params![
                agent_id,
                namespace,
                clamped,
                now_f64(),
            ],
        )?;
        
        Ok(())
    }
    
    /// Unsubscribe an agent from a namespace.
    pub fn unsubscribe(
        &self,
        agent_id: &str,
        namespace: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let affected = self.conn.execute(
            "DELETE FROM subscriptions WHERE subscriber_id = ? AND namespace = ?",
            params![agent_id, namespace],
        )?;
        
        Ok(affected > 0)
    }
    
    /// List all subscriptions for an agent.
    pub fn list_subscriptions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Subscription>, Box<dyn std::error::Error>> {
        let mut stmt = self.conn.prepare(
            "SELECT subscriber_id, namespace, min_importance, created_at FROM subscriptions WHERE subscriber_id = ?"
        )?;
        
        let rows = stmt.query_map(params![agent_id], |row| {
            let created_at_f64: f64 = row.get(3)?;
            Ok(Subscription {
                subscriber_id: row.get(0)?,
                namespace: row.get(1)?,
                min_importance: row.get(2)?,
                created_at: f64_to_datetime(created_at_f64),
            })
        })?;
        
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
    
    /// Helper to query notifications for a subscription.
    fn query_notifications_for_sub(
        &self,
        sub: &Subscription,
        since: Option<&DateTime<Utc>>,
    ) -> Result<Vec<Notification>, Box<dyn std::error::Error>> {
        let mut notifications = Vec::new();
        
        // Build query based on wildcard vs specific namespace
        if sub.namespace == "*" {
            // All namespaces
            if let Some(since_dt) = since {
                let mut stmt = self.conn.prepare(
                    "SELECT id, namespace, content, importance, created_at FROM memories 
                     WHERE created_at > ? AND importance >= ?"
                )?;
                
                let rows = stmt.query_map(params![datetime_to_f64(since_dt), sub.min_importance], |row| {
                    let created_at_f64: f64 = row.get(4)?;
                    Ok(Notification {
                        memory_id: row.get(0)?,
                        namespace: row.get(1)?,
                        content: row.get(2)?,
                        importance: row.get(3)?,
                        created_at: f64_to_datetime(created_at_f64),
                        subscription_namespace: sub.namespace.clone(),
                        threshold: sub.min_importance,
                    })
                })?;
                
                for row in rows {
                    if let Ok(notif) = row {
                        notifications.push(notif);
                    }
                }
            } else {
                let mut stmt = self.conn.prepare(
                    "SELECT id, namespace, content, importance, created_at FROM memories 
                     WHERE importance >= ?"
                )?;
                
                let rows = stmt.query_map(params![sub.min_importance], |row| {
                    let created_at_f64: f64 = row.get(4)?;
                    Ok(Notification {
                        memory_id: row.get(0)?,
                        namespace: row.get(1)?,
                        content: row.get(2)?,
                        importance: row.get(3)?,
                        created_at: f64_to_datetime(created_at_f64),
                        subscription_namespace: sub.namespace.clone(),
                        threshold: sub.min_importance,
                    })
                })?;
                
                for row in rows {
                    if let Ok(notif) = row {
                        notifications.push(notif);
                    }
                }
            }
        } else {
            // Specific namespace
            if let Some(since_dt) = since {
                let mut stmt = self.conn.prepare(
                    "SELECT id, namespace, content, importance, created_at FROM memories 
                     WHERE created_at > ? AND importance >= ? AND namespace = ?"
                )?;
                
                let rows = stmt.query_map(
                    params![datetime_to_f64(since_dt), sub.min_importance, &sub.namespace],
                    |row| {
                        let created_at_f64: f64 = row.get(4)?;
                        Ok(Notification {
                            memory_id: row.get(0)?,
                            namespace: row.get(1)?,
                            content: row.get(2)?,
                            importance: row.get(3)?,
                            created_at: f64_to_datetime(created_at_f64),
                            subscription_namespace: sub.namespace.clone(),
                            threshold: sub.min_importance,
                        })
                    }
                )?;
                
                for row in rows {
                    if let Ok(notif) = row {
                        notifications.push(notif);
                    }
                }
            } else {
                let mut stmt = self.conn.prepare(
                    "SELECT id, namespace, content, importance, created_at FROM memories 
                     WHERE importance >= ? AND namespace = ?"
                )?;
                
                let rows = stmt.query_map(
                    params![sub.min_importance, &sub.namespace],
                    |row| {
                        let created_at_f64: f64 = row.get(4)?;
                        Ok(Notification {
                            memory_id: row.get(0)?,
                            namespace: row.get(1)?,
                            content: row.get(2)?,
                            importance: row.get(3)?,
                            created_at: f64_to_datetime(created_at_f64),
                            subscription_namespace: sub.namespace.clone(),
                            threshold: sub.min_importance,
                        })
                    }
                )?;
                
                for row in rows {
                    if let Ok(notif) = row {
                        notifications.push(notif);
                    }
                }
            }
        }
        
        Ok(notifications)
    }
    
    /// Check for notifications since last check.
    ///
    /// Returns new memories that exceed the subscription thresholds.
    /// Updates the cursor so the same notifications aren't returned twice.
    pub fn check_notifications(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Notification>, Box<dyn std::error::Error>> {
        // Get last checked timestamp
        let last_checked: Option<f64> = self.conn
            .query_row(
                "SELECT last_checked FROM notification_cursor WHERE agent_id = ?",
                params![agent_id],
                |row| row.get(0),
            )
            .ok();
        
        let last_checked_dt = last_checked.map(f64_to_datetime);
        
        // Get agent's subscriptions
        let subscriptions = self.list_subscriptions(agent_id)?;
        
        if subscriptions.is_empty() {
            return Ok(vec![]);
        }
        
        let mut notifications = Vec::new();
        
        for sub in &subscriptions {
            let sub_notifs = self.query_notifications_for_sub(sub, last_checked_dt.as_ref())?;
            notifications.extend(sub_notifs);
        }
        
        // Update cursor
        self.conn.execute(
            "INSERT OR REPLACE INTO notification_cursor (agent_id, last_checked) VALUES (?, ?)",
            params![agent_id, now_f64()],
        )?;
        
        // Deduplicate by memory_id (in case multiple subscriptions match same memory)
        notifications.sort_by(|a, b| a.memory_id.cmp(&b.memory_id));
        notifications.dedup_by(|a, b| a.memory_id == b.memory_id);
        
        // Sort by created_at descending
        notifications.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        
        Ok(notifications)
    }
    
    /// Peek at notifications without updating cursor.
    pub fn peek_notifications(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Notification>, Box<dyn std::error::Error>> {
        // Get last checked timestamp
        let last_checked: Option<f64> = self.conn
            .query_row(
                "SELECT last_checked FROM notification_cursor WHERE agent_id = ?",
                params![agent_id],
                |row| row.get(0),
            )
            .ok();
        
        let last_checked_dt = last_checked.map(f64_to_datetime);
        
        let subscriptions = self.list_subscriptions(agent_id)?;
        
        if subscriptions.is_empty() {
            return Ok(vec![]);
        }
        
        let mut notifications = Vec::new();
        
        for sub in &subscriptions {
            let sub_notifs = self.query_notifications_for_sub(sub, last_checked_dt.as_ref())?;
            notifications.extend(sub_notifs);
        }
        
        notifications.sort_by(|a, b| a.memory_id.cmp(&b.memory_id));
        notifications.dedup_by(|a, b| a.memory_id == b.memory_id);
        notifications.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        
        Ok(notifications)
    }
    
    /// Reset notification cursor (useful for testing or re-checking everything).
    pub fn reset_cursor(&self, agent_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.conn.execute(
            "DELETE FROM notification_cursor WHERE agent_id = ?",
            params![agent_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        
        // Create memories table
        conn.execute_batch(
            r#"
            CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                memory_type TEXT NOT NULL,
                layer TEXT NOT NULL,
                created_at REAL NOT NULL,
                working_strength REAL NOT NULL DEFAULT 1.0,
                core_strength REAL NOT NULL DEFAULT 0.0,
                importance REAL NOT NULL DEFAULT 0.3,
                pinned INTEGER NOT NULL DEFAULT 0,
                consolidation_count INTEGER NOT NULL DEFAULT 0,
                last_consolidated REAL,
                source TEXT DEFAULT '',
                contradicts TEXT DEFAULT '',
                contradicted_by TEXT DEFAULT '',
                metadata TEXT,
                namespace TEXT NOT NULL DEFAULT 'default'
            );
            "#,
        ).unwrap();
        
        conn
    }
    
    #[test]
    fn test_subscribe_unsubscribe() {
        let conn = setup_test_db();
        let mgr = SubscriptionManager::new(&conn).unwrap();
        
        // Subscribe
        mgr.subscribe("ceo", "trading", 0.8).unwrap();
        
        let subs = mgr.list_subscriptions("ceo").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].namespace, "trading");
        assert!((subs[0].min_importance - 0.8).abs() < 0.01);
        
        // Unsubscribe
        let removed = mgr.unsubscribe("ceo", "trading").unwrap();
        assert!(removed);
        
        let subs = mgr.list_subscriptions("ceo").unwrap();
        assert!(subs.is_empty());
    }
    
    #[test]
    fn test_subscribe_wildcard() {
        let conn = setup_test_db();
        let mgr = SubscriptionManager::new(&conn).unwrap();
        
        mgr.subscribe("ceo", "*", 0.9).unwrap();
        
        let subs = mgr.list_subscriptions("ceo").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].namespace, "*");
    }
    
    #[test]
    fn test_notifications_basic() {
        let conn = setup_test_db();
        let mgr = SubscriptionManager::new(&conn).unwrap();
        
        // Subscribe to trading namespace with threshold 0.7
        mgr.subscribe("ceo", "trading", 0.7).unwrap();
        
        // Add a high-importance memory
        conn.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, importance, namespace)
             VALUES ('m1', 'Oil price spike', 'factual', 'working', strftime('%s','now'), 0.9, 'trading')",
            [],
        ).unwrap();
        
        // Check notifications
        let notifs = mgr.check_notifications("ceo").unwrap();
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].memory_id, "m1");
        assert_eq!(notifs[0].namespace, "trading");
        
        // Check again - should be empty (cursor updated)
        let notifs = mgr.check_notifications("ceo").unwrap();
        assert!(notifs.is_empty());
    }
    
    #[test]
    fn test_notifications_threshold() {
        let conn = setup_test_db();
        let mgr = SubscriptionManager::new(&conn).unwrap();
        
        mgr.subscribe("ceo", "trading", 0.8).unwrap();
        
        // Add low-importance memory
        conn.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, importance, namespace)
             VALUES ('m1', 'Minor update', 'factual', 'working', strftime('%s','now'), 0.3, 'trading')",
            [],
        ).unwrap();
        
        // Should not trigger notification
        let notifs = mgr.check_notifications("ceo").unwrap();
        assert!(notifs.is_empty());
    }
    
    #[test]
    fn test_notifications_wildcard() {
        let conn = setup_test_db();
        let mgr = SubscriptionManager::new(&conn).unwrap();
        
        // Subscribe to all namespaces
        mgr.subscribe("ceo", "*", 0.8).unwrap();
        
        // Add memories to different namespaces
        conn.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, importance, namespace)
             VALUES ('m1', 'Trading alert', 'factual', 'working', strftime('%s','now'), 0.9, 'trading')",
            [],
        ).unwrap();
        
        conn.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, importance, namespace)
             VALUES ('m2', 'Engine alert', 'factual', 'working', strftime('%s','now'), 0.85, 'engine')",
            [],
        ).unwrap();
        
        let notifs = mgr.check_notifications("ceo").unwrap();
        assert_eq!(notifs.len(), 2);
    }
    
    #[test]
    fn test_peek_notifications() {
        let conn = setup_test_db();
        let mgr = SubscriptionManager::new(&conn).unwrap();
        
        mgr.subscribe("ceo", "trading", 0.7).unwrap();
        
        conn.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, importance, namespace)
             VALUES ('m1', 'Test', 'factual', 'working', strftime('%s','now'), 0.9, 'trading')",
            [],
        ).unwrap();
        
        // Peek should not update cursor
        let notifs = mgr.peek_notifications("ceo").unwrap();
        assert_eq!(notifs.len(), 1);
        
        // Peek again - should still return same results
        let notifs = mgr.peek_notifications("ceo").unwrap();
        assert_eq!(notifs.len(), 1);
        
        // Now check (updates cursor)
        let notifs = mgr.check_notifications("ceo").unwrap();
        assert_eq!(notifs.len(), 1);
        
        // Check again - empty
        let notifs = mgr.check_notifications("ceo").unwrap();
        assert!(notifs.is_empty());
    }
}
