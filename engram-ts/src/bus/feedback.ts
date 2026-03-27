/**
 * Behavior Feedback — Track heartbeat action outcomes
 */

import Database from 'better-sqlite3';

/** Default window size for action scoring (recent N attempts) */
export const DEFAULT_SCORE_WINDOW = 20;
/** Threshold for low action score that triggers deprioritization */
export const LOW_SCORE_THRESHOLD = 0.2;
/** Minimum attempts before suggesting deprioritization */
export const MIN_ATTEMPTS_FOR_SUGGESTION = 10;

/**
 * A logged behavior outcome
 */
export interface BehaviorLog {
  /** Action name/description */
  action: string;
  /** Whether the outcome was positive (true) or negative (false) */
  outcome: boolean;
  /** When this outcome was recorded */
  timestamp: number;
}

/**
 * Statistics for an action
 */
export interface ActionStats {
  /** Action name */
  action: string;
  /** Total attempts */
  total: number;
  /** Positive outcomes */
  positive: number;
  /** Negative outcomes */
  negative: number;
  /** Success rate (positive / total) */
  score: number;
}

/**
 * Check if this action should be deprioritized
 */
export function shouldDeprioritize(stats: ActionStats): boolean {
  return stats.total >= MIN_ATTEMPTS_FOR_SUGGESTION && stats.score < LOW_SCORE_THRESHOLD;
}

/**
 * Describe the action performance in human-readable terms
 */
export function describeStats(stats: ActionStats): string {
  const rating =
    stats.score >= 0.8
      ? 'excellent'
      : stats.score >= 0.5
      ? 'moderate'
      : stats.score >= 0.2
      ? 'poor'
      : 'very poor';

  return `${stats.action}: ${rating} (${(stats.score * 100).toFixed(0)}% success rate, ${
    stats.positive
  }/${stats.total} positive)`;
}

/**
 * Behavior feedback tracker
 */
export class BehaviorFeedback {
  constructor(private db: Database.Database) {
    this.ensureTable();
  }

  private ensureTable(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS behavior_log (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        action TEXT NOT NULL,
        outcome INTEGER NOT NULL,
        timestamp INTEGER NOT NULL
      );
      
      CREATE INDEX IF NOT EXISTS idx_behavior_action ON behavior_log(action);
      CREATE INDEX IF NOT EXISTS idx_behavior_timestamp ON behavior_log(timestamp);
    `);
  }

  /**
   * Log an action outcome
   */
  logOutcome(action: string, positive: boolean): void {
    const now = Date.now() / 1000;
    const stmt = this.db.prepare(
      'INSERT INTO behavior_log (action, outcome, timestamp) VALUES (?, ?, ?)'
    );
    stmt.run(action, positive ? 1 : 0, now);
  }

  /**
   * Get the success score for an action
   * Returns the positive rate over the recent window of attempts
   */
  getActionScore(action: string, window: number = DEFAULT_SCORE_WINDOW): number | null {
    const stmt = this.db.prepare(
      'SELECT outcome FROM behavior_log WHERE action = ? ORDER BY timestamp DESC LIMIT ?'
    );

    const rows = stmt.all(action, window) as Array<{ outcome: number }>;

    if (rows.length === 0) return null;

    const positiveCount = rows.filter(r => r.outcome !== 0).length;
    return positiveCount / rows.length;
  }

  /**
   * Get full statistics for an action
   */
  getActionStats(action: string): ActionStats | null {
    const stmt = this.db.prepare('SELECT COUNT(*), SUM(outcome) FROM behavior_log WHERE action = ?');

    const row = stmt.get(action) as { 'COUNT(*)': number; 'SUM(outcome)': number | null } | undefined;

    if (!row || row['COUNT(*)'] === 0) return null;

    const total = row['COUNT(*)'];
    const positive = row['SUM(outcome)'] ?? 0;

    return {
      action,
      total,
      positive,
      negative: total - positive,
      score: positive / total,
    };
  }

  /**
   * Get all action statistics
   */
  getAllActionStats(): ActionStats[] {
    const stmt = this.db.prepare(
      'SELECT action, COUNT(*) as total, SUM(outcome) as positive FROM behavior_log GROUP BY action ORDER BY total DESC'
    );

    const rows = stmt.all() as Array<{ action: string; total: number; positive: number | null }>;

    return rows.map(row => {
      const total = row.total;
      const positive = row.positive ?? 0;
      return {
        action: row.action,
        total,
        positive,
        negative: total - positive,
        score: total > 0 ? positive / total : 0,
      };
    });
  }

  /**
   * Get actions that should be deprioritized
   */
  getActionsToDeprioritize(): ActionStats[] {
    return this.getAllActionStats().filter(shouldDeprioritize);
  }

  /**
   * Get actions with high success rate
   */
  getSuccessfulActions(minScore: number): ActionStats[] {
    return this.getAllActionStats().filter(
      a => a.total >= MIN_ATTEMPTS_FOR_SUGGESTION && a.score >= minScore
    );
  }

  /**
   * Get recent behavior logs for an action
   */
  getRecentLogs(action: string, limit: number): BehaviorLog[] {
    const stmt = this.db.prepare(
      'SELECT action, outcome, timestamp FROM behavior_log WHERE action = ? ORDER BY timestamp DESC LIMIT ?'
    );

    const rows = stmt.all(action, limit) as Array<{
      action: string;
      outcome: number;
      timestamp: number;
    }>;

    return rows.map(row => ({
      action: row.action,
      outcome: row.outcome !== 0,
      timestamp: row.timestamp,
    }));
  }

  /**
   * Clear all logs for an action (e.g., after adjusting HEARTBEAT)
   */
  clearAction(action: string): number {
    const stmt = this.db.prepare('DELETE FROM behavior_log WHERE action = ?');
    const result = stmt.run(action);
    return result.changes;
  }

  /**
   * Prune old logs (keep only recent N per action)
   */
  pruneOldLogs(keepPerAction: number): number {
    // SQLite approach using subquery
    const stmt = this.db.prepare(`
      DELETE FROM behavior_log WHERE id NOT IN (
        SELECT id FROM (
          SELECT id, ROW_NUMBER() OVER (PARTITION BY action ORDER BY timestamp DESC) as rn
          FROM behavior_log
        )
        WHERE rn <= ?
      )
    `);

    const result = stmt.run(keepPerAction);
    return result.changes;
  }
}
