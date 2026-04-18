/**
 * Engram v2 Tests — Namespace, ACL, Emotional Bus, Subscriptions
 */

import * as os from 'os';
import * as path from 'path';
import * as fs from 'fs';
import { Memory } from '../src/memory';
import { MemoryConfig } from '../src/config';
import { Permission } from '../src/types';

let tmpdir: string;
let workspaceDir: string;

beforeAll(() => {
  tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'engram-v2-test-'));
  workspaceDir = path.join(tmpdir, 'workspace');
  fs.mkdirSync(workspaceDir, { recursive: true });
  
  // Create test workspace files
  fs.writeFileSync(
    path.join(workspaceDir, 'SOUL.md'),
    `# Core Drives
curiosity: Always seek to understand new things
helpfulness: Assist the user effectively

# Values
- Be honest and direct
- Learn from mistakes
`,
  );

  fs.writeFileSync(
    path.join(workspaceDir, 'HEARTBEAT.md'),
    `# Tasks
- [ ] Check emails
- [x] Review calendar
- [ ] Run consolidation
`,
  );

  fs.writeFileSync(
    path.join(workspaceDir, 'IDENTITY.md'),
    `name: TestAgent
creature: AI Assistant
vibe: helpful and curious
emoji: 🤖
`,
  );
});

afterAll(() => {
  fs.rmSync(tmpdir, { recursive: true, force: true });
});

test('Namespace support - basic operations', () => {
  const dbPath = path.join(tmpdir, 'namespace-test.db');
  const mem = new Memory(dbPath, MemoryConfig.default());

  // Add memories to different namespaces
  const id1 = mem.addToNamespace('Memory in default namespace', {
    type: 'factual',
    namespace: 'default',
  });

  const id2 = mem.addToNamespace('Memory in trading namespace', {
    type: 'factual',
    namespace: 'trading',
  });

  const id3 = mem.addToNamespace('Another trading memory', {
    type: 'factual',
    namespace: 'trading',
  });

  // Verify namespace isolation
  expect(id1).toBeTruthy();
  expect(id2).toBeTruthy();
  expect(id3).toBeTruthy();

  mem.close();
});

test('ACL - Permission management', () => {
  const dbPath = path.join(tmpdir, 'acl-test.db');
  const mem = new Memory(dbPath, MemoryConfig.default());

  mem.setAgentId('ceo');

  // Grant permissions
  mem.grant('trader', 'trading', Permission.WRITE);
  mem.grant('analyst', 'trading', Permission.READ);
  mem.grant('admin', '*', Permission.ADMIN);

  // Check permissions
  expect(mem.checkPermission('trader', 'trading', Permission.READ)).toBe(true);
  expect(mem.checkPermission('trader', 'trading', Permission.WRITE)).toBe(true);
  expect(mem.checkPermission('trader', 'trading', Permission.ADMIN)).toBe(false);

  expect(mem.checkPermission('analyst', 'trading', Permission.READ)).toBe(true);
  expect(mem.checkPermission('analyst', 'trading', Permission.WRITE)).toBe(false);

  expect(mem.checkPermission('admin', 'trading', Permission.ADMIN)).toBe(true);
  expect(mem.checkPermission('admin', 'anything', Permission.ADMIN)).toBe(true);

  // Revoke permission
  const revoked = mem.revoke('trader', 'trading');
  expect(revoked).toBe(true);
  expect(mem.checkPermission('trader', 'trading', Permission.READ)).toBe(false);

  // List permissions
  const permissions = mem.listPermissions('admin');
  expect(permissions.length).toBe(1);
  expect(permissions[0].namespace).toBe('*');
  expect(permissions[0].permission).toBe(Permission.ADMIN);

  mem.close();
});

test('Emotional Bus - Drive alignment', () => {
  const dbPath = path.join(tmpdir, 'bus-alignment-test.db');
  const mem = Memory.withEmotionalBus(dbPath, workspaceDir, MemoryConfig.default());

  expect(mem.emotionalBus).toBeTruthy();

  const drives = mem.emotionalBus!.getDrives();
  expect(drives.length).toBeGreaterThan(0);
  expect(drives.some(d => d.name === 'curiosity')).toBe(true);

  // Test importance alignment
  const aligned = 'I want to understand and learn new concepts deeply';
  const boost = mem.emotionalBus!.alignImportance(aligned);
  expect(boost).toBeGreaterThan(1.0);

  const unaligned = 'xyz abc 123 random text';
  const noBoost = mem.emotionalBus!.alignImportance(unaligned);
  expect(noBoost).toBe(1.0);

  // Test alignment score
  const score = mem.emotionalBus!.alignmentScore(aligned);
  expect(score).toBeGreaterThanOrEqual(0.5);

  // Test finding aligned drives
  const foundDrives = mem.emotionalBus!.findAligned(aligned);
  expect(foundDrives.length).toBeGreaterThan(0);
  expect(foundDrives.some(([name]) => name === 'curiosity')).toBe(true);

  mem.close();
});

test('Emotional Bus - Emotion tracking', () => {
  const dbPath = path.join(tmpdir, 'bus-emotion-test.db');
  const mem = Memory.withEmotionalBus(dbPath, workspaceDir, MemoryConfig.default());

  // Record some emotional interactions
  mem.emotionalBus!.processInteraction('good coding session', 0.8, 'coding');
  mem.emotionalBus!.processInteraction('great problem solving', 0.7, 'coding');
  mem.emotionalBus!.processInteraction('frustrating bug', -0.6, 'debugging');

  const trends = mem.emotionalBus!.getTrends();
  expect(trends.length).toBe(2);

  const codingTrend = trends.find(t => t.domain === 'coding');
  expect(codingTrend).toBeTruthy();
  expect(codingTrend!.count).toBe(2);
  expect(codingTrend!.valence).toBeGreaterThan(0);

  const debuggingTrend = trends.find(t => t.domain === 'debugging');
  expect(debuggingTrend).toBeTruthy();
  expect(debuggingTrend!.valence).toBeLessThan(0);

  mem.close();
});

test('Emotional Bus - Behavior feedback', () => {
  const dbPath = path.join(tmpdir, 'bus-behavior-test.db');
  const mem = Memory.withEmotionalBus(dbPath, workspaceDir, MemoryConfig.default());

  // Log some behavior outcomes
  mem.emotionalBus!.logBehavior('check_email', true);
  mem.emotionalBus!.logBehavior('check_email', true);
  mem.emotionalBus!.logBehavior('check_email', false);

  mem.emotionalBus!.logBehavior('bad_action', false);
  mem.emotionalBus!.logBehavior('bad_action', false);
  mem.emotionalBus!.logBehavior('bad_action', false);

  const stats = mem.emotionalBus!.getBehaviorStats();
  expect(stats.length).toBe(2);

  const emailStats = stats.find(s => s.action === 'check_email');
  expect(emailStats).toBeTruthy();
  expect(emailStats!.total).toBe(3);
  expect(emailStats!.positive).toBe(2);
  expect(emailStats!.score).toBeCloseTo(2 / 3, 2);

  const badStats = stats.find(s => s.action === 'bad_action');
  expect(badStats).toBeTruthy();
  expect(badStats!.score).toBe(0);

  mem.close();
});

test('Emotional Bus - SOUL suggestions', () => {
  const dbPath = path.join(tmpdir, 'bus-soul-test.db');
  const mem = Memory.withEmotionalBus(dbPath, workspaceDir, MemoryConfig.default());

  // Record many negative interactions in a domain
  for (let i = 0; i < 15; i++) {
    mem.emotionalBus!.processInteraction('bad debugging experience', -0.8, 'debugging');
  }

  const suggestions = mem.emotionalBus!.suggestSoulUpdates();
  expect(suggestions.length).toBeGreaterThan(0);
  expect(suggestions.some(s => s.domain === 'debugging')).toBe(true);

  mem.close();
});

test('Emotional Bus - HEARTBEAT suggestions', () => {
  const dbPath = path.join(tmpdir, 'bus-heartbeat-test.db');
  const mem = Memory.withEmotionalBus(dbPath, workspaceDir, MemoryConfig.default());

  // Log many failures for an action
  for (let i = 0; i < 15; i++) {
    mem.emotionalBus!.logBehavior('useless_check', false);
  }

  const suggestions = mem.emotionalBus!.suggestHeartbeatUpdates();
  expect(suggestions.length).toBeGreaterThan(0);
  expect(suggestions.some(s => s.action === 'useless_check')).toBe(true);
  expect(suggestions.some(s => s.suggestion === 'deprioritize')).toBe(true);

  mem.close();
});

test('Subscriptions - Basic subscribe/unsubscribe', () => {
  const dbPath = path.join(tmpdir, 'sub-test.db');
  const mem = new Memory(dbPath, MemoryConfig.default());

  // Subscribe
  mem.subscribe('ceo', 'trading', 0.8);

  const subs = mem.listSubscriptions('ceo');
  expect(subs.length).toBe(1);
  expect(subs[0].namespace).toBe('trading');
  expect(subs[0].minImportance).toBeCloseTo(0.8, 2);

  // Unsubscribe
  const removed = mem.unsubscribe('ceo', 'trading');
  expect(removed).toBe(true);

  const subsAfter = mem.listSubscriptions('ceo');
  expect(subsAfter.length).toBe(0);

  mem.close();
});

test('Subscriptions - Notifications', () => {
  const dbPath = path.join(tmpdir, 'notif-test.db');
  const mem = new Memory(dbPath, MemoryConfig.default());

  // Subscribe to trading namespace with threshold 0.7
  mem.subscribe('ceo', 'trading', 0.7);

  // Add a high-importance memory
  mem.addToNamespace('Oil price spike detected', {
    type: 'factual',
    importance: 0.9,
    namespace: 'trading',
  });

  // Check notifications
  const notifs = mem.checkNotifications('ceo');
  expect(notifs.length).toBe(1);
  expect(notifs[0].namespace).toBe('trading');
  expect(notifs[0].importance).toBeCloseTo(0.9, 2);

  // Check again - should be empty (cursor updated)
  const notifsAgain = mem.checkNotifications('ceo');
  expect(notifsAgain.length).toBe(0);

  mem.close();
});

test('Subscriptions - Wildcard namespace', () => {
  const dbPath = path.join(tmpdir, 'wildcard-test.db');
  const mem = new Memory(dbPath, MemoryConfig.default());

  // Subscribe to all namespaces
  mem.subscribe('ceo', '*', 0.8);

  // Add memories to different namespaces
  mem.addToNamespace('Trading alert', {
    type: 'factual',
    importance: 0.9,
    namespace: 'trading',
  });

  mem.addToNamespace('Engine alert', {
    type: 'factual',
    importance: 0.85,
    namespace: 'engine',
  });

  const notifs = mem.checkNotifications('ceo');
  expect(notifs.length).toBe(2);

  mem.close();
});

test('Subscriptions - Peek notifications', () => {
  const dbPath = path.join(tmpdir, 'peek-test.db');
  const mem = new Memory(dbPath, MemoryConfig.default());

  mem.subscribe('ceo', 'trading', 0.7);

  mem.addToNamespace('Test memory', {
    type: 'factual',
    importance: 0.9,
    namespace: 'trading',
  });

  // Peek should not update cursor
  const peekNotifs = mem.peekNotifications('ceo');
  expect(peekNotifs.length).toBe(1);

  // Peek again - should still return same results
  const peekAgain = mem.peekNotifications('ceo');
  expect(peekAgain.length).toBe(1);

  // Now check (updates cursor)
  const checkNotifs = mem.checkNotifications('ceo');
  expect(checkNotifs.length).toBe(1);

  // Check again - empty
  const checkAgain = mem.checkNotifications('ceo');
  expect(checkAgain.length).toBe(0);

  mem.close();
});

test('Emotional Bus - Workspace file reading', () => {
  const dbPath = path.join(tmpdir, 'workspace-test.db');
  const mem = Memory.withEmotionalBus(dbPath, workspaceDir, MemoryConfig.default());

  const identity = mem.emotionalBus!.getIdentity();
  expect(identity.name).toBe('TestAgent');
  expect(identity.creature).toBe('AI Assistant');
  expect(identity.emoji).toBe('🤖');

  const tasks = mem.emotionalBus!.getHeartbeatTasks();
  expect(tasks.length).toBe(3);
  expect(tasks[0].description).toBe('Check emails');
  expect(tasks[0].completed).toBe(false);
  expect(tasks[1].completed).toBe(true);

  mem.close();
});

test('Integration - Add memory with emotion and namespace', () => {
  const dbPath = path.join(tmpdir, 'integration-test.db');
  const mem = Memory.withEmotionalBus(dbPath, workspaceDir, MemoryConfig.default());

  const memId = mem.addWithEmotion('Great debugging session today', {
    type: 'episodic',
    importance: 0.6,
    namespace: 'coding',
    emotion: 0.9,
    domain: 'debugging',
  });

  expect(memId).toBeTruthy();

  // Check that emotion was recorded
  const trends = mem.emotionalBus!.getTrends();
  const debuggingTrend = trends.find(t => t.domain === 'debugging');
  expect(debuggingTrend).toBeTruthy();
  expect(debuggingTrend!.valence).toBeCloseTo(0.9, 2);

  mem.close();
});

test('Backward compatibility - v1 API still works', () => {
  const dbPath = path.join(tmpdir, 'compat-test.db');
  const mem = new Memory(dbPath, MemoryConfig.default());

  // v1 API - add without namespace
  const id1 = mem.add('Test memory', { type: 'factual', importance: 0.5 });
  expect(id1).toBeTruthy();

  // v1 API - recall
  const results = mem.recall('test', { limit: 5 });
  expect(results.length).toBeGreaterThan(0);

  // v1 API - consolidate
  mem.consolidate(1.0);

  // v1 API - stats
  const stats = mem.stats();
  expect(stats.total_memories).toBeGreaterThan(0);

  mem.close();
});
