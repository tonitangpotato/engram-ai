/**
 * Engram Context Engine — Configuration
 */

import { homedir } from "node:os";
import { join } from "node:path";

export interface EngramPluginConfig {
  dbPath: string;
  embedding: {
    provider: "openai" | "ollama" | "mcp" | "none";
    apiKey?: string;
    model?: string;
    baseUrl?: string;
  };
  autoCapture: boolean;
  consolidateAfterTurns: number;
  workingMemoryCapacity: number;
  workingMemoryDecaySec: number;
  maxRecallResults: number;
  minConfidence: number;
}

const DEFAULT_DB_PATH = join(homedir(), ".openclaw", "engram.db");

export function resolveConfig(
  raw?: Record<string, unknown>,
): EngramPluginConfig {
  const cfg = raw ?? {};
  const embedding = (cfg.embedding as Record<string, unknown>) ?? {};

  return {
    dbPath: (cfg.dbPath as string) ?? DEFAULT_DB_PATH,
    embedding: {
      provider:
        (embedding.provider as EngramPluginConfig["embedding"]["provider"]) ??
        "none",
      apiKey: embedding.apiKey as string | undefined,
      model: embedding.model as string | undefined,
      baseUrl: embedding.baseUrl as string | undefined,
    },
    autoCapture: (cfg.autoCapture as boolean) ?? true,
    consolidateAfterTurns: (cfg.consolidateAfterTurns as number) ?? 20,
    workingMemoryCapacity: (cfg.workingMemoryCapacity as number) ?? 7,
    workingMemoryDecaySec: (cfg.workingMemoryDecaySec as number) ?? 300,
    maxRecallResults: (cfg.maxRecallResults as number) ?? 10,
    minConfidence: (cfg.minConfidence as number) ?? 0.3,
  };
}
