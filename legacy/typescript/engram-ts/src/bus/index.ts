/**
 * Emotional Bus — Connects Engram to agent workspace files
 * 
 * The Emotional Bus creates closed-loop feedback between:
 * - Memory emotions → SOUL updates (drive evolution)
 * - SOUL drives → Memory importance (what matters)
 * - Behavior outcomes → HEARTBEAT adjustments (adaptive behavior)
 */

import Database from 'better-sqlite3';
import {
  Drive,
  HeartbeatTask,
  Identity,
  readSoul,
  readHeartbeat,
  readIdentity,
  updateSoulField,
  addSoulDrive,
  updateHeartbeatTask,
  addHeartbeatTask,
} from './mod_io';
import {
  EmotionalAccumulator,
  EmotionalTrend,
  NEGATIVE_THRESHOLD,
  MIN_EVENTS_FOR_SUGGESTION,
  needsSoulUpdate,
} from './accumulator';
import {
  calculateImportanceBoost,
  scoreAlignment,
  findAlignedDrives,
} from './alignment';
import {
  BehaviorFeedback,
  ActionStats,
  shouldDeprioritize,
} from './feedback';

/**
 * A suggested update to SOUL.md based on emotional trends
 */
export interface SoulUpdate {
  /** The domain/topic this update relates to */
  domain: string;
  /** Suggested action (e.g., "add drive", "modify drive", "note pattern") */
  action: string;
  /** Suggested content */
  content: string;
  /** The emotional trend that triggered this suggestion */
  trend: EmotionalTrend;
}

/**
 * A suggested update to HEARTBEAT.md based on behavior feedback
 */
export interface HeartbeatUpdate {
  /** The action this update relates to */
  action: string;
  /** Suggested change (e.g., "deprioritize", "boost", "remove") */
  suggestion: string;
  /** The behavior stats that triggered this suggestion */
  stats: ActionStats;
}

/**
 * The Emotional Bus — main interface for emotional feedback loops
 */
export class EmotionalBus {
  private workspaceDir: string;
  private drives: Drive[];
  private accumulator: EmotionalAccumulator;
  private feedback: BehaviorFeedback;

  constructor(workspaceDir: string, db: Database.Database) {
    this.workspaceDir = workspaceDir;
    this.accumulator = new EmotionalAccumulator(db);
    this.feedback = new BehaviorFeedback(db);
    this.drives = readSoul(workspaceDir);
  }

  /**
   * Reload drives from SOUL.md
   */
  reloadDrives(): void {
    this.drives = readSoul(this.workspaceDir);
  }

  /**
   * Get the current drives
   */
  getDrives(): Drive[] {
    return this.drives;
  }

  /**
   * Process an interaction with emotional content
   * This is the main entry point for the emotional feedback loop
   */
  processInteraction(content: string, emotion: number, domain: string): void {
    // Record emotion in accumulator
    this.accumulator.recordEmotion(domain, emotion);
  }

  /**
   * Calculate importance boost for a memory based on drive alignment
   */
  alignImportance(content: string): number {
    return calculateImportanceBoost(content, this.drives);
  }

  /**
   * Score how well content aligns with drives
   */
  alignmentScore(content: string): number {
    return scoreAlignment(content, this.drives);
  }

  /**
   * Find which drives a piece of content aligns with
   */
  findAligned(content: string): Array<[string, number]> {
    return findAlignedDrives(content, this.drives);
  }

  /**
   * Log a behavior outcome
   */
  logBehavior(action: string, positive: boolean): void {
    this.feedback.logOutcome(action, positive);
  }

  /**
   * Get emotional trends
   */
  getTrends(): EmotionalTrend[] {
    return this.accumulator.getAllTrends();
  }

  /**
   * Get behavior statistics
   */
  getBehaviorStats(): ActionStats[] {
    return this.feedback.getAllActionStats();
  }

  /**
   * Suggest SOUL updates based on accumulated emotional trends
   */
  suggestSoulUpdates(): SoulUpdate[] {
    const trendsNeedingUpdate = this.accumulator.getTrendsNeedingUpdate();
    const suggestions: SoulUpdate[] = [];

    for (const trend of trendsNeedingUpdate) {
      if (trend.valence < -0.7) {
        suggestions.push({
          domain: trend.domain,
          action: 'add drive',
          content: `Avoid ${trend.domain} approaches that consistently lead to negative outcomes`,
          trend,
        });
      } else if (trend.valence < NEGATIVE_THRESHOLD) {
        suggestions.push({
          domain: trend.domain,
          action: 'note pattern',
          content: `Be cautious with ${trend.domain} - showing signs of friction (${trend.valence.toFixed(
            2
          )} avg over ${trend.count} events)`,
          trend,
        });
      }
    }

    // Also suggest reinforcing very positive trends
    const allTrends = this.accumulator.getAllTrends();
    for (const trend of allTrends) {
      if (trend.count >= MIN_EVENTS_FOR_SUGGESTION && trend.valence > 0.7) {
        suggestions.push({
          domain: trend.domain,
          action: 'reinforce',
          content: `Continue ${trend.domain} - consistently positive outcomes (${trend.valence.toFixed(
            2
          )} avg over ${trend.count} events)`,
          trend,
        });
      }
    }

    return suggestions;
  }

  /**
   * Suggest HEARTBEAT updates based on behavior feedback
   */
  suggestHeartbeatUpdates(): HeartbeatUpdate[] {
    const suggestions: HeartbeatUpdate[] = [];

    // Actions to deprioritize
    for (const stats of this.feedback.getActionsToDeprioritize()) {
      suggestions.push({
        action: stats.action,
        suggestion: 'deprioritize',
        stats,
      });
    }

    // Actions doing well (suggest boosting)
    for (const stats of this.feedback.getSuccessfulActions(0.8)) {
      suggestions.push({
        action: stats.action,
        suggestion: 'boost',
        stats,
      });
    }

    return suggestions;
  }

  /**
   * Get the current identity from workspace
   */
  getIdentity(): Identity {
    return readIdentity(this.workspaceDir);
  }

  /**
   * Get heartbeat tasks from workspace
   */
  getHeartbeatTasks(): HeartbeatTask[] {
    return readHeartbeat(this.workspaceDir);
  }

  /**
   * Update a SOUL field
   */
  updateSoul(key: string, value: string): boolean {
    return updateSoulField(this.workspaceDir, key, value);
  }

  /**
   * Add a new drive to SOUL
   */
  addSoulDrive(key: string, value: string): void {
    addSoulDrive(this.workspaceDir, key, value);
  }

  /**
   * Update a heartbeat task completion status
   */
  updateHeartbeatTask(task: string, completed: boolean): boolean {
    return updateHeartbeatTask(this.workspaceDir, task, completed);
  }

  /**
   * Add a new heartbeat task
   */
  addHeartbeatTask(description: string): void {
    addHeartbeatTask(this.workspaceDir, description);
  }
}

// Re-export all types and functions
export * from './mod_io';
export * from './accumulator';
export * from './alignment';
export * from './feedback';
