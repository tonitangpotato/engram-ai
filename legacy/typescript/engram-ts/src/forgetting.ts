/**
 * Forgetting Model — Ebbinghaus + Interference
 *
 * R(t) = e^(-t/S)
 * S grows with spaced repetition and importance.
 */

import { MemoryEntry, MemoryType, MemoryLayer, DEFAULT_DECAY_RATES } from './core';
import { SQLiteStore } from './store';

export function computeStability(entry: MemoryEntry): number {
  const baseDecay = DEFAULT_DECAY_RATES[entry.memoryType] ?? 0.05;
  const baseS = 1.0 / baseDecay;

  const nAccesses = entry.accessTimes.length;
  const spacingFactor = 1.0 + 0.5 * Math.log1p(nAccesses);

  const importanceFactor = 0.5 + entry.importance;

  const consolidationFactor = 1.0 + 0.2 * entry.consolidationCount;

  return baseS * spacingFactor * importanceFactor * consolidationFactor;
}

export function retrievability(entry: MemoryEntry, now?: number): number {
  now = now ?? Date.now() / 1000;

  const lastAccess = entry.accessTimes.length > 0
    ? Math.max(...entry.accessTimes)
    : entry.createdAt;
  const tDays = (now - lastAccess) / 86400;

  if (tDays <= 0) return 1.0;

  const S = computeStability(entry);
  return Math.exp(-tDays / S);
}

export function effectiveStrength(entry: MemoryEntry, now?: number): number {
  const R = retrievability(entry, now);
  const traceStrength = entry.workingStrength + entry.coreStrength;
  return traceStrength * R;
}

export function shouldForget(entry: MemoryEntry, threshold: number = 0.01, now?: number): boolean {
  if (entry.pinned) return false;
  return effectiveStrength(entry, now) < threshold;
}

export function pruneforgotten(store: SQLiteStore, threshold: number = 0.01, now?: number): MemoryEntry[] {
  const pruned: MemoryEntry[] = [];
  for (const entry of store.all()) {
    if (shouldForget(entry, threshold, now)) {
      if (entry.layer !== MemoryLayer.L4_ARCHIVE) {
        entry.layer = MemoryLayer.L4_ARCHIVE;
        store.update(entry);
        pruned.push(entry);
      }
    }
  }
  return pruned;
}

export function retrievalInducedForgetting(
  store: SQLiteStore,
  retrievedEntry: MemoryEntry,
  suppressionFactor: number = 0.05,
): void {
  const retrievedWords = new Set(retrievedEntry.content.toLowerCase().split(/\s+/));

  for (const entry of store.all()) {
    if (entry.id === retrievedEntry.id) continue;
    if (entry.memoryType !== retrievedEntry.memoryType) continue;

    const entryWords = new Set(entry.content.toLowerCase().split(/\s+/));
    if (entryWords.size === 0) continue;

    let overlapCount = 0;
    for (const w of retrievedWords) {
      if (entryWords.has(w)) overlapCount++;
    }
    const overlap = overlapCount / entryWords.size;

    if (overlap > 0.3) {
      entry.workingStrength *= (1 - suppressionFactor * overlap);
      store.update(entry);
    }
  }
}
