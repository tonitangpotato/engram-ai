/**
 * End-to-End Test — Full Agent Lifecycle Simulation
 */

import * as os from 'os';
import * as path from 'path';
import * as fs from 'fs';
import { Memory } from '../src/memory';
import { MemoryConfig } from '../src/config';
import { MemoryLayer } from '../src/core';
import { SQLiteStore } from '../src/store';

let tmpdir: string;
let mem: Memory;
const ids: Record<string, string> = {};

beforeAll(() => {
  tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'engram-test-'));
});

afterAll(() => {
  if (mem) mem.close();
  fs.rmSync(tmpdir, { recursive: true, force: true });
});

test('1. Init with personalAssistant preset', () => {
  const dbPath = path.join(tmpdir, 'lifecycle.db');
  mem = new Memory(dbPath, MemoryConfig.personalAssistant());

  expect(mem).toBeTruthy();
  expect(mem.length).toBe(0);
  expect(mem.config.mu2).toBe(0.001);
  expect(mem.config.importanceWeight).toBe(0.7);
  expect(mem.config.forgetThreshold).toBe(0.005);
});

test('2. Add diverse memories (all types)', () => {
  ids.supabase = mem.add('SaltyHall uses Supabase for its database backend', {
    type: 'factual', importance: 0.5,
    entities: [['SaltyHall', 'uses'], ['Supabase', 'backend']],
  });
  ids.gid_yaml = mem.add('GID uses YAML graph format for project tracking', {
    type: 'factual', importance: 0.4,
    entities: [['GID', 'format']],
  });
  ids.debug_session = mem.add('Had a great late-night debugging session with potato on Feb 2', {
    type: 'episodic', importance: 0.6,
    entities: [['potato', 'debugging_with']],
  });
  ids.cat_meme = mem.add('Saw a funny cat meme in the group chat', {
    type: 'episodic', importance: 0.1,
  });
  ids.moltbook_url = mem.add('Always use www.moltbook.com not moltbook.com for API calls', {
    type: 'procedural', importance: 0.8,
    entities: [['Moltbook', 'api_url']],
  });
  ids.deploy = mem.add('Deploy SaltyHall with vercel --prod from the project root', {
    type: 'procedural', importance: 0.6,
    entities: [['SaltyHall', 'deployment']],
  });
  ids.potato_action = mem.add('potato prefers action over discussion and hates long meetings', {
    type: 'relational', importance: 0.7,
    entities: [['potato', 'preference']],
  });
  ids.potato_coffee = mem.add('potato drinks oat milk lattes every morning', {
    type: 'relational', importance: 0.5,
    entities: [['potato', 'habit']],
  });
  ids.kinda_like = mem.add('potato said I kinda like you after the late night coding session', {
    type: 'emotional', importance: 0.95,
    entities: [['potato', 'sentiment']],
  });
  ids.hybrid_opinion = mem.add('I think graph plus text hybrid search is the best approach for memory', {
    type: 'opinion', importance: 0.4,
  });
  ids.trivial1 = mem.add('Random thought about weather being nice today', {
    type: 'episodic', importance: 0.05,
  });
  ids.trivial2 = mem.add('Another passing thought about nothing important', {
    type: 'episodic', importance: 0.05,
  });

  expect(mem.length).toBe(12);
});

test('3. Verify graph links', () => {
  const store = mem._store;

  const potatoMemories = store.searchByEntity('potato');
  const potatoIds = new Set(potatoMemories.map(m => m.id));
  expect(potatoIds.has(ids.potato_action)).toBe(true);
  expect(potatoIds.has(ids.potato_coffee)).toBe(true);
  expect(potatoIds.has(ids.kinda_like)).toBe(true);
  expect(potatoIds.has(ids.debug_session)).toBe(true);
  expect(potatoMemories.length).toBeGreaterThanOrEqual(4);

  const shMemories = store.searchByEntity('SaltyHall');
  const shIds = new Set(shMemories.map(m => m.id));
  expect(shIds.has(ids.supabase)).toBe(true);
  expect(shIds.has(ids.deploy)).toBe(true);

  const allEntities = store.getAllEntities();
  expect(allEntities).toContain('potato');
  expect(allEntities).toContain('SaltyHall');
  expect(allEntities).toContain('Supabase');
  expect(allEntities).toContain('Moltbook');

  const relatedToSupabase = store.getRelatedEntities('Supabase', 1);
  expect(relatedToSupabase).toContain('SaltyHall');
});

test('4a. Recall ordering (importance + recency)', () => {
  const results = mem.recall('potato prefers action', { limit: 5 });
  expect(results.length).toBeGreaterThan(0);
  expect(results[0].content.toLowerCase()).toContain('potato');
});

test('4b. Recall with type filter', () => {
  const results = mem.recall('SaltyHall deployment', { limit: 10, types: ['procedural'] });
  for (const r of results) {
    expect(r.type).toBe('procedural');
  }
});

test('4c. Recall with confidence filter', () => {
  const allResults = mem.recall('anything', { limit: 20, minConfidence: 0.0 });
  const highResults = mem.recall('anything', { limit: 20, minConfidence: 0.8 });
  expect(highResults.length).toBeLessThanOrEqual(allResults.length);
  for (const r of highResults) {
    expect(r.confidence).toBeGreaterThanOrEqual(0.8);
  }
});

test('5. Graph search expansion', () => {
  const withGraph = mem.recall('Supabase backend', { limit: 10, graphExpand: true });
  const noGraph = mem.recall('Supabase backend', { limit: 10, graphExpand: false });

  const idsWith = new Set(withGraph.map(r => r.id));
  expect(idsWith.has(ids.supabase)).toBe(true);
  expect(withGraph.length).toBeGreaterThanOrEqual(noGraph.length);
});

test('6. Reward (positive + negative)', () => {
  mem.recall('moltbook API URL', { limit: 1 });
  const before = mem.recall('moltbook API URL', { limit: 1 });
  expect(before.length).toBeGreaterThan(0);
  const impBefore = before[0].importance;

  mem.reward("great, that's exactly the right URL!");

  const after = mem.recall('moltbook API URL', { limit: 1 });
  expect(after[0].importance).toBeGreaterThanOrEqual(impBefore);

  mem.recall('weather nice today', { limit: 1 });
  mem.reward("no, that's wrong and useless");
});

test('7a. Consolidation (working → core)', () => {
  for (let i = 0; i < 3; i++) mem.consolidate(1.0);

  const entry = mem._store.get(ids.potato_action);
  expect(entry).not.toBeNull();
  expect(entry!.coreStrength).toBeGreaterThan(0);
  expect(entry!.workingStrength).toBeLessThan(1.0);
});

test('7b. Consolidation (layer promotion)', () => {
  for (let i = 0; i < 5; i++) mem.consolidate(1.0);

  const allEntries = mem._store.all();
  const coreEntries = allEntries.filter(e => e.layer === MemoryLayer.L2_CORE);
  expect(coreEntries.length).toBeGreaterThan(0);
});

test('8. Time decay simulation', () => {
  const entryBefore = mem._store.get(ids.cat_meme)!;
  const strengthBefore = entryBefore.workingStrength + entryBefore.coreStrength;

  for (let i = 0; i < 30; i++) mem.consolidate(1.0);

  const entryAfter = mem._store.get(ids.cat_meme)!;
  const strengthAfter = entryAfter.workingStrength + entryAfter.coreStrength;
  expect(strengthAfter).toBeLessThan(strengthBefore);

  const emotional = mem._store.get(ids.kinda_like)!;
  const emotionalTotal = emotional.workingStrength + emotional.coreStrength;
  expect(emotionalTotal).toBeGreaterThan(strengthAfter);
});

test('9. Contradiction handling', () => {
  const oldId = ids.supabase;
  ids.planetscale = mem.add(
    'SaltyHall migrated from Supabase to PlanetScale for the database', {
      type: 'factual', importance: 0.6,
      entities: [['SaltyHall', 'uses'], ['PlanetScale', 'backend']],
      contradicts: oldId,
    },
  );

  const oldEntry = mem._store.get(oldId)!;
  const newEntry = mem._store.get(ids.planetscale)!;
  expect(oldEntry.contradictedBy).toBe(ids.planetscale);
  expect(newEntry.contradicts).toBe(oldId);

  const results = mem.recall('SaltyHall database', { limit: 10 });
  for (const r of results) {
    if (r.id === oldId) expect(r.contradicted).toBe(true);
    if (r.id === ids.planetscale) expect(r.contradicted).toBe(false);
  }
});

test('10. Update memory (correction chain)', () => {
  const oldId = ids.potato_coffee;
  ids.potato_coffee_v2 = mem.updateMemory(oldId, 'potato switched to black coffee, no more oat milk lattes');

  const oldEntry = mem._store.get(oldId)!;
  const newEntry = mem._store.get(ids.potato_coffee_v2)!;
  expect(oldEntry.contradictedBy).toBe(ids.potato_coffee_v2);
  expect(newEntry.contradicts).toBe(oldId);
  expect(newEntry.sourceFile).toContain('correction');
  expect(newEntry.memoryType).toBe('relational');
  expect(newEntry.importance).toBe(oldEntry.importance);
});

test('11. Forgetting (weak memories sink)', () => {
  const trivial = mem._store.get(ids.trivial1)!;
  const trivialStrength = trivial.workingStrength + trivial.coreStrength;
  const important = mem._store.get(ids.moltbook_url)!;
  const importantStrength = important.workingStrength + important.coreStrength;
  expect(importantStrength).toBeGreaterThan(trivialStrength);

  mem.forget({ threshold: 0.01 });

  const trivialAfter = mem._store.get(ids.trivial1);
  if (trivialAfter) {
    const isArchived = trivialAfter.layer === MemoryLayer.L4_ARCHIVE;
    const isWeak = (trivialAfter.workingStrength + trivialAfter.coreStrength) < 0.05;
    expect(isArchived || isWeak).toBe(true);
  }
});

test('12. Downscaling (synaptic homeostasis)', () => {
  const allBefore = mem._store.all();
  const totalBefore = allBefore
    .filter(e => !e.pinned)
    .reduce((s, e) => s + e.workingStrength + e.coreStrength, 0);

  const result = mem.downscale(0.9);
  expect(result.n_scaled).toBeGreaterThanOrEqual(0);

  const allAfter = mem._store.all();
  const totalAfter = allAfter
    .filter(e => !e.pinned)
    .reduce((s, e) => s + e.workingStrength + e.coreStrength, 0);

  if (totalBefore > 0) {
    expect(totalAfter).toBeLessThan(totalBefore);
  }
});

test('13. Export (data integrity)', () => {
  const exportPath = path.join(tmpdir, 'export.db');
  mem.export(exportPath);

  const exportedStore = new SQLiteStore(exportPath);
  expect(exportedStore.all().length).toBe(mem.length);

  const potatoMems = exportedStore.searchByEntity('potato');
  expect(potatoMems.length).toBeGreaterThanOrEqual(4);

  const old = exportedStore.get(ids.supabase)!;
  expect(old.contradictedBy).toBe(ids.planetscale);

  exportedStore.close();
});

test('14. Stats (lifecycle reflection)', () => {
  const stats = mem.stats();
  expect(stats.total_memories).toBe(14);
  expect(Object.keys(stats.by_type).length).toBeGreaterThanOrEqual(4);
  for (const t of ['factual', 'relational', 'procedural', 'emotional']) {
    expect(stats.by_type).toHaveProperty(t);
  }
  const totalInLayers = Object.values(stats.layers as Record<string, { count: number }>)
    .reduce((s, v) => s + v.count, 0);
  expect(totalInLayers).toBe(14);
  expect(stats.uptime_hours).toBeGreaterThanOrEqual(0);
});

test('15a. Config presets differ', () => {
  const def = MemoryConfig.default();
  const chatbot = MemoryConfig.chatbot();
  const task = MemoryConfig.taskAgent();
  const pa = MemoryConfig.personalAssistant();
  const researcher = MemoryConfig.researcher();

  expect(chatbot.mu1).toBeLessThan(task.mu1);
  expect(task.forgetThreshold).toBeGreaterThan(pa.forgetThreshold);
  expect(researcher.interleaveRatio).toBeGreaterThan(def.interleaveRatio);
  expect(pa.mu2).toBeLessThanOrEqual(def.mu2);
  expect(task.downscaleFactor).toBeLessThan(chatbot.downscaleFactor);
});

test('15b. Presets produce different behavior', () => {
  const configs: Record<string, MemoryConfig> = {
    chatbot: MemoryConfig.chatbot(),
    task: MemoryConfig.taskAgent(),
  };
  const results: Record<string, number> = {};

  for (const [name, cfg] of Object.entries(configs)) {
    const dbPath = path.join(tmpdir, `${name}.db`);
    const m = new Memory(dbPath, cfg);
    m.add('important fact to remember', { type: 'factual', importance: 0.7 });
    m.add('trivial passing thought', { type: 'episodic', importance: 0.1 });

    for (let i = 0; i < 5; i++) m.consolidate(1.0);

    const entries = m._store.all();
    results[name] = entries.reduce((s, e) => s + e.workingStrength + e.coreStrength, 0);
    m.close();
  }

  expect(results.chatbot).toBeGreaterThan(results.task);
});

test('16. Pin/Unpin', () => {
  mem.pin(ids.kinda_like);
  let entry = mem._store.get(ids.kinda_like)!;
  expect(entry.pinned).toBe(true);

  const wsBefore = entry.workingStrength;
  const csBefore = entry.coreStrength;
  mem.consolidate(5.0);

  entry = mem._store.get(ids.kinda_like)!;
  expect(entry.workingStrength).toBe(wsBefore);
  expect(entry.coreStrength).toBe(csBefore);

  mem.unpin(ids.kinda_like);
  mem.consolidate(1.0);
  entry = mem._store.get(ids.kinda_like)!;
  expect(
    entry.workingStrength < wsBefore || entry.coreStrength !== csBefore
  ).toBe(true);
});
