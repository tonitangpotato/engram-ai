/**
 * Subscription and Notification Model for Cross-Agent Intelligence
 */

import Database from 'better-sqlite3';

/**
 * A notification about a new memory that exceeded a subscription threshold
 */
export interface Notification {
  /** Memory ID */
  memoryId: string;
  /** Namespace the memory was stored in */
  namespace: string;
  /** Memory content (for convenience) */
  content: string;
  /** Memory importance */
  importance: number;
  /** When the memory was created */
  createdAt: number;
  /** The subscription that triggered this notification */
  subscriptionNamespace: string;
  /** The threshold that was exceeded */
  threshold: number;
}

/**
 * A subscription entry
 */
export interface Subscription {
  /** Agent ID of the subscriber */
  subscriberId: string;
  /** Namespace to watch ("*" = all namespaces) */
  namespace: string;
  /** Minimum importance to trigger notification */
  minImportance: number;
  /** When this subscription was created */
  createdAt: number;
}

/**
 * Manages subscriptions and notifications
 */
export class SubscriptionManager {
  constructor(private db: Database.Database) {
    this.initTables();
  }

  private initTables(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS subscriptions (
        subscriber_id TEXT NOT NULL,
        namespace TEXT NOT NULL,
        min_importance REAL NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (subscriber_id, namespace)
      );
      
      CREATE TABLE IF NOT EXISTS notification_cursor (
        agent_id TEXT PRIMARY KEY,
        last_checked INTEGER NOT NULL
      );
      
      CREATE INDEX IF NOT EXISTS idx_subscriptions_ns ON subscriptions(namespace);
    `);
  }

  /**
   * Subscribe an agent to a namespace
   */
  subscribe(agentId: string, namespace: string, minImportance: number): void {
    const clamped = Math.max(0, Math.min(1, minImportance));
    const stmt = this.db.prepare(`
      INSERT OR REPLACE INTO subscriptions (subscriber_id, namespace, min_importance, created_at)
      VALUES (?, ?, ?, ?)
    `);
    
    stmt.run(agentId, namespace, clamped, Date.now() / 1000);
  }

  /**
   * Unsubscribe an agent from a namespace
   */
  unsubscribe(agentId: string, namespace: string): boolean {
    const stmt = this.db.prepare(
      'DELETE FROM subscriptions WHERE subscriber_id = ? AND namespace = ?'
    );
    const result = stmt.run(agentId, namespace);
    return result.changes > 0;
  }

  /**
   * List all subscriptions for an agent
   */
  listSubscriptions(agentId: string): Subscription[] {
    const stmt = this.db.prepare(
      'SELECT subscriber_id, namespace, min_importance, created_at FROM subscriptions WHERE subscriber_id = ?'
    );
    
    const rows = stmt.all(agentId) as Array<{
      subscriber_id: string;
      namespace: string;
      min_importance: number;
      created_at: number;
    }>;
    
    return rows.map(row => ({
      subscriberId: row.subscriber_id,
      namespace: row.namespace,
      minImportance: row.min_importance,
      createdAt: row.created_at,
    }));
  }

  /**
   * Query notifications for a subscription
   */
  private queryNotificationsForSub(
    sub: Subscription,
    since?: number,
  ): Notification[] {
    const notifications: Notification[] = [];

    if (sub.namespace === '*') {
      // All namespaces
      const stmt = since
        ? this.db.prepare(
            'SELECT id, namespace, content, importance, created_at FROM memories WHERE created_at > ? AND importance >= ?'
          )
        : this.db.prepare(
            'SELECT id, namespace, content, importance, created_at FROM memories WHERE importance >= ?'
          );

      const params = since ? [since, sub.minImportance] : [sub.minImportance];
      const rows = stmt.all(...params) as Array<{
        id: string;
        namespace: string;
        content: string;
        importance: number;
        created_at: number;
      }>;

      for (const row of rows) {
        notifications.push({
          memoryId: row.id,
          namespace: row.namespace,
          content: row.content,
          importance: row.importance,
          createdAt: row.created_at,
          subscriptionNamespace: sub.namespace,
          threshold: sub.minImportance,
        });
      }
    } else {
      // Specific namespace
      const stmt = since
        ? this.db.prepare(
            'SELECT id, namespace, content, importance, created_at FROM memories WHERE created_at > ? AND importance >= ? AND namespace = ?'
          )
        : this.db.prepare(
            'SELECT id, namespace, content, importance, created_at FROM memories WHERE importance >= ? AND namespace = ?'
          );

      const params = since
        ? [since, sub.minImportance, sub.namespace]
        : [sub.minImportance, sub.namespace];
      const rows = stmt.all(...params) as Array<{
        id: string;
        namespace: string;
        content: string;
        importance: number;
        created_at: number;
      }>;

      for (const row of rows) {
        notifications.push({
          memoryId: row.id,
          namespace: row.namespace,
          content: row.content,
          importance: row.importance,
          createdAt: row.created_at,
          subscriptionNamespace: sub.namespace,
          threshold: sub.minImportance,
        });
      }
    }

    return notifications;
  }

  /**
   * Check for notifications since last check
   * Updates the cursor so the same notifications aren't returned twice
   */
  checkNotifications(agentId: string): Notification[] {
    // Get last checked timestamp
    const cursorStmt = this.db.prepare(
      'SELECT last_checked FROM notification_cursor WHERE agent_id = ?'
    );
    const cursorRow = cursorStmt.get(agentId) as { last_checked: number } | undefined;
    const lastChecked = cursorRow?.last_checked;

    // Get agent's subscriptions
    const subscriptions = this.listSubscriptions(agentId);

    if (subscriptions.length === 0) {
      return [];
    }

    const notifications: Notification[] = [];
    const now = Date.now() / 1000;

    for (const sub of subscriptions) {
      const subNotifs = this.queryNotificationsForSub(sub, lastChecked);
      notifications.push(...subNotifs);
    }

    // Update cursor
    const updateStmt = this.db.prepare(
      'INSERT OR REPLACE INTO notification_cursor (agent_id, last_checked) VALUES (?, ?)'
    );
    updateStmt.run(agentId, now);

    // Deduplicate by memory_id
    const seen = new Set<string>();
    const unique = notifications.filter(n => {
      if (seen.has(n.memoryId)) return false;
      seen.add(n.memoryId);
      return true;
    });

    // Sort by created_at descending
    unique.sort((a, b) => b.createdAt - a.createdAt);

    return unique;
  }

  /**
   * Peek at notifications without updating cursor
   */
  peekNotifications(agentId: string): Notification[] {
    const cursorStmt = this.db.prepare(
      'SELECT last_checked FROM notification_cursor WHERE agent_id = ?'
    );
    const cursorRow = cursorStmt.get(agentId) as { last_checked: number } | undefined;
    const lastChecked = cursorRow?.last_checked;

    const subscriptions = this.listSubscriptions(agentId);

    if (subscriptions.length === 0) {
      return [];
    }

    const notifications: Notification[] = [];

    for (const sub of subscriptions) {
      const subNotifs = this.queryNotificationsForSub(sub, lastChecked);
      notifications.push(...subNotifs);
    }

    // Deduplicate
    const seen = new Set<string>();
    const unique = notifications.filter(n => {
      if (seen.has(n.memoryId)) return false;
      seen.add(n.memoryId);
      return true;
    });

    unique.sort((a, b) => b.createdAt - a.createdAt);

    return unique;
  }

  /**
   * Reset notification cursor (useful for testing or re-checking everything)
   */
  resetCursor(agentId: string): void {
    const stmt = this.db.prepare('DELETE FROM notification_cursor WHERE agent_id = ?');
    stmt.run(agentId);
  }
}
