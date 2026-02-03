/**
 * Reward-Modulated Learning — Dopaminergic Feedback Signals
 */

import { MemoryEntry } from './core';
import { SQLiteStore } from './store';

const POSITIVE_SIGNALS = [
  '好的', '不错', '对', '对的', '很好', '棒', '可以', '行',
  'good', 'nice', 'correct', 'yes', 'right', 'exactly', 'perfect',
  'great', 'thanks', 'thank you', 'awesome', 'love it', 'well done',
];

const NEGATIVE_SIGNALS = [
  '不对', '别这样', '错', '错了', '不行', '不好', '停', '别',
  'wrong', 'no', "don't", 'stop', 'bad', 'incorrect', 'nope',
  "that's wrong", 'not right', 'undo', 'cancel',
];

export function detectFeedback(text: string): [string, number] {
  const textLower = text.toLowerCase().trim();

  let posMatches = 0;
  for (const s of POSITIVE_SIGNALS) {
    if (textLower.includes(s.toLowerCase())) posMatches++;
  }

  let negMatches = 0;
  for (const s of NEGATIVE_SIGNALS) {
    if (textLower.includes(s.toLowerCase())) negMatches++;
  }

  if (posMatches === 0 && negMatches === 0) {
    return ['neutral', 0.0];
  }

  if (posMatches > negMatches) {
    const confidence = Math.min(0.95, 0.3 + 0.2 * posMatches);
    return ['positive', confidence];
  } else if (negMatches > posMatches) {
    const confidence = Math.min(0.95, 0.3 + 0.2 * negMatches);
    return ['negative', confidence];
  } else {
    return ['neutral', 0.1];
  }
}

export function applyReward(
  store: SQLiteStore,
  feedbackPolarity: string,
  recentN: number = 3,
  rewardMagnitude: number = 0.15,
): void {
  if (feedbackPolarity !== 'positive' && feedbackPolarity !== 'negative') return;

  const allMemories = store.all();
  if (allMemories.length === 0) return;

  const lastAccess = (m: MemoryEntry): number =>
    m.accessTimes.length > 0 ? Math.max(...m.accessTimes) : m.createdAt;

  const sorted = [...allMemories].sort((a, b) => lastAccess(b) - lastAccess(a));
  const targets = sorted.slice(0, recentN);

  for (let i = 0; i < targets.length; i++) {
    const entry = targets[i];
    const discount = 1.0 / (1.0 + 0.5 * i);

    if (feedbackPolarity === 'positive') {
      entry.importance = Math.min(1.0, entry.importance + rewardMagnitude * discount);
      entry.workingStrength += 0.05 * discount;
    } else {
      entry.importance = Math.max(0.0, entry.importance - rewardMagnitude * discount);
      entry.workingStrength *= (1.0 - 0.1 * discount);
    }

    store.update(entry);
  }
}
