/**
 * Engram Context Engine — OpenClaw Plugin Entry Point
 *
 * Registers the Engram cognitive context engine as an OpenClaw plugin.
 *
 * Config:
 *   plugins.slots.contextEngine: "engram"
 *   plugins.entries.engram:
 *     dbPath: "~/.openclaw/engram.db"
 *     autoCapture: true
 *     # ... see openclaw.plugin.json for full schema
 *
 * What changes:
 *   Instead of keeping the last N messages (FIFO), Engram scores every
 *   message by ACT-R activation (recency × frequency), Hebbian association
 *   (semantic neighborhood), and content importance — then packs the most
 *   cognitively relevant messages into the token budget.
 *
 *   Long-term memories are automatically captured and recalled across
 *   sessions, giving the agent persistent context without manual prompting.
 */

import { resolveConfig } from "./config.js";
import { EngramContextEngine } from "./engine.js";

// Plugin definition — OpenClaw loads this module and calls register()
export default {
  id: "engram",
  name: "Engram Cognitive Context Engine",
  version: "1.0.0",
  kind: "context-engine" as const,

  register(api: {
    id: string;
    logger: {
      debug?: (msg: string) => void;
      info: (msg: string) => void;
      warn: (msg: string) => void;
      error: (msg: string) => void;
    };
    pluginConfig?: Record<string, unknown>;
    registerContextEngine: (
      id: string,
      factory: () => unknown,
    ) => void;
  }) {
    const config = resolveConfig(api.pluginConfig);

    api.registerContextEngine("engram", () => {
      return new EngramContextEngine({
        config,
        logger: api.logger,
      });
    });

    api.logger.info(
      `Engram context engine registered (db: ${config.dbPath}, ` +
        `autoCapture: ${config.autoCapture}, ` +
        `wmCapacity: ${config.workingMemoryCapacity})`,
    );
  },
};
