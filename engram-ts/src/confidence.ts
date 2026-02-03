/**
 * Confidence Scoring — Two-Dimensional Metacognitive Monitoring
 *
 * 1. Content reliability — how trustworthy (stable over time)
 * 2. Retrieval salience — how "top of mind" (decays with time)
 */

import { MemoryEntry } from './core';
import { SQLiteStore } from './store';
import { effectiveStrength } from './forgetting';

const DEFAULT_RELIABILITY: Record<string, number> = {
  factual: 0.85,
  episodic: 0.90,
  relational: 0.75,
  emotional: 0.95,
  procedural: 0.90,
  opinion: 0.60,
};

export function contentReliability(entry: MemoryEntry): number {
  let base = DEFAULT_RELIABILITY[entry.memoryType] ?? 0.7;

  if (entry.contradictedBy) {
    base *= 0.3;
  }

  if (entry.pinned) {
    base = Math.max(base, 0.95);
  }

  const importanceBoost = entry.importance * 0.1;
  return Math.min(1.0, base + importanceBoost);
}

export function retrievalSalience(
  entry: MemoryEntry,
  store?: SQLiteStore | null,
  now?: number,
): number {
  const eff = effectiveStrength(entry, now);

  if (store) {
    const allStrengths = store.all().map(m => effectiveStrength(m, now));
    const maxStrength = allStrengths.length > 0 ? Math.max(...allStrengths) : 1.0;
    if (maxStrength <= 0) return 0.0;
    const raw = eff / maxStrength;
    return Math.min(1.0, Math.max(0.0, raw));
  } else {
    let raw = 2.0 / (1.0 + Math.exp(-2.0 * eff)) - 1.0;
    raw = Math.max(0.0, raw);
    return Math.min(1.0, raw);
  }
}

export function confidenceScore(
  entry: MemoryEntry,
  store?: SQLiteStore | null,
  now?: number,
): number {
  const rel = contentReliability(entry);
  const sal = retrievalSalience(entry, store, now);
  return 0.7 * rel + 0.3 * sal;
}

export function confidenceLabel(score: number): string {
  if (score >= 0.8) return 'certain';
  if (score >= 0.6) return 'likely';
  if (score >= 0.4) return 'uncertain';
  return 'vague';
}

export function confidenceDetail(
  entry: MemoryEntry,
  store?: SQLiteStore | null,
  now?: number,
): {
  reliability: number;
  salience: number;
  combined: number;
  label: string;
  description: string;
} {
  const rel = contentReliability(entry);
  const sal = retrievalSalience(entry, store, now);
  const combined = 0.7 * rel + 0.3 * sal;
  const label = confidenceLabel(combined);

  let description: string;
  if (rel >= 0.8 && sal >= 0.7) {
    description = 'I clearly remember this';
  } else if (rel >= 0.8 && sal < 0.4) {
    description = "I have a reliable record of this, though it's from a while ago";
  } else if (rel < 0.6) {
    description = "I have a note about this but I'm not sure how accurate it is";
  } else {
    description = 'I recall this but the details might not be exact';
  }

  return {
    reliability: Math.round(rel * 1000) / 1000,
    salience: Math.round(sal * 1000) / 1000,
    combined: Math.round(combined * 1000) / 1000,
    label,
    description,
  };
}
