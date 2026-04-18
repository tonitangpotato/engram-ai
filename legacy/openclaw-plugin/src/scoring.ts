/**
 * Engram Context Engine — Message Scoring & Selection
 *
 * The core intelligence: replaces FIFO windowing with cognitive scoring.
 *
 * Each message gets a composite score from:
 * 1. ACT-R base-level activation (recency + frequency decay)
 * 2. Hebbian association strength (semantic neighborhood)
 * 3. Importance weighting (user questions > acks)
 * 4. Role priority (system > user > assistant for context)
 * 5. Recency boost (recent messages get floor score to preserve coherence)
 */

export interface ScoredMessage {
  index: number;
  score: number;
  tokens: number;
  role: string;
}

/**
 * Estimate token count for a message.
 * Uses the ~4 chars/token heuristic (conservative for English,
 * closer to 2 chars/token for CJK — we lean conservative).
 */
export function estimateTokens(content: string): number {
  if (!content) return 4; // role overhead

  // CJK characters are roughly 1-2 tokens each
  const cjkCount = (content.match(/[\u4e00-\u9fff\u3040-\u309f\u30a0-\u30ff]/g) || []).length;
  const nonCjkLength = content.length - cjkCount;

  // CJK: ~1.5 tokens per char, Latin: ~0.25 tokens per char
  const estimate = Math.ceil(cjkCount * 1.5 + nonCjkLength / 4);
  return estimate + 4; // role + framing overhead
}

/**
 * Compute importance score for a message based on content signals.
 */
export function messageImportance(role: string, content: string): number {
  let score = 0.5; // baseline

  // Role weighting
  if (role === "system") score = 0.95; // always important
  if (role === "user") score = 0.7;
  if (role === "assistant") score = 0.5;

  // Content signals
  if (!content) return score;
  const lower = content.toLowerCase();

  // Questions are high-value context
  if (lower.includes("?") || lower.includes("？")) score += 0.15;

  // Code blocks suggest technical context
  if (lower.includes("```")) score += 0.1;

  // Tool use — important for continuity
  if (role === "assistant" && lower.includes("tool_use")) score += 0.2;
  if (role === "user" && lower.includes("tool_result")) score += 0.15;

  // Short acks are low value
  if (content.length < 20 && !lower.includes("?")) score -= 0.2;

  // HEARTBEAT_OK is minimal value
  if (lower.includes("heartbeat_ok")) score = 0.05;

  // NO_REPLY is minimal value
  if (lower.trim() === "no_reply") score = 0.05;

  return Math.max(0, Math.min(1, score));
}

/**
 * Score all messages and select the best subset within token budget.
 *
 * Strategy:
 * 1. Always include system messages
 * 2. Always include the last N messages (coherence floor)
 * 3. Score remaining messages by importance
 * 4. Greedily fill budget by score
 * 5. Re-sort by original order (preserve conversation flow)
 */
export function selectMessages(params: {
  messages: Array<{ role: string; content?: string }>;
  tokenBudget: number;
  recentFloor?: number; // always keep last N messages (default: 6)
  engramScores?: Map<number, number>; // index -> engram activation score
}): {
  selectedIndices: number[];
  totalTokens: number;
} {
  const { messages, tokenBudget, recentFloor = 6, engramScores } = params;

  if (messages.length === 0) {
    return { selectedIndices: [], totalTokens: 0 };
  }

  // Score and estimate tokens for each message
  const scored: ScoredMessage[] = messages.map((msg, i) => {
    const content =
      typeof msg.content === "string"
        ? msg.content
        : JSON.stringify(msg.content ?? "");
    const tokens = estimateTokens(content);

    let score = messageImportance(msg.role, content);

    // Engram activation boost — messages that correspond to recalled memories
    // get their ACT-R activation score blended in
    if (engramScores?.has(i)) {
      const engramScore = engramScores.get(i)!;
      // Blend: 40% content importance + 60% engram cognitive score
      score = score * 0.4 + engramScore * 0.6;
    }

    // Recency gradient — last messages get boosted
    const recencyRank = messages.length - i;
    if (recencyRank <= recentFloor) {
      score = Math.max(score, 0.9); // floor at 0.9 for recent
    } else if (recencyRank <= recentFloor * 2) {
      score = Math.max(score, 0.6); // moderate boost for semi-recent
    }

    return { index: i, score, tokens, role: msg.role };
  });

  // Phase 1: Must-include (system messages + recent floor)
  const mustInclude = new Set<number>();
  let usedTokens = 0;

  for (const s of scored) {
    if (s.role === "system") {
      mustInclude.add(s.index);
      usedTokens += s.tokens;
    }
  }

  // Recent floor — last N messages always included
  for (let i = Math.max(0, messages.length - recentFloor); i < messages.length; i++) {
    if (!mustInclude.has(i)) {
      mustInclude.add(i);
      usedTokens += scored[i].tokens;
    }
  }

  // Phase 2: Greedily add by score until budget exhausted
  const remaining = scored
    .filter((s) => !mustInclude.has(s.index))
    .sort((a, b) => b.score - a.score);

  const selected = new Set(mustInclude);

  for (const s of remaining) {
    if (usedTokens + s.tokens > tokenBudget) continue;
    selected.add(s.index);
    usedTokens += s.tokens;
  }

  // Phase 3: Sort by original order to preserve conversation flow
  const selectedIndices = [...selected].sort((a, b) => a - b);

  return { selectedIndices, totalTokens: usedTokens };
}
