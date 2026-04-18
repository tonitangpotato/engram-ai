/**
 * Engram Context Engine — Core Implementation
 *
 * Replaces FIFO context windowing with neuroscience-grounded memory:
 *
 * - ACT-R activation decay: memories strengthen with use, decay with time
 * - Hebbian association: co-activated memories form links
 * - Working memory: Miller's Law 7±2 chunks, topic-change detection
 * - Ebbinghaus consolidation: periodic forgetting + strengthening
 *
 * The key insight: instead of "keep last N messages", we score every
 * message by cognitive relevance and pack the most meaningful ones
 * into the token budget.
 */

import {
  Memory,
  MemoryConfig,
  getSessionWM,
  clearSession,
} from "neuromemory-ai";
import type { EngramPluginConfig } from "./config.js";
import { analyzeForCapture } from "./capture.js";
import { selectMessages, estimateTokens } from "./scoring.js";

export interface EngramContextEngineOptions {
  config: EngramPluginConfig;
  logger: {
    debug?: (msg: string) => void;
    info: (msg: string) => void;
    warn: (msg: string) => void;
    error: (msg: string) => void;
  };
}

interface AgentMessage {
  role: string;
  content?: string | unknown;
  [key: string]: unknown;
}

export class EngramContextEngine {
  readonly info = {
    id: "engram",
    name: "Engram Cognitive Context Engine",
    version: "1.0.0",
    ownsCompaction: true, // We handle our own context management
  };

  private memory: Memory;
  private config: EngramPluginConfig;
  private logger: EngramContextEngineOptions["logger"];
  private turnCounters: Map<string, number> = new Map();

  constructor(opts: EngramContextEngineOptions) {
    this.config = opts.config;
    this.logger = opts.logger;

    // Initialize Engram memory with cognitive parameters
    const memConfig = new MemoryConfig({
      actrDecay: 0.5,
      contextWeight: 1.0,
      importanceWeight: 0.8,
      hebbianEnabled: true,
      hebbianDecay: 0.95,
      downscaleFactor: 0.8,
    });
    this.memory = new Memory(this.config.dbPath, memConfig);

    this.logger.info(
      `Engram context engine initialized (db: ${this.config.dbPath})`,
    );
  }

  // ---------------------------------------------------------------------------
  // bootstrap — Initialize engine for a session
  // ---------------------------------------------------------------------------
  async bootstrap(params: {
    sessionId: string;
    sessionFile: string;
  }): Promise<{ bootstrapped: boolean; importedMessages?: number; reason?: string }> {
    const { sessionId } = params;

    // Ensure working memory exists for this session
    getSessionWM(sessionId);

    this.logger.info(`Engram bootstrapped for session: ${sessionId}`);
    return { bootstrapped: true };
  }

  // ---------------------------------------------------------------------------
  // ingest — Store a message, optionally capture as long-term memory
  // ---------------------------------------------------------------------------
  async ingest(params: {
    sessionId: string;
    message: AgentMessage;
    isHeartbeat?: boolean;
  }): Promise<{ ingested: boolean }> {
    const { sessionId, message, isHeartbeat } = params;

    // Skip heartbeat messages for memory capture
    if (isHeartbeat) return { ingested: false };

    if (!this.config.autoCapture) return { ingested: false };

    const content = extractContent(message);
    if (!content) return { ingested: false };

    // Analyze if this message is worth storing long-term
    const decision = analyzeForCapture(message.role, content);

    if (decision.shouldCapture) {
      try {
        this.memory.add(decision.content, {
          type: decision.memoryType,
          importance: decision.importance,
          source: `session:${sessionId}`,
        });

        this.logger.debug?.(
          `Captured ${decision.memoryType} memory (importance: ${decision.importance}): ${decision.content.slice(0, 80)}...`,
        );
        return { ingested: true };
      } catch (err) {
        this.logger.error(
          `Failed to capture memory: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }

    return { ingested: false };
  }

  // ---------------------------------------------------------------------------
  // ingestBatch — Batch ingest a completed turn
  // ---------------------------------------------------------------------------
  async ingestBatch(params: {
    sessionId: string;
    messages: AgentMessage[];
    isHeartbeat?: boolean;
  }): Promise<{ ingestedCount: number }> {
    let count = 0;
    for (const msg of params.messages) {
      const result = await this.ingest({
        sessionId: params.sessionId,
        message: msg,
        isHeartbeat: params.isHeartbeat,
      });
      if (result.ingested) count++;
    }
    return { ingestedCount: count };
  }

  // ---------------------------------------------------------------------------
  // afterTurn — Post-turn lifecycle (consolidation check)
  // ---------------------------------------------------------------------------
  async afterTurn(params: {
    sessionId: string;
    sessionFile: string;
    messages: AgentMessage[];
    prePromptMessageCount: number;
    isHeartbeat?: boolean;
  }): Promise<void> {
    const { sessionId } = params;

    // Increment turn counter
    const turns = (this.turnCounters.get(sessionId) ?? 0) + 1;
    this.turnCounters.set(sessionId, turns);

    // Periodic consolidation — Ebbinghaus forgetting + Hebbian strengthening
    if (turns % this.config.consolidateAfterTurns === 0) {
      try {
        this.memory.consolidate(1.0); // 1 day equivalent
        this.logger.info(
          `Ran consolidation cycle after ${turns} turns (session: ${sessionId})`,
        );
      } catch (err) {
        this.logger.warn(
          `Consolidation failed: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }
  }

  // ---------------------------------------------------------------------------
  // assemble — THE CORE: Build cognitively-scored context under token budget
  // ---------------------------------------------------------------------------
  async assemble(params: {
    sessionId: string;
    messages: AgentMessage[];
    tokenBudget?: number;
  }): Promise<{
    messages: AgentMessage[];
    estimatedTokens: number;
    systemPromptAddition?: string;
  }> {
    const { sessionId, messages, tokenBudget } = params;
    const budget = tokenBudget ?? 200_000; // sensible default

    // If messages fit in budget, return all (no need to score)
    const totalTokens = messages.reduce((sum, m) => {
      return sum + estimateTokens(extractContent(m) ?? "");
    }, 0);

    if (totalTokens <= budget) {
      return {
        messages,
        estimatedTokens: totalTokens,
        systemPromptAddition: await this.buildMemoryContext(sessionId, messages),
      };
    }

    // --- Cognitive scoring ---
    const wm = getSessionWM(sessionId);

    // Extract the current topic from recent messages
    const recentContent = messages
      .slice(-3)
      .map((m) => extractContent(m))
      .filter(Boolean)
      .join(" ");

    // Get Engram activation scores for message content
    const engramScores = new Map<number, number>();

    if (recentContent && wm.needsRecall(recentContent, this.memory)) {
      // Topic changed or WM empty — do full recall
      try {
        const recalled = this.memory.recall(recentContent, {
          limit: this.config.maxRecallResults,
          minConfidence: this.config.minConfidence,
        });

        // Match recalled memories against message content
        if (recalled.length > 0) {
          const recalledContent = new Set(recalled.map((r) => r.content));

          for (let i = 0; i < messages.length; i++) {
            const msgContent = extractContent(messages[i]);
            if (!msgContent) continue;

            // Check if this message content is semantically close to a recalled memory
            for (const rc of recalledContent) {
              if (contentOverlap(msgContent, rc) > 0.3) {
                // Normalize activation to 0-1 range
                const recalledItem = recalled.find((r) => r.content === rc);
                if (recalledItem) {
                  engramScores.set(i, Math.min(1, recalledItem.confidence));
                }
                break;
              }
            }
          }

          // Update working memory with recalled IDs
          wm.activate(recalled.map((r) => r.id));
        }
      } catch (err) {
        this.logger.warn(
          `Engram recall failed, falling back to recency: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }

    // Select messages using cognitive scoring
    const { selectedIndices, totalTokens: selected_tokens } = selectMessages({
      messages: messages.map((m) => ({
        role: m.role,
        content: extractContent(m),
      })),
      tokenBudget: budget,
      recentFloor: 6,
      engramScores,
    });

    const selectedMessages = selectedIndices.map((i) => messages[i]);

    this.logger.info(
      `Assembled context: ${selectedMessages.length}/${messages.length} messages, ` +
        `~${selected_tokens} tokens (budget: ${budget}), ` +
        `${engramScores.size} engram-scored`,
    );

    return {
      messages: selectedMessages,
      estimatedTokens: selected_tokens,
      systemPromptAddition: await this.buildMemoryContext(sessionId, messages),
    };
  }

  // ---------------------------------------------------------------------------
  // compact — Consolidate and prune
  // ---------------------------------------------------------------------------
  async compact(params: {
    sessionId: string;
    sessionFile: string;
    tokenBudget?: number;
    force?: boolean;
  }): Promise<{
    ok: boolean;
    compacted: boolean;
    reason?: string;
    result?: {
      summary?: string;
      tokensBefore: number;
      tokensAfter?: number;
    };
  }> {
    try {
      this.memory.consolidate(1.0);

      const stats = this.memory.stats() as Record<string, unknown>;
      const totalMemories = (stats.total_memories as number) ?? 0;
      const byType = (stats.by_type as Record<string, unknown>) ?? {};

      this.logger.info(
        `Compaction: consolidated ${totalMemories} memories across ${Object.keys(byType).length} types`,
      );

      return {
        ok: true,
        compacted: true,
        reason: "Engram consolidation cycle completed",
        result: {
          summary: `Consolidated ${totalMemories} memories across ${Object.keys(byType).length} types`,
          tokensBefore: 0, // N/A for Engram — we score, not trim
        },
      };
    } catch (err) {
      return {
        ok: false,
        compacted: false,
        reason: `Consolidation error: ${err instanceof Error ? err.message : String(err)}`,
      };
    }
  }

  // ---------------------------------------------------------------------------
  // Subagent lifecycle
  // ---------------------------------------------------------------------------
  async prepareSubagentSpawn(params: {
    parentSessionKey: string;
    childSessionKey: string;
    ttlMs?: number;
  }): Promise<{ rollback: () => void | Promise<void> } | undefined> {
    // Create a working memory scope for the child
    getSessionWM(params.childSessionKey);

    this.logger.debug?.(
      `Prepared subagent WM: ${params.childSessionKey} (parent: ${params.parentSessionKey})`,
    );

    return {
      rollback: () => {
        clearSession(params.childSessionKey);
      },
    };
  }

  async onSubagentEnded(params: {
    childSessionKey: string;
    reason: string;
  }): Promise<void> {
    // Clean up child working memory
    clearSession(params.childSessionKey);
    this.turnCounters.delete(params.childSessionKey);

    this.logger.debug?.(
      `Cleaned up subagent WM: ${params.childSessionKey} (reason: ${params.reason})`,
    );
  }

  // ---------------------------------------------------------------------------
  // dispose
  // ---------------------------------------------------------------------------
  async dispose(): Promise<void> {
    // Run final consolidation
    try {
      this.memory.consolidate(1.0);
    } catch {
      // Best effort
    }
    this.logger.info("Engram context engine disposed");
  }

  // ---------------------------------------------------------------------------
  // Private helpers
  // ---------------------------------------------------------------------------

  /**
   * Build a system prompt addition with relevant long-term memories.
   * Injects pinned + high-activation memories as background context.
   */
  private async buildMemoryContext(
    sessionId: string,
    messages: AgentMessage[],
  ): Promise<string | undefined> {
    const recentContent = messages
      .slice(-5)
      .map((m) => extractContent(m))
      .filter(Boolean)
      .join(" ");

    if (!recentContent) return undefined;

    try {
      const recalled = this.memory.recall(recentContent, {
        limit: 5,
        minConfidence: 0.5,
      });

      if (recalled.length === 0) return undefined;

      const lines = recalled.map(
        (r) =>
          `- ${r.content} (${r.confidence_label}, ${r.type})`,
      );

      return `[Relevant memories from Engram]:\n${lines.join("\n")}`;
    } catch {
      return undefined;
    }
  }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

function extractContent(message: AgentMessage): string {
  if (typeof message.content === "string") return message.content;
  if (Array.isArray(message.content)) {
    return (message.content as Array<{ text?: string; type?: string }>)
      .filter((p) => p.type === "text" && p.text)
      .map((p) => p.text!)
      .join("\n");
  }
  if (message.content) return JSON.stringify(message.content);
  return "";
}

/**
 * Simple word-overlap ratio between two texts.
 * Used for matching recalled memories against message content.
 */
function contentOverlap(a: string, b: string): number {
  const wordsA = new Set(
    a
      .toLowerCase()
      .split(/\s+/)
      .filter((w) => w.length > 2),
  );
  const wordsB = new Set(
    b
      .toLowerCase()
      .split(/\s+/)
      .filter((w) => w.length > 2),
  );

  if (wordsA.size === 0 || wordsB.size === 0) return 0;

  let overlap = 0;
  for (const w of wordsA) {
    if (wordsB.has(w)) overlap++;
  }

  return overlap / Math.min(wordsA.size, wordsB.size);
}
