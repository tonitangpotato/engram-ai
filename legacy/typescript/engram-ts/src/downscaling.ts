/**
 * Synaptic Downscaling — Homeostatic Plasticity
 * (Tononi & Cirelli, 2003)
 */

import { SQLiteStore } from './store';

export function synapticDownscale(
  store: SQLiteStore,
  factor: number = 0.95,
): { n_scaled: number; avg_before: number; avg_after: number } {
  if (factor <= 0 || factor > 1) throw new Error(`Factor must be in (0, 1], got ${factor}`);

  const memories = store.all();
  if (memories.length === 0) return { n_scaled: 0, avg_before: 0, avg_after: 0 };

  let totalBefore = 0;
  let totalAfter = 0;
  let nScaled = 0;

  for (const entry of memories) {
    if (entry.pinned) continue;

    const before = entry.workingStrength + entry.coreStrength;
    totalBefore += before;

    entry.workingStrength *= factor;
    entry.coreStrength *= factor;

    const after = entry.workingStrength + entry.coreStrength;
    totalAfter += after;
    nScaled++;

    store.update(entry);
  }

  return {
    n_scaled: nScaled,
    avg_before: nScaled > 0 ? totalBefore / nScaled : 0,
    avg_after: nScaled > 0 ? totalAfter / nScaled : 0,
  };
}
