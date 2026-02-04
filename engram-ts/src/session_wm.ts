/**
 * Session Working Memory — Cognitive science-based session-level memory management.
 *
 * Based on:
 * - Miller's Law: 7±2 chunks capacity
 * - Baddeley's Working Memory Model: 5-minute decay
 * - ACT-R: Spreading activation from context
 * - Hebbian Learning: Associative neighborhood checking
 *
 * The core insight: Instead of deciding "should I recall?" per message,
 * maintain a working memory state and only recall when the topic changes.
 */

import type { Memory } from './memory';
import { effectiveStrength } from './forgetting';
import { confidenceLabel } from './confidence';
import { getHebbianNeighbors } from './hebbian';

export interface WorkingMemoryItem {
  id: string;
  activatedAt: number;
}

export interface SessionRecallResult {
  results: Array<{
    id: string;
    content: string;
    type: string;
    confidence: number;
    confidence_label: string;
    strength: number;
    age_days: number;
    from_working_memory: boolean;
  }>;
  fullRecallTriggered: boolean;
  workingMemorySize: number;
  reason: 'empty_wm' | 'topic_change' | 'topic_continuous';
}

/**
 * Simulates cognitive science working memory — limited capacity + time decay.
 *
 * Used to intelligently decide when to trigger full memory recall vs.
 * reusing already-active memories. Reduces API calls by 70-80% for
 * continuous conversation topics.
 */
export class SessionWorkingMemory {
  capacity: number;
  decaySeconds: number;
  private items: Map<string, number>; // memory_id -> last_activated timestamp

  /**
   * Initialize working memory.
   *
   * @param capacity Maximum number of active memory chunks (Miller's Law: 7±2)
   * @param decaySeconds Time until a memory fades from working memory (default: 5 min)
   */
  constructor(capacity: number = 7, decaySeconds: number = 300.0) {
    this.capacity = capacity;
    this.decaySeconds = decaySeconds;
    this.items = new Map();
  }

  /**
   * Activate memories (bring into working memory).
   *
   * Called after a recall — the retrieved memories become active in WM.
   * If capacity is exceeded, oldest items are pruned.
   */
  activate(memoryIds: string[]): void {
    const now = Date.now() / 1000;
    for (const mid of memoryIds) {
      this.items.set(mid, now);
    }
    this.prune();
  }

  /**
   * Prune working memory:
   * 1. Remove decayed items (older than decaySeconds)
   * 2. If still over capacity, keep only the most recent
   */
  private prune(): void {
    const now = Date.now() / 1000;

    // Time decay — remove items older than decaySeconds
    for (const [k, v] of this.items) {
      if (now - v >= this.decaySeconds) {
        this.items.delete(k);
      }
    }

    // Capacity limit — keep only the most recently activated
    if (this.items.size > this.capacity) {
      const sorted = [...this.items.entries()].sort((a, b) => b[1] - a[1]);
      this.items = new Map(sorted.slice(0, this.capacity));
    }
  }

  /**
   * Get currently active memory IDs (after pruning).
   */
  getActiveIds(): string[] {
    this.prune();
    return [...this.items.keys()];
  }

  /**
   * Get full memory objects for active working memory items.
   */
  getActiveMemories(memory: Memory): Array<{
    id: string;
    content: string;
    type: string;
    confidence: number;
    confidence_label: string;
    strength: number;
    age_days: number;
    layer: string;
    importance: number;
    pinned: boolean;
    from_wm: boolean;
  }> {
    this.prune();
    const results: Array<{
      id: string;
      content: string;
      type: string;
      confidence: number;
      confidence_label: string;
      strength: number;
      age_days: number;
      layer: string;
      importance: number;
      pinned: boolean;
      from_wm: boolean;
    }> = [];

    const now = Date.now() / 1000;

    for (const mid of this.items.keys()) {
      const entry = memory._store.get(mid);
      if (entry) {
        const strength = effectiveStrength(entry, now);
        const conf = Math.min(1.0, strength * 1.2);

        results.push({
          id: entry.id,
          content: entry.content,
          type: entry.memoryType,
          confidence: Math.round(conf * 1000) / 1000,
          confidence_label: confidenceLabel(conf),
          strength: Math.round(strength * 1000) / 1000,
          age_days: Math.round(entry.ageDays() * 10) / 10,
          layer: entry.layer,
          importance: Math.round(entry.importance * 100) / 100,
          pinned: entry.pinned,
          from_wm: true,
        });
      }
    }

    return results;
  }

  /**
   * Determine if a full recall is needed, or if working memory suffices.
   *
   * Logic:
   * 1. If working memory is empty → need recall
   * 2. Do a lightweight probe (limit=3 cheap recall)
   * 3. Check if probe results overlap with:
   *    - Current working memory IDs
   *    - Hebbian neighbors of working memory IDs
   * 4. If ≥60% overlap → topic is continuous, skip full recall
   * 5. Otherwise → topic changed, do full recall
   */
  needsRecall(message: string, memory: Memory): boolean {
    this.prune();

    // Empty working memory → always recall
    if (this.items.size === 0) {
      return true;
    }

    const currentIds = new Set(this.items.keys());

    // Collect Hebbian neighbors of current working memory
    const neighbors = new Set<string>();
    for (const mid of currentIds) {
      const links = getHebbianNeighbors(memory._store, mid);
      for (const targetId of links) {
        neighbors.add(targetId);
      }
    }

    // Lightweight probe — just 3 results to check topic continuity
    const probe = memory.recall(message, { limit: 3, graphExpand: false });
    if (probe.length === 0) {
      return true; // No results → need full recall
    }

    const probeIds = new Set(probe.map(r => r.id));

    // Check overlap with current WM + Hebbian neighborhood
    let overlapCount = 0;
    for (const pid of probeIds) {
      if (currentIds.has(pid) || neighbors.has(pid)) {
        overlapCount++;
      }
    }

    const overlapRatio = overlapCount / probeIds.size;

    // ≥60% overlap → topic is continuous
    if (overlapRatio >= 0.6) {
      return false;
    }

    return true;
  }

  /**
   * Check if working memory is currently empty (after pruning).
   */
  isEmpty(): boolean {
    this.prune();
    return this.items.size === 0;
  }

  /**
   * Get current working memory size (after pruning).
   */
  size(): number {
    this.prune();
    return this.items.size;
  }

  /**
   * Clear all items from working memory.
   */
  clear(): void {
    this.items.clear();
  }

  toString(): string {
    this.prune();
    return `SessionWorkingMemory(size=${this.items.size}/${this.capacity})`;
  }
}

// Session registry for per-session working memory state
const sessionRegistry = new Map<string, SessionWorkingMemory>();

/**
 * Get or create a SessionWorkingMemory for a given session ID.
 */
export function getSessionWM(sessionId: string = 'default'): SessionWorkingMemory {
  if (!sessionRegistry.has(sessionId)) {
    sessionRegistry.set(sessionId, new SessionWorkingMemory());
  }
  return sessionRegistry.get(sessionId)!;
}

/**
 * Clear and remove a session's working memory.
 */
export function clearSession(sessionId: string): boolean {
  if (sessionRegistry.has(sessionId)) {
    sessionRegistry.delete(sessionId);
    return true;
  }
  return false;
}

/**
 * List all active session IDs.
 */
export function listSessions(): string[] {
  return [...sessionRegistry.keys()];
}
