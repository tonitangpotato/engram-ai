/**
 * Emotional Accumulator — Track emotional valence trends per domain
 */

import Database from 'better-sqlite3';

/** Threshold for negative valence to trigger SOUL update suggestion */
export const NEGATIVE_THRESHOLD = -0.5;
/** Minimum event count before suggesting SOUL updates */
export const MIN_EVENTS_FOR_SUGGESTION = 10;

/**
 * Emotional trend for a domain
 */
export interface EmotionalTrend {
  /** Domain name (e.g., "coding", "communication", "research") */
  domain: string;
  /** Running average valence (-1.0 to 1.0) */
  valence: number;
  /** Number of emotional events recorded */
  count: number;
  /** Last update timestamp */
  lastUpdated: number;
}

/**
 * Check if this trend suggests a need for SOUL update
 */
export function needsSoulUpdate(trend: EmotionalTrend): boolean {
  return trend.count >= MIN_EVENTS_FOR_SUGGESTION && trend.valence < NEGATIVE_THRESHOLD;
}

/**
 * Describe the trend in human-readable terms
 */
export function describeTrend(trend: EmotionalTrend): string {
  const sentiment = trend.valence > 0.3 ? 'positive' : trend.valence < -0.3 ? 'negative' : 'neutral';
  return `${trend.domain}: ${sentiment} trend (${trend.valence.toFixed(2)} avg over ${trend.count} events)`;
}

/**
 * Emotional accumulator that tracks valence trends per domain
 */
export class EmotionalAccumulator {
  constructor(private db: Database.Database) {
    this.ensureTable();
  }

  private ensureTable(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS emotional_trends (
        domain TEXT PRIMARY KEY,
        valence REAL NOT NULL DEFAULT 0.0,
        count INTEGER NOT NULL DEFAULT 0,
        last_updated INTEGER NOT NULL
      );
    `);
  }

  /**
   * Record an emotional event for a domain
   * Updates the running average valence for the domain
   * Valence should be in range -1.0 (very negative) to 1.0 (very positive)
   */
  recordEmotion(domain: string, valence: number): void {
    // Clamp valence to valid range
    const clamped = Math.max(-1.0, Math.min(1.0, valence));
    const now = Date.now() / 1000;

    // Try to get existing trend
    const stmt = this.db.prepare('SELECT valence, count FROM emotional_trends WHERE domain = ?');
    const row = stmt.get(domain) as { valence: number; count: number } | undefined;

    if (row) {
      // Update running average: new_avg = (old_avg * count + new_value) / (count + 1)
      const newCount = row.count + 1;
      const newValence = (row.valence * row.count + clamped) / newCount;

      const updateStmt = this.db.prepare(
        'UPDATE emotional_trends SET valence = ?, count = ?, last_updated = ? WHERE domain = ?'
      );
      updateStmt.run(newValence, newCount, now, domain);
    } else {
      // Insert new trend
      const insertStmt = this.db.prepare(
        'INSERT INTO emotional_trends (domain, valence, count, last_updated) VALUES (?, ?, 1, ?)'
      );
      insertStmt.run(domain, clamped, now);
    }
  }

  /**
   * Get the emotional trend for a specific domain
   */
  getTrend(domain: string): EmotionalTrend | null {
    const stmt = this.db.prepare(
      'SELECT domain, valence, count, last_updated FROM emotional_trends WHERE domain = ?'
    );
    const row = stmt.get(domain) as
      | { domain: string; valence: number; count: number; last_updated: number }
      | undefined;

    if (!row) return null;

    return {
      domain: row.domain,
      valence: row.valence,
      count: row.count,
      lastUpdated: row.last_updated,
    };
  }

  /**
   * Get all emotional trends
   */
  getAllTrends(): EmotionalTrend[] {
    const stmt = this.db.prepare(
      'SELECT domain, valence, count, last_updated FROM emotional_trends ORDER BY count DESC'
    );

    const rows = stmt.all() as Array<{
      domain: string;
      valence: number;
      count: number;
      last_updated: number;
    }>;

    return rows.map(row => ({
      domain: row.domain,
      valence: row.valence,
      count: row.count,
      lastUpdated: row.last_updated,
    }));
  }

  /**
   * Get all trends that suggest SOUL updates
   */
  getTrendsNeedingUpdate(): EmotionalTrend[] {
    return this.getAllTrends().filter(needsSoulUpdate);
  }

  /**
   * Reset a domain's trend (after SOUL has been updated)
   */
  resetTrend(domain: string): void {
    const stmt = this.db.prepare('DELETE FROM emotional_trends WHERE domain = ?');
    stmt.run(domain);
  }

  /**
   * Decay all trends by a factor (used during consolidation)
   * This moves trends toward neutral over time
   */
  decayTrends(factor: number): number {
    const now = Date.now() / 1000;
    const stmt = this.db.prepare(
      'UPDATE emotional_trends SET valence = valence * ?, last_updated = ?'
    );
    const result = stmt.run(factor, now);
    return result.changes;
  }
}
