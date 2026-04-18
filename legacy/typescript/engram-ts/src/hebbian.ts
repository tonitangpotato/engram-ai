/**
 * Hebbian Learning — Co-activation forms memory links
 *
 * "Neurons that fire together, wire together."
 *
 * When memories are recalled together repeatedly, they form Hebbian links.
 * These links create an associative network independent of explicit entity
 * tagging — purely emergent from usage patterns.
 *
 * Key insight: This captures implicit relationships that the agent discovers
 * through experience, not explicit knowledge stored at encoding time.
 */

import { SQLiteStore } from './store';
import { MemoryConfig } from './config';

/**
 * Record co-activation for a set of memory IDs.
 *
 * When multiple memories are retrieved together (e.g., in a single recall),
 * each pair gets their coactivation_count incremented. When the count
 * reaches the threshold, a Hebbian link is automatically formed.
 *
 * @param store - The SQLiteStore instance
 * @param memoryIds - List of memory IDs that were co-activated
 * @param config - Memory configuration containing Hebbian parameters
 * @returns List of [id1, id2] tuples for newly formed links
 */
export function recordCoactivation(
  store: SQLiteStore,
  memoryIds: string[],
  config: MemoryConfig,
): Array<[string, string]> {
  if (!config.hebbianEnabled || memoryIds.length < 2) {
    return [];
  }

  // Pre-fetch created_at timestamps if STDP is enabled
  const timestamps: Map<string, number> = new Map();
  if (config.stdpEnabled) {
    for (const mid of memoryIds) {
      const row = store.db.prepare(
        'SELECT created_at FROM memories WHERE id=?'
      ).get(mid) as { created_at: number } | undefined;
      if (row) {
        timestamps.set(mid, row.created_at);
      }
    }
  }

  const newLinks: Array<[string, string]> = [];

  // Generate all pairs
  for (let i = 0; i < memoryIds.length; i++) {
    for (let j = i + 1; j < memoryIds.length; j++) {
      let id1 = memoryIds[i];
      let id2 = memoryIds[j];

      // Ensure consistent ordering (smaller ID first)
      if (id1 > id2) {
        [id1, id2] = [id2, id1];
      }

      const formed = maybeCreateLink(store, id1, id2, config.hebbianThreshold);
      if (formed) {
        newLinks.push([id1, id2]);
      }

      // STDP: track temporal ordering
      if (config.stdpEnabled && timestamps.has(id1) && timestamps.has(id2)) {
        updateTemporalCounts(store, id1, id2, timestamps.get(id1)!, timestamps.get(id2)!);
      }
    }
  }

  return newLinks;
}

/**
 * Update temporal forward/backward counts for a Hebbian link pair.
 *
 * With consistent ordering (id1 < id2):
 * - temporal_forward: id1 was created before id2 (id1 → id2 direction)
 * - temporal_backward: id2 was created before id1 (id2 → id1 direction)
 */
function updateTemporalCounts(
  store: SQLiteStore,
  id1: string,
  id2: string,
  ts1: number,
  ts2: number,
): void {
  if (ts1 === ts2) return; // Simultaneous — no temporal signal

  // Check if the row exists
  const existing = store.db.prepare(
    'SELECT temporal_forward, temporal_backward FROM hebbian_links WHERE source_id=? AND target_id=?'
  ).get(id1, id2) as { temporal_forward: number; temporal_backward: number } | undefined;

  if (!existing) return;

  if (ts1 < ts2) {
    // id1 was created before id2 → forward direction
    store.db.prepare(
      'UPDATE hebbian_links SET temporal_forward = temporal_forward + 1 WHERE source_id=? AND target_id=?'
    ).run(id1, id2);
  } else {
    // id2 was created before id1 → backward direction
    store.db.prepare(
      'UPDATE hebbian_links SET temporal_backward = temporal_backward + 1 WHERE source_id=? AND target_id=?'
    ).run(id1, id2);
  }
}

/**
 * Increment coactivation count and create link if threshold is met.
 *
 * Uses upsert to atomically increment the counter. When threshold is
 * reached for the first time, creates the bidirectional link.
 *
 * @param store - The SQLiteStore instance
 * @param id1 - First memory ID (should be <= id2 for consistency)
 * @param id2 - Second memory ID
 * @param threshold - Activation count needed to form link
 * @returns True if a new link was formed on this call
 */
export function maybeCreateLink(
  store: SQLiteStore,
  id1: string,
  id2: string,
  threshold: number = 3,
): boolean {
  // Ensure consistent ordering
  if (id1 > id2) {
    [id1, id2] = [id2, id1];
  }

  // Check if link already exists
  const existing = store.getHebbianLink(id1, id2);

  if (existing && existing.strength > 0) {
    // Link already exists, just increment coactivation count
    store.upsertHebbianLink(id1, id2, existing.strength, existing.coactivationCount + 1);
    return false;
  }

  if (existing) {
    // Record exists but strength=0 (tracking phase), increment count
    const newCount = existing.coactivationCount + 1;
    if (newCount >= threshold) {
      // Threshold reached! Create bidirectional link
      store.upsertHebbianLink(id1, id2, 1.0, newCount);
      store.upsertHebbianLink(id2, id1, 1.0, newCount);
      return true;
    } else {
      store.upsertHebbianLink(id1, id2, 0.0, newCount);
      return false;
    }
  } else {
    // First co-activation, create tracking record with strength=0
    store.upsertHebbianLink(id1, id2, 0.0, 1);
    return false;
  }
}

/**
 * Get all memories linked to this one via Hebbian connections.
 *
 * Only returns neighbors with positive link strength (formed links,
 * not just tracked co-activations).
 *
 * @param store - The SQLiteStore instance
 * @param memoryId - Memory ID to find neighbors for
 * @returns List of connected memory IDs
 */
export function getHebbianNeighbors(store: SQLiteStore, memoryId: string): string[] {
  return store.getHebbianNeighbors(memoryId);
}

/**
 * Decay all Hebbian link strengths by a factor.
 *
 * Called during consolidation to gradually weaken unused links.
 * Links that decay below a threshold (0.1) are removed.
 *
 * @param store - The SQLiteStore instance
 * @param factor - Multiplicative decay factor (0.95 = 5% decay)
 * @returns Number of links pruned
 */
export function decayHebbianLinks(store: SQLiteStore, factor: number = 0.95): number {
  return store.decayHebbianLinks(factor);
}

/**
 * Strengthen an existing Hebbian link.
 *
 * Called when linked memories are accessed together again.
 * Caps strength at 2.0 to prevent unbounded growth.
 *
 * @param store - The SQLiteStore instance
 * @param id1 - First memory ID
 * @param id2 - Second memory ID
 * @param boost - Amount to add to strength
 * @returns True if link existed and was strengthened
 */
export function strengthenLink(
  store: SQLiteStore,
  id1: string,
  id2: string,
  boost: number = 0.1,
): boolean {
  // Update both directions
  let updated = false;

  for (const [src, tgt] of [[id1, id2], [id2, id1]]) {
    const existing = store.getHebbianLink(src, tgt);
    if (existing && existing.strength > 0) {
      const newStrength = Math.min(2.0, existing.strength + boost);
      store.upsertHebbianLink(src, tgt, newStrength, existing.coactivationCount);
      updated = true;
    }
  }

  return updated;
}

/**
 * Get all formed Hebbian links (strength > 0).
 *
 * @param store - The SQLiteStore instance
 * @returns List of { sourceId, targetId, strength } objects
 */
export function getAllHebbianLinks(
  store: SQLiteStore,
): Array<{ sourceId: string; targetId: string; strength: number }> {
  return store.getAllHebbianLinks();
}

/**
 * STDP candidate for causal inference.
 */
export interface StdpCandidate {
  sourceId: string;
  targetId: string;
  strength: number;
  temporalForward: number;
  temporalBackward: number;
  direction: 'forward' | 'backward';
  confidence: number;
  causeId: string;
  effectId: string;
}

/**
 * Find Hebbian links with strong temporal signal for causal inference.
 *
 * Returns links where temporal_forward > temporal_backward * causalThreshold
 * or vice versa, indicating a consistent temporal ordering (potential causation).
 */
export function getStdpCandidates(
  store: SQLiteStore,
  causalThreshold: number = 2.0,
  minObservations: number = 3,
): StdpCandidate[] {
  const rows = store.db.prepare(`
    SELECT source_id, target_id, strength, temporal_forward, temporal_backward
    FROM hebbian_links
    WHERE strength > 0
      AND (temporal_forward + temporal_backward) >= ?
  `).all(minObservations) as Array<{
    source_id: string;
    target_id: string;
    strength: number;
    temporal_forward: number;
    temporal_backward: number;
  }>;

  const candidates: StdpCandidate[] = [];

  for (const r of rows) {
    const fwd = r.temporal_forward ?? 0;
    const bwd = r.temporal_backward ?? 0;
    const total = fwd + bwd;

    if (total < minObservations) continue;

    if (bwd === 0 && fwd >= minObservations) {
      candidates.push({
        sourceId: r.source_id, targetId: r.target_id,
        strength: r.strength, temporalForward: fwd, temporalBackward: bwd,
        direction: 'forward', confidence: 1.0,
        causeId: r.source_id, effectId: r.target_id,
      });
    } else if (fwd === 0 && bwd >= minObservations) {
      candidates.push({
        sourceId: r.source_id, targetId: r.target_id,
        strength: r.strength, temporalForward: fwd, temporalBackward: bwd,
        direction: 'backward', confidence: 1.0,
        causeId: r.target_id, effectId: r.source_id,
      });
    } else if (fwd > bwd * causalThreshold) {
      candidates.push({
        sourceId: r.source_id, targetId: r.target_id,
        strength: r.strength, temporalForward: fwd, temporalBackward: bwd,
        direction: 'forward', confidence: parseFloat((fwd / total).toFixed(3)),
        causeId: r.source_id, effectId: r.target_id,
      });
    } else if (bwd > fwd * causalThreshold) {
      candidates.push({
        sourceId: r.source_id, targetId: r.target_id,
        strength: r.strength, temporalForward: fwd, temporalBackward: bwd,
        direction: 'backward', confidence: parseFloat((bwd / total).toFixed(3)),
        causeId: r.target_id, effectId: r.source_id,
      });
    }
  }

  return candidates;
}

/**
 * Update the direction field of a Hebbian link.
 */
export function updateLinkDirection(
  store: SQLiteStore,
  sourceId: string,
  targetId: string,
  direction: string,
): void {
  store.db.prepare(
    'UPDATE hebbian_links SET direction = ? WHERE source_id = ? AND target_id = ?'
  ).run(direction, sourceId, targetId);
}
