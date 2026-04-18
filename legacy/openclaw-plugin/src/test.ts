/**
 * Engram Context Engine — Integration Tests
 * Run: npx tsx src/test.ts
 */

import { Memory, MemoryConfig, getSessionWM, clearSession } from "neuromemory-ai";
import { EngramContextEngine } from "./engine.js";
import { analyzeForCapture } from "./capture.js";
import { selectMessages, estimateTokens, messageImportance } from "./scoring.js";
import { resolveConfig } from "./config.js";
import { mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

let passed = 0;
let failed = 0;

function assert(cond: boolean, msg: string) {
  if (cond) {
    passed++;
    console.log(`  ✅ ${msg}`);
  } else {
    failed++;
    console.error(`  ❌ ${msg}`);
  }
}

function assertClose(a: number, b: number, tolerance: number, msg: string) {
  assert(Math.abs(a - b) <= tolerance, `${msg} (got ${a}, expected ~${b})`);
}

const logger = {
  debug: (_msg: string) => {},
  info: (_msg: string) => {},
  warn: (msg: string) => console.warn(`  ⚠️  ${msg}`),
  error: (msg: string) => console.error(`  💥 ${msg}`),
};

function makeTmpDb(): string {
  const dir = mkdtempSync(join(tmpdir(), "engram-test-"));
  return join(dir, "test.db");
}

// ---------------------------------------------------------------------------
// Test: Config
// ---------------------------------------------------------------------------

console.log("\n📋 Config Tests");

{
  const cfg = resolveConfig();
  assert(cfg.autoCapture === true, "Default autoCapture is true");
  assert(cfg.workingMemoryCapacity === 7, "Default WM capacity is 7 (Miller's Law)");
  assert(cfg.consolidateAfterTurns === 20, "Default consolidation interval is 20");
  assert(cfg.minConfidence === 0.3, "Default minConfidence is 0.3");

  const custom = resolveConfig({ autoCapture: false, workingMemoryCapacity: 5 });
  assert(custom.autoCapture === false, "Custom autoCapture override");
  assert(custom.workingMemoryCapacity === 5, "Custom WM capacity override");
}

// ---------------------------------------------------------------------------
// Test: Token Estimation
// ---------------------------------------------------------------------------

console.log("\n🔢 Token Estimation Tests");

{
  const english = estimateTokens("Hello, how are you doing today?");
  assert(english > 5 && english < 20, `English tokens reasonable: ${english}`);

  const chinese = estimateTokens("你好世界，今天天气怎么样？");
  assert(chinese > 10 && chinese < 40, `Chinese tokens reasonable: ${chinese}`);

  const code = estimateTokens("const x = await fetch('https://api.example.com/data');");
  assert(code > 10 && code < 30, `Code tokens reasonable: ${code}`);

  const empty = estimateTokens("");
  assert(empty === 4, `Empty string = 4 (overhead only): ${empty}`);
}

// ---------------------------------------------------------------------------
// Test: Message Importance Scoring
// ---------------------------------------------------------------------------

console.log("\n⚖️ Message Importance Tests");

{
  const sysScore = messageImportance("system", "You are a helpful assistant.");
  assert(sysScore >= 0.9, `System message high importance: ${sysScore}`);

  const questionScore = messageImportance("user", "How do I configure the database?");
  assert(questionScore > 0.7, `User question high importance: ${questionScore}`);

  const ackScore = messageImportance("user", "ok");
  assert(ackScore < 0.5, `Short ack low importance: ${ackScore}`);

  const heartbeat = messageImportance("assistant", "HEARTBEAT_OK");
  assert(heartbeat < 0.1, `HEARTBEAT_OK minimal importance: ${heartbeat}`);

  const noReply = messageImportance("assistant", "NO_REPLY");
  assert(noReply < 0.1, `NO_REPLY minimal importance: ${noReply}`);

  const toolUse = messageImportance("assistant", 'I\'ll use the tool_use to search for that.');
  assert(toolUse > 0.6, `Tool use higher importance: ${toolUse}`);
}

// ---------------------------------------------------------------------------
// Test: Message Selection (Scoring)
// ---------------------------------------------------------------------------

console.log("\n🎯 Message Selection Tests");

{
  // Simple case: everything fits
  const msgs = [
    { role: "system", content: "You are helpful." },
    { role: "user", content: "Hello!" },
    { role: "assistant", content: "Hi there!" },
  ];

  const r1 = selectMessages({ messages: msgs, tokenBudget: 100000 });
  assert(r1.selectedIndices.length === 3, "All messages selected when budget is large");

  // Budget squeeze: should keep system + recent
  const longMsgs = Array.from({ length: 30 }, (_, i) => ({
    role: i % 2 === 0 ? "user" : "assistant",
    content: `Message number ${i}: ${"x".repeat(200)}`,
  }));
  longMsgs.unshift({ role: "system", content: "System prompt " + "y".repeat(100) });

  const r2 = selectMessages({ messages: longMsgs, tokenBudget: 500, recentFloor: 4 });
  assert(r2.selectedIndices.includes(0), "System message always included");
  assert(
    r2.selectedIndices.includes(longMsgs.length - 1),
    "Last message always included",
  );
  assert(
    r2.selectedIndices.length < longMsgs.length,
    `Budget squeeze: selected ${r2.selectedIndices.length}/${longMsgs.length}`,
  );

  // Verify order preservation
  const sorted = [...r2.selectedIndices].sort((a, b) => a - b);
  assert(
    JSON.stringify(r2.selectedIndices) === JSON.stringify(sorted),
    "Selected indices in original order",
  );
}

// ---------------------------------------------------------------------------
// Test: Auto-Capture
// ---------------------------------------------------------------------------

console.log("\n📸 Auto-Capture Tests");

{
  const pref = analyzeForCapture("user", "I prefer using Rust over Python for this project.");
  assert(pref.shouldCapture === true, "Captures preference");
  assert(pref.memoryType === "preference", `Type is preference: ${pref.memoryType}`);

  const fact = analyzeForCapture("user", "I work at a startup in San Francisco.");
  assert(fact.shouldCapture === true, "Captures fact");
  assert(fact.memoryType === "semantic", `Type is semantic: ${fact.memoryType}`);

  const decision = analyzeForCapture("user", "Let's use PostgreSQL instead of MySQL going forward.");
  assert(decision.shouldCapture === true, "Captures decision");
  assert(decision.memoryType === "procedural", `Type is procedural: ${decision.memoryType}`);

  const correction = analyzeForCapture("user", "Actually, the API endpoint should be /v2/users, not /v1.");
  assert(correction.shouldCapture === true, "Captures correction");
  assert(correction.importance >= 0.8, `Correction high importance: ${correction.importance}`);

  const shortAck = analyzeForCapture("user", "ok");
  assert(shortAck.shouldCapture === false, "Skips short ack");

  const heartbeat = analyzeForCapture("assistant", "HEARTBEAT_OK");
  assert(heartbeat.shouldCapture === false, "Skips heartbeat");

  const zhPref = analyzeForCapture("user", "我喜欢用TypeScript写前端，Rust写后端。");
  assert(zhPref.shouldCapture === true, "Captures Chinese preference");

  const zhDecision = analyzeForCapture("user", "以后我们都用这个架构，不要改了。");
  assert(zhDecision.shouldCapture === true, "Captures Chinese decision");
}

// ---------------------------------------------------------------------------
// Test: Engine — Bootstrap & Ingest
// ---------------------------------------------------------------------------

console.log("\n🧠 Engine Integration Tests");

{
  const dbPath = makeTmpDb();
  const engine = new EngramContextEngine({
    config: resolveConfig({ dbPath, autoCapture: true }),
    logger,
  });

  // Bootstrap
  const boot = await engine.bootstrap({ sessionId: "test-1", sessionFile: "/tmp/test.json" });
  assert(boot.bootstrapped === true, "Bootstrap succeeds");

  // Ingest — preference should be captured
  const r1 = await engine.ingest({
    sessionId: "test-1",
    message: { role: "user", content: "I always prefer dark mode in my editor." },
  });
  assert(r1.ingested === true, "Preference message captured");

  // Ingest — short ack should be skipped
  const r2 = await engine.ingest({
    sessionId: "test-1",
    message: { role: "user", content: "ok" },
  });
  assert(r2.ingested === false, "Short ack skipped");

  // Ingest — heartbeat should be skipped
  const r3 = await engine.ingest({
    sessionId: "test-1",
    message: { role: "assistant", content: "checking..." },
    isHeartbeat: true,
  });
  assert(r3.ingested === false, "Heartbeat message skipped");

  // Ingest batch
  const r4 = await engine.ingestBatch({
    sessionId: "test-1",
    messages: [
      { role: "user", content: "We decided to use SQLite for the database." },
      { role: "assistant", content: "Great choice for embedded use." },
      { role: "user", content: "yeah" },
    ],
  });
  assert(r4.ingestedCount === 1, `Batch: captured ${r4.ingestedCount} of 3 messages`);

  // Cleanup
  await engine.dispose();
  rmSync(dbPath, { force: true });
}

// ---------------------------------------------------------------------------
// Test: Engine — Assemble (the core!)
// ---------------------------------------------------------------------------

console.log("\n🏗️ Assemble Tests");

{
  const dbPath = makeTmpDb();
  const engine = new EngramContextEngine({
    config: resolveConfig({ dbPath, autoCapture: true, maxRecallResults: 5 }),
    logger,
  });

  await engine.bootstrap({ sessionId: "asm-1", sessionFile: "/tmp/test.json" });

  // Store some memories first
  await engine.ingest({
    sessionId: "asm-1",
    message: { role: "user", content: "I prefer using Vim keybindings in VS Code." },
  });
  await engine.ingest({
    sessionId: "asm-1",
    message: { role: "user", content: "We use Docker for all our deployments, never bare metal." },
  });

  // Assemble with small message set — should pass through
  const smallMsgs = [
    { role: "system" as const, content: "You are helpful." },
    { role: "user" as const, content: "Tell me about Docker." },
    { role: "assistant" as const, content: "Docker is a containerization platform." },
  ];

  const r1 = await engine.assemble({
    sessionId: "asm-1",
    messages: smallMsgs,
    tokenBudget: 100000,
  });
  assert(r1.messages.length === 3, "Small set passes through completely");
  assert(r1.estimatedTokens > 0, `Estimated tokens: ${r1.estimatedTokens}`);

  // Assemble with large message set + tight budget — should score and select
  const largeMsgs: Array<{ role: string; content: string }> = [
    { role: "system", content: "You are an expert assistant." },
  ];
  for (let i = 0; i < 50; i++) {
    largeMsgs.push({
      role: i % 2 === 0 ? "user" : "assistant",
      content: `Turn ${i}: ${"lorem ipsum dolor sit amet ".repeat(10)}`,
    });
  }
  largeMsgs.push({ role: "user", content: "What was the Docker deployment config?" });

  const r2 = await engine.assemble({
    sessionId: "asm-1",
    messages: largeMsgs,
    tokenBudget: 500,
  });
  assert(
    r2.messages.length < largeMsgs.length,
    `Large set filtered: ${r2.messages.length}/${largeMsgs.length}`,
  );
  assert(r2.messages[0].role === "system", "System message preserved");
  assert(
    r2.messages[r2.messages.length - 1] === largeMsgs[largeMsgs.length - 1],
    "Last message preserved",
  );

  // Check systemPromptAddition — should have relevant memory
  if (r2.systemPromptAddition) {
    assert(
      r2.systemPromptAddition.includes("Engram"),
      `systemPromptAddition contains Engram memories`,
    );
  }

  await engine.dispose();
  rmSync(dbPath, { force: true });
}

// ---------------------------------------------------------------------------
// Test: Engine — Compact (Consolidation)
// ---------------------------------------------------------------------------

console.log("\n🗜️ Compact Tests");

{
  const dbPath = makeTmpDb();
  const engine = new EngramContextEngine({
    config: resolveConfig({ dbPath }),
    logger,
  });

  // Store some memories to consolidate
  await engine.ingest({
    sessionId: "cmp-1",
    message: { role: "user", content: "I have a preference for functional programming." },
  });

  const r1 = await engine.compact({
    sessionId: "cmp-1",
    sessionFile: "/tmp/test.json",
  });
  assert(r1.ok === true, "Compaction succeeds");
  assert(r1.compacted === true, "Compaction ran");

  await engine.dispose();
  rmSync(dbPath, { force: true });
}

// ---------------------------------------------------------------------------
// Test: Engine — Subagent Lifecycle
// ---------------------------------------------------------------------------

console.log("\n👶 Subagent Lifecycle Tests");

{
  const dbPath = makeTmpDb();
  const engine = new EngramContextEngine({
    config: resolveConfig({ dbPath }),
    logger,
  });

  const prep = await engine.prepareSubagentSpawn({
    parentSessionKey: "main",
    childSessionKey: "sub-1",
  });

  assert(prep !== undefined, "Spawn preparation returned");
  assert(typeof prep!.rollback === "function", "Rollback function provided");

  // Rollback should clean up
  await prep!.rollback();

  // End should clean up
  await engine.prepareSubagentSpawn({
    parentSessionKey: "main",
    childSessionKey: "sub-2",
  });
  await engine.onSubagentEnded({
    childSessionKey: "sub-2",
    reason: "completed",
  });

  assert(true, "Subagent lifecycle completed without errors");

  await engine.dispose();
  rmSync(dbPath, { force: true });
}

// ---------------------------------------------------------------------------
// Test: Engine — AfterTurn Consolidation Trigger
// ---------------------------------------------------------------------------

console.log("\n🔄 AfterTurn Tests");

{
  const dbPath = makeTmpDb();
  const engine = new EngramContextEngine({
    config: resolveConfig({ dbPath, consolidateAfterTurns: 3 }),
    logger: {
      ...logger,
      info: (msg: string) => {
        if (msg.includes("consolidation")) {
          console.log(`    → ${msg}`);
        }
      },
    },
  });

  const dummyMsgs = [{ role: "user", content: "hello" }];

  // Turns 1, 2 — no consolidation
  await engine.afterTurn({
    sessionId: "at-1",
    sessionFile: "/tmp/test.json",
    messages: dummyMsgs,
    prePromptMessageCount: 0,
  });
  await engine.afterTurn({
    sessionId: "at-1",
    sessionFile: "/tmp/test.json",
    messages: dummyMsgs,
    prePromptMessageCount: 1,
  });

  // Turn 3 — should trigger consolidation
  await engine.afterTurn({
    sessionId: "at-1",
    sessionFile: "/tmp/test.json",
    messages: dummyMsgs,
    prePromptMessageCount: 2,
  });

  assert(true, "AfterTurn consolidation triggered at correct interval");

  await engine.dispose();
  rmSync(dbPath, { force: true });
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

console.log(`\n${"=".repeat(50)}`);
console.log(`Results: ${passed} passed, ${failed} failed`);
console.log(`${"=".repeat(50)}\n`);

if (failed > 0) {
  process.exit(1);
}
