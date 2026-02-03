/**
 * Memory Chain Consolidation Model (Murre & Chessa, 2011)
 *
 * dr₁/dt = -μ₁ · r₁(t)
 * dr₂/dt = α · r₁(t) - μ₂ · r₂(t)
 */

import { MemoryEntry, MemoryLayer, MemoryType } from './core';
import { SQLiteStore } from './store';

const MU1 = 0.15;
const MU2 = 0.005;
const ALPHA = 0.08;

export function applyDecay(
  entry: MemoryEntry,
  dtDays: number,
  mu1: number = MU1,
  mu2: number = MU2,
): void {
  if (entry.pinned) return;
  entry.workingStrength *= Math.exp(-mu1 * dtDays);
  entry.coreStrength *= Math.exp(-mu2 * dtDays);
}

export function consolidateSingle(
  entry: MemoryEntry,
  dtDays: number = 1.0,
  alpha: number = ALPHA,
  mu1: number = MU1,
  mu2: number = MU2,
): void {
  if (entry.pinned) return;

  const effectiveAlpha = alpha * (0.2 + entry.importance ** 2);
  const transfer = effectiveAlpha * entry.workingStrength * dtDays;
  entry.coreStrength += transfer;

  applyDecay(entry, dtDays, mu1, mu2);

  entry.consolidationCount += 1;
  entry.lastConsolidated = Date.now() / 1000;
}

export function runConsolidationCycle(
  store: SQLiteStore,
  dtDays: number = 1.0,
  interleaveRatio: number = 0.3,
  alpha: number = ALPHA,
  mu1: number = MU1,
  mu2: number = MU2,
  replayBoost: number = 0.01,
  promoteThreshold: number = 0.25,
  demoteThreshold: number = 0.05,
  archiveThreshold: number = 0.15,
): void {
  const allMemories = store.all();

  // Step 1: Consolidate all L3 (working) memories
  const working = allMemories.filter(m => m.layer === MemoryLayer.L3_WORKING);
  for (const entry of working) {
    consolidateSingle(entry, dtDays, alpha, mu1, mu2);
    store.update(entry);
  }

  // Step 2: Interleaved replay of L4 (archive) memories
  const archive = allMemories.filter(m => m.layer === MemoryLayer.L4_ARCHIVE);
  if (archive.length > 0) {
    const nReplay = Math.max(1, Math.floor(archive.length * interleaveRatio));
    // Simple random sample
    const shuffled = [...archive].sort(() => Math.random() - 0.5);
    const replaySample = shuffled.slice(0, Math.min(nReplay, archive.length));
    for (const entry of replaySample) {
      entry.coreStrength += replayBoost * (0.5 + entry.importance);
      entry.consolidationCount += 1;
      entry.lastConsolidated = Date.now() / 1000;
      store.update(entry);
    }
  }

  // Step 3: Decay L2 (core) memories slightly
  const core = allMemories.filter(m => m.layer === MemoryLayer.L2_CORE);
  for (const entry of core) {
    applyDecay(entry, dtDays, 0, mu2); // No working decay for L2
    store.update(entry);
  }

  // Step 4: Layer promotion/demotion
  rebalanceLayers(store, promoteThreshold, demoteThreshold, archiveThreshold);
}

function rebalanceLayers(
  store: SQLiteStore,
  promoteThreshold: number = 0.25,
  demoteThreshold: number = 0.05,
  archiveThreshold: number = 0.15,
): void {
  for (const entry of store.all()) {
    const total = entry.workingStrength + entry.coreStrength;
    const oldLayer = entry.layer;

    if (entry.pinned) {
      entry.layer = MemoryLayer.L2_CORE;
    } else if (entry.layer === MemoryLayer.L3_WORKING) {
      if (entry.coreStrength >= promoteThreshold) {
        entry.layer = MemoryLayer.L2_CORE;
      } else if (entry.workingStrength < archiveThreshold && entry.coreStrength < archiveThreshold) {
        entry.layer = MemoryLayer.L4_ARCHIVE;
      }
    } else if (entry.layer === MemoryLayer.L2_CORE) {
      if (total < demoteThreshold && !entry.pinned) {
        entry.layer = MemoryLayer.L4_ARCHIVE;
      }
    }

    if (entry.layer !== oldLayer) {
      store.update(entry);
    }
  }
}

export function getConsolidationStats(store: SQLiteStore): {
  total_memories: number;
  layers: Record<string, { count: number; avg_working: number; avg_core: number; avg_importance: number }>;
  pinned: number;
} {
  const allMem = store.all();
  const byLayer: Record<string, { count: number; avg_working: number; avg_core: number; avg_importance: number }> = {};

  for (const layer of Object.values(MemoryLayer)) {
    const entries = allMem.filter(m => m.layer === layer);
    const count = entries.length;
    byLayer[layer] = {
      count,
      avg_working: count > 0 ? entries.reduce((s, m) => s + m.workingStrength, 0) / count : 0,
      avg_core: count > 0 ? entries.reduce((s, m) => s + m.coreStrength, 0) / count : 0,
      avg_importance: count > 0 ? entries.reduce((s, m) => s + m.importance, 0) / count : 0,
    };
  }

  return {
    total_memories: allMem.length,
    layers: byLayer,
    pinned: allMem.filter(m => m.pinned).length,
  };
}
