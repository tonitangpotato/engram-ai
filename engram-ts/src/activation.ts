/**
 * ACT-R Activation-Based Retrieval
 *
 * A_i = B_i + Σ(W_j · S_ji) + ε
 * B_i = ln(Σ_k t_k^(-d))
 */

import { MemoryEntry } from './core';
import { SQLiteStore } from './store';

export function baseLevelActivation(
  entry: MemoryEntry,
  now?: number,
  decay: number = 0.5,
): number {
  now = now ?? Date.now() / 1000;

  if (entry.accessTimes.length === 0) return -Infinity;

  let total = 0.0;
  for (const tk of entry.accessTimes) {
    let age = now - tk;
    if (age <= 0) age = 0.001;
    total += Math.pow(age, -decay);
  }

  if (total <= 0) return -Infinity;
  return Math.log(total);
}

export function spreadingActivation(
  entry: MemoryEntry,
  contextKeywords: string[],
  weight: number = 1.0,
): number {
  if (contextKeywords.length === 0) return 0.0;

  const contentLower = entry.content.toLowerCase();
  let matches = 0;
  for (const kw of contextKeywords) {
    if (contentLower.includes(kw.toLowerCase())) matches++;
  }

  return weight * (matches / contextKeywords.length);
}

export function retrievalActivation(
  entry: MemoryEntry,
  contextKeywords?: string[],
  now?: number,
  baseDecay: number = 0.5,
  contextWeight: number = 1.5,
  importanceWeight: number = 0.5,
): number {
  const base = baseLevelActivation(entry, now, baseDecay);
  if (base === -Infinity) return -Infinity;

  let context = 0.0;
  if (contextKeywords && contextKeywords.length > 0) {
    context = spreadingActivation(entry, contextKeywords, contextWeight);
  }

  const importanceBoost = entry.importance * importanceWeight;
  return base + context + importanceBoost;
}

export function retrieveTopK(
  store: SQLiteStore,
  contextKeywords?: string[],
  k: number = 5,
  now?: number,
  minActivation: number = -10.0,
): Array<[MemoryEntry, number]> {
  const scored: Array<[MemoryEntry, number]> = [];

  for (const entry of store.all()) {
    const score = retrievalActivation(entry, contextKeywords, now);
    if (score > minActivation) {
      scored.push([entry, score]);
    }
  }

  scored.sort((a, b) => b[1] - a[1]);
  return scored.slice(0, k);
}
