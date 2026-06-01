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
    /// Phase D flag (T29.1): when true, notification queries read from the
    /// unified substrate (`nodes` table filtered to `node_kind='memory'`)
    /// instead of the legacy `memories` table. Subscription metadata itself
    /// (the `subscriptions` table) is unaffected — only memory-side reads
    /// are switched. Mirrors `MemoryConfig::unified_substrate`.
    /// See `.gid/features/v04-unified-substrate/design.md` §5.4, §8.5.
    unified_substrate: bool,
}

impl<'a> SubscriptionManager<'a> {
    /// Create a new SubscriptionManager, initializing tables if needed.
    ///
    /// `unified_substrate` mirrors `MemoryConfig::unified_substrate` (T28).
    /// When `false` (default for v0.3 deployments), notification queries
    /// read from the legacy `memories` table. When `true`, they read from
    /// the unified `nodes` table with `node_kind='memory'`. Both paths
    /// return bit-exactly equivalent `Notification` rows while Phase B
    /// dual-writes keep the two sides in sync.
    pub fn new(
        conn: &'a Connection,
        unified_substrate: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::init_tables(conn)?;
        Ok(Self {
            conn,
            unified_substrate,
        })
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
        let clamped = min_importance.clamp(0.0, 1.0);

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

        // T29.1: choose memory source based on Phase D `unified_substrate`
        // flag. Legacy reads `memories` directly. Unified reads `nodes`
        // filtered to `node_kind='memory'`. Field semantics are identical
        // (see `Storage::insert_memory_node_row`, the single source of
        // truth for the memory→node projection): `id`, `namespace`,
        // `content`, `importance`, `created_at` round-trip 1:1.
        //
        // Bug-for-bug compatible: the legacy query intentionally does NOT
        // filter `deleted_at IS NULL` or `superseded_by IS NULL`.
        // Subscriptions are designed for newly-stored memories where
        // these are always NULL. The unified branch matches that;
        // tightening the filter would be a behavior change, out of scope
        // for T29.1.
        let memories_source: &str = if self.unified_substrate {
            "nodes WHERE node_kind = 'memory' AND"
        } else {
            "memories WHERE"
        };

        // Build query based on wildcard vs specific namespace
        if sub.namespace == "*" {
            // All namespaces
            if let Some(since_dt) = since {
                let sql = format!(
                    "SELECT id, namespace, content, importance, created_at FROM {} \
                     created_at > ? AND importance >= ?",
                    memories_source,
                );
                let mut stmt = self.conn.prepare(&sql)?;

                let rows = stmt.query_map(
                    params![datetime_to_f64(since_dt), sub.min_importance],
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
                    },
                )?;

                for notif in rows.flatten() {
                    notifications.push(notif);
                }
            } else {
                let sql = format!(
                    "SELECT id, namespace, content, importance, created_at FROM {} \
                     importance >= ?",
                    memories_source,
                );
                let mut stmt = self.conn.prepare(&sql)?;

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

                for notif in rows.flatten() {
                    notifications.push(notif);
                }
            }
        } else {
            // Specific namespace
            if let Some(since_dt) = since {
                let sql = format!(
                    "SELECT id, namespace, content, importance, created_at FROM {} \
                     created_at > ? AND importance >= ? AND namespace = ?",
                    memories_source,
                );
                let mut stmt = self.conn.prepare(&sql)?;

                let rows = stmt.query_map(
                    params![
                        datetime_to_f64(since_dt),
                        sub.min_importance,
                        &sub.namespace
                    ],
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
                    },
                )?;

                for notif in rows.flatten() {
                    notifications.push(notif);
                }
            } else {
                let sql = format!(
                    "SELECT id, namespace, content, importance, created_at FROM {} \
                     importance >= ? AND namespace = ?",
                    memories_source,
                );
                let mut stmt = self.conn.prepare(&sql)?;

                let rows = stmt.query_map(params![sub.min_importance, &sub.namespace], |row| {
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

                for notif in rows.flatten() {
                    notifications.push(notif);
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
        let last_checked: Option<f64> = self
            .conn
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
        let last_checked: Option<f64> = self
            .conn
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
        )
        .unwrap();

        conn
    }

    #[test]
    fn test_subscribe_unsubscribe() {
        let conn = setup_test_db();
        let mgr = SubscriptionManager::new(&conn, false).unwrap();

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
        let mgr = SubscriptionManager::new(&conn, false).unwrap();

        mgr.subscribe("ceo", "*", 0.9).unwrap();

        let subs = mgr.list_subscriptions("ceo").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].namespace, "*");
    }

    #[test]
    fn test_notifications_basic() {
        let conn = setup_test_db();
        let mgr = SubscriptionManager::new(&conn, false).unwrap();

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
        let mgr = SubscriptionManager::new(&conn, false).unwrap();

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
        let mgr = SubscriptionManager::new(&conn, false).unwrap();

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
        let mgr = SubscriptionManager::new(&conn, false).unwrap();

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

    // ───────────────────────────────────────────────────────────────────
    // T29.1 contract tests — Phase D unified-substrate read-switch.
    //
    // These tests pin the contract that with Phase B dual-writes in
    // place, the legacy and unified read paths return bit-identical
    // notifications. If a future refactor breaks this equivalence
    // (e.g. by changing which columns are read, or by tightening
    // WHERE clauses on only one side), these tests will catch it.
    // ───────────────────────────────────────────────────────────────────

    /// Setup DB with BOTH legacy `memories` and unified `nodes` tables.
    /// Mirrors steady-state Phase B + Phase D conditions where dual-writes
    /// keep both sides in sync.
    fn setup_test_db_unified() -> Connection {
        let conn = setup_test_db();
        // Minimal `nodes` schema covering only the columns this module
        // reads (real schema in storage.rs:374 has many more). Including
        // `node_kind` is mandatory because `nodes` is shared across
        // memory / entity / topic rows.
        conn.execute_batch(
            r#"
            CREATE TABLE nodes (
                id          TEXT PRIMARY KEY,
                node_kind   TEXT NOT NULL,
                namespace   TEXT NOT NULL DEFAULT 'default',
                content     TEXT NOT NULL,
                importance  REAL NOT NULL DEFAULT 0.3,
                created_at  REAL NOT NULL
            );
            "#,
        )
        .unwrap();
        conn
    }

    /// Insert one memory row into BOTH legacy `memories` and unified
    /// `nodes`. Mirrors the Phase B dual-write contract.
    fn insert_memory_dual(
        conn: &Connection,
        id: &str,
        namespace: &str,
        content: &str,
        importance: f64,
        created_at: f64,
    ) {
        conn.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, \
             importance, namespace) VALUES (?, ?, 'episodic', 'working', ?, ?, ?)",
            params![id, content, created_at, importance, namespace],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, content, importance, created_at) \
             VALUES (?, 'memory', ?, ?, ?, ?)",
            params![id, namespace, content, importance, created_at],
        )
        .unwrap();
        // Note: also insert into nodes with node_kind='entity' or 'topic'
        // would NOT be picked up because the unified query filters by
        // node_kind='memory'. The `test_unified_path_ignores_non_memory_nodes`
        // case below exercises that boundary.
    }

    /// Assert two notification lists are set-equivalent after sorting by
    /// memory_id (SQLite ORDER is unspecified without ORDER BY).
    fn assert_notifications_eq(legacy: &[Notification], unified: &[Notification]) {
        assert_eq!(
            legacy.len(),
            unified.len(),
            "row count mismatch: legacy={} unified={}",
            legacy.len(),
            unified.len(),
        );
        let mut l = legacy.to_vec();
        let mut u = unified.to_vec();
        l.sort_by(|a, b| a.memory_id.cmp(&b.memory_id));
        u.sort_by(|a, b| a.memory_id.cmp(&b.memory_id));
        for (la, ua) in l.iter().zip(u.iter()) {
            assert_eq!(la.memory_id, ua.memory_id, "memory_id");
            assert_eq!(la.namespace, ua.namespace, "namespace");
            assert_eq!(la.content, ua.content, "content");
            assert!(
                (la.importance - ua.importance).abs() < f64::EPSILON,
                "importance: {} vs {}",
                la.importance,
                ua.importance,
            );
            assert_eq!(la.created_at, ua.created_at, "created_at");
            assert_eq!(
                la.subscription_namespace, ua.subscription_namespace,
                "sub_ns"
            );
            assert!(
                (la.threshold - ua.threshold).abs() < f64::EPSILON,
                "threshold mismatch",
            );
        }
    }

    /// Exercises all four query branches (wildcard×since, wildcard×nosince,
    /// specific×since, specific×nosince) under both flag settings,
    /// asserting equivalence on each.
    #[test]
    fn test_t29_1_unified_path_matches_legacy_all_branches() {
        let conn = setup_test_db_unified();

        // Subscribe under both flag settings — same data, same subscription.
        let mgr_legacy = SubscriptionManager::new(&conn, false).unwrap();
        let mgr_unified = SubscriptionManager::new(&conn, true).unwrap();

        // Seed: 4 memories across 2 namespaces, varying importance + time.
        // ts is seconds since epoch; tests use small integers to keep
        // intent obvious.
        insert_memory_dual(&conn, "m1", "trading", "buy signal alpha", 0.9, 100.0);
        insert_memory_dual(&conn, "m2", "trading", "noise", 0.4, 200.0);
        insert_memory_dual(&conn, "m3", "ops", "deploy started", 0.95, 300.0);
        insert_memory_dual(&conn, "m4", "ops", "log noise", 0.2, 400.0);

        // Branch A: specific namespace, no `since` filter.
        let sub_specific = Subscription {
            subscriber_id: "agent".into(),
            namespace: "trading".into(),
            min_importance: 0.5,
            created_at: Utc::now(),
        };
        let l = mgr_legacy
            .query_notifications_for_sub(&sub_specific, None)
            .unwrap();
        let u = mgr_unified
            .query_notifications_for_sub(&sub_specific, None)
            .unwrap();
        assert_eq!(
            l.len(),
            1,
            "specific+nosince: only m1 exceeds 0.5 in trading"
        );
        assert_notifications_eq(&l, &u);

        // Branch B: specific namespace, WITH `since` filter (cuts off m1@100).
        let since = f64_to_datetime(150.0);
        let l = mgr_legacy
            .query_notifications_for_sub(&sub_specific, Some(&since))
            .unwrap();
        let u = mgr_unified
            .query_notifications_for_sub(&sub_specific, Some(&since))
            .unwrap();
        assert_eq!(
            l.len(),
            0,
            "specific+since=150: m1@100 cut off, m2 below threshold"
        );
        assert_notifications_eq(&l, &u);

        // Branch C: wildcard namespace, no `since`.
        let sub_wild = Subscription {
            subscriber_id: "ceo".into(),
            namespace: "*".into(),
            min_importance: 0.8,
            created_at: Utc::now(),
        };
        let l = mgr_legacy
            .query_notifications_for_sub(&sub_wild, None)
            .unwrap();
        let u = mgr_unified
            .query_notifications_for_sub(&sub_wild, None)
            .unwrap();
        assert_eq!(
            l.len(),
            2,
            "wildcard+nosince: m1 (0.9) and m3 (0.95) exceed 0.8"
        );
        assert_notifications_eq(&l, &u);

        // Branch D: wildcard namespace, WITH `since`.
        let since = f64_to_datetime(250.0);
        let l = mgr_legacy
            .query_notifications_for_sub(&sub_wild, Some(&since))
            .unwrap();
        let u = mgr_unified
            .query_notifications_for_sub(&sub_wild, Some(&since))
            .unwrap();
        assert_eq!(
            l.len(),
            1,
            "wildcard+since=250: only m3 (0.95@300) survives both filters"
        );
        assert_notifications_eq(&l, &u);
    }

    /// Sanity check: the unified path filters on `node_kind='memory'`, so
    /// a non-memory row in `nodes` (e.g. an entity or topic written by
    /// resolution pipeline / KC) must NOT appear in notifications.
    /// Pins the WHERE clause so a future SQL refactor that drops the
    /// node_kind filter would fail loudly.
    #[test]
    fn test_t29_1_unified_path_ignores_non_memory_nodes() {
        let conn = setup_test_db_unified();
        let mgr_unified = SubscriptionManager::new(&conn, true).unwrap();

        // Insert a memory and a non-memory (entity) row with the SAME
        // namespace + importance >= subscription threshold. Only the
        // memory should surface.
        insert_memory_dual(&conn, "mem1", "trading", "real signal", 0.9, 100.0);
        conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, content, importance, created_at) \
             VALUES (?, 'entity', ?, ?, ?, ?)",
            params!["ent1", "trading", "person:Alice", 0.95_f64, 110.0_f64],
        )
        .unwrap();

        let sub = Subscription {
            subscriber_id: "agent".into(),
            namespace: "trading".into(),
            min_importance: 0.5,
            created_at: Utc::now(),
        };
        let notifs = mgr_unified.query_notifications_for_sub(&sub, None).unwrap();
        assert_eq!(
            notifs.len(),
            1,
            "only mem1 should surface — entity row is filtered by node_kind"
        );
        assert_eq!(notifs[0].memory_id, "mem1");
    }

    /// Empty-result equivalence: both paths must return zero rows when
    /// no memory matches. Guards against silent "row count differs by
    /// 0 vs N" bugs from query shape divergence.
    #[test]
    fn test_t29_1_unified_path_empty_result_matches_legacy() {
        let conn = setup_test_db_unified();
        let mgr_legacy = SubscriptionManager::new(&conn, false).unwrap();
        let mgr_unified = SubscriptionManager::new(&conn, true).unwrap();

        // Only a low-importance memory; subscription wants high.
        insert_memory_dual(&conn, "low", "trading", "noise", 0.1, 100.0);

        let sub = Subscription {
            subscriber_id: "agent".into(),
            namespace: "trading".into(),
            min_importance: 0.9,
            created_at: Utc::now(),
        };
        let l = mgr_legacy.query_notifications_for_sub(&sub, None).unwrap();
        let u = mgr_unified.query_notifications_for_sub(&sub, None).unwrap();
        assert!(l.is_empty() && u.is_empty(), "both must be empty");
        assert_notifications_eq(&l, &u);
    }
}
