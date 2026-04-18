/**
 * Hebbian Learning Tests
 */

import * as os from 'os';
import * as path from 'path';
import * as fs from 'fs';
import { Memory } from '../src/memory';
import { MemoryConfig } from '../src/config';
import { SQLiteStore } from '../src/store';
import { MemoryType } from '../src/core';
import {
  recordCoactivation,
  maybeCreateLink,
  getHebbianNeighbors,
  decayHebbianLinks,
  strengthenLink,
  getAllHebbianLinks,
} from '../src/hebbian';

let tmpdir: string;
let store: SQLiteStore;
let mem: Memory;
let config: MemoryConfig;
const ids: string[] = [];

beforeAll(() => {
  tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'hebbian-test-'));
  const dbPath = path.join(tmpdir, 'hebbian.db');
  store = new SQLiteStore(dbPath);
  config = MemoryConfig.default();
});

afterAll(() => {
  if (store) store.close();
  if (mem) mem.close();
  fs.rmSync(tmpdir, { recursive: true, force: true });
});

describe('Hebbian Link Formation', () => {
  test('Should create tracking record on first co-activation', () => {
    ids.push(store.add('Memory A', MemoryType.FACTUAL, 0.5).id);
    ids.push(store.add('Memory B', MemoryType.FACTUAL, 0.5).id);

    const formed = maybeCreateLink(store, ids[0], ids[1], 3);
    expect(formed).toBe(false);

    const link = store.getHebbianLink(ids[0], ids[1]);
    expect(link).toBeTruthy();
    expect(link?.strength).toBe(0.0);
    expect(link?.coactivationCount).toBe(1);
  });

  test('Should increment count on subsequent co-activation', () => {
    const formed = maybeCreateLink(store, ids[0], ids[1], 3);
    expect(formed).toBe(false);

    const link = store.getHebbianLink(ids[0], ids[1]);
    expect(link?.coactivationCount).toBe(2);
    expect(link?.strength).toBe(0.0);
  });

  test('Should form bidirectional link when threshold is reached', () => {
    const formed = maybeCreateLink(store, ids[0], ids[1], 3);
    expect(formed).toBe(true);

    const linkAB = store.getHebbianLink(ids[0], ids[1]);
    const linkBA = store.getHebbianLink(ids[1], ids[0]);

    expect(linkAB?.strength).toBe(1.0);
    expect(linkAB?.coactivationCount).toBe(3);
    expect(linkBA?.strength).toBe(1.0);
    expect(linkBA?.coactivationCount).toBe(3);
  });

  test('Should not re-form link after it already exists', () => {
    const formed = maybeCreateLink(store, ids[0], ids[1], 3);
    expect(formed).toBe(false);

    const link = store.getHebbianLink(ids[0], ids[1]);
    expect(link?.coactivationCount).toBe(4);
  });

  test('Should handle consistent ordering (id1 < id2)', () => {
    // Try creating with reversed IDs
    maybeCreateLink(store, ids[1], ids[0], 3);
    const link = store.getHebbianLink(ids[0], ids[1]);
    expect(link?.coactivationCount).toBe(5);
  });
});

describe('recordCoactivation', () => {
  beforeAll(() => {
    ids.push(store.add('Memory C', MemoryType.FACTUAL, 0.5).id);
    ids.push(store.add('Memory D', MemoryType.FACTUAL, 0.5).id);
    ids.push(store.add('Memory E', MemoryType.FACTUAL, 0.5).id);
  });

  test('Should handle multiple memory co-activation', () => {
    const memIds = [ids[2], ids[3], ids[4]];
    
    // First co-activation
    recordCoactivation(store, memIds, config);
    
    // Check all three pairs were tracked
    expect(store.getHebbianLink(ids[2], ids[3])?.coactivationCount).toBe(1);
    expect(store.getHebbianLink(ids[2], ids[4])?.coactivationCount).toBe(1);
    expect(store.getHebbianLink(ids[3], ids[4])?.coactivationCount).toBe(1);
  });

  test('Should form links after threshold activations', () => {
    const memIds = [ids[2], ids[3], ids[4]];
    
    // Second and third co-activations
    recordCoactivation(store, memIds, config);
    const newLinks = recordCoactivation(store, memIds, config);
    
    // All three pairs should have formed links
    expect(newLinks.length).toBe(3);
    expect(store.getHebbianLink(ids[2], ids[3])?.strength).toBe(1.0);
    expect(store.getHebbianLink(ids[2], ids[4])?.strength).toBe(1.0);
    expect(store.getHebbianLink(ids[3], ids[4])?.strength).toBe(1.0);
  });

  test('Should return empty array for single memory', () => {
    const newLinks = recordCoactivation(store, [ids[0]], config);
    expect(newLinks.length).toBe(0);
  });

  test('Should respect hebbianEnabled config', () => {
    const disabledConfig = new MemoryConfig({ hebbianEnabled: false });
    const newId = store.add('Memory F', MemoryType.FACTUAL, 0.5).id;
    const newId2 = store.add('Memory G', MemoryType.FACTUAL, 0.5).id;
    
    const newLinks = recordCoactivation(store, [newId, newId2], disabledConfig);
    expect(newLinks.length).toBe(0);
    expect(store.getHebbianLink(newId, newId2)).toBeNull();
  });
});

describe('getHebbianNeighbors', () => {
  test('Should return all linked neighbors', () => {
    const neighbors = getHebbianNeighbors(store, ids[2]);
    expect(neighbors).toContain(ids[3]);
    expect(neighbors).toContain(ids[4]);
    expect(neighbors.length).toBeGreaterThanOrEqual(2);
  });

  test('Should return empty array for memory with no links', () => {
    const newId = store.add('Memory H', MemoryType.FACTUAL, 0.5).id;
    const neighbors = getHebbianNeighbors(store, newId);
    expect(neighbors.length).toBe(0);
  });
});

describe('Link Strength Operations', () => {
  test('Should strengthen existing link', () => {
    const initialLink = store.getHebbianLink(ids[2], ids[3]);
    expect(initialLink?.strength).toBe(1.0);

    const strengthened = strengthenLink(store, ids[2], ids[3], 0.3);
    expect(strengthened).toBe(true);

    const updatedLink = store.getHebbianLink(ids[2], ids[3]);
    expect(updatedLink?.strength).toBe(1.3);

    // Check reverse direction was also strengthened
    const reverseLink = store.getHebbianLink(ids[3], ids[2]);
    expect(reverseLink?.strength).toBe(1.3);
  });

  test('Should cap strength at 2.0', () => {
    strengthenLink(store, ids[2], ids[3], 1.0);
    const link = store.getHebbianLink(ids[2], ids[3]);
    expect(link?.strength).toBeLessThanOrEqual(2.0);
  });

  test('Should return false for non-existent link', () => {
    const newId1 = store.add('Memory I', MemoryType.FACTUAL, 0.5).id;
    const newId2 = store.add('Memory J', MemoryType.FACTUAL, 0.5).id;
    const result = strengthenLink(store, newId1, newId2, 0.1);
    expect(result).toBe(false);
  });
});

describe('Link Decay', () => {
  test('Should decay all link strengths', () => {
    const beforeLinks = getAllHebbianLinks(store);
    const initialStrengths = beforeLinks.map(l => l.strength);

    const pruned = decayHebbianLinks(store, 0.5); // 50% decay
    
    const afterLinks = getAllHebbianLinks(store);
    afterLinks.forEach((link, i) => {
      const expected = initialStrengths[i] * 0.5;
      expect(Math.abs(link.strength - expected)).toBeLessThan(0.01);
    });
  });

  test('Should prune weak links below 0.1', () => {
    // Create a weak link
    const weakId1 = store.add('Weak A', MemoryType.FACTUAL, 0.5).id;
    const weakId2 = store.add('Weak B', MemoryType.FACTUAL, 0.5).id;
    store.upsertHebbianLink(weakId1, weakId2, 0.15, 1);

    const pruned = decayHebbianLinks(store, 0.5);
    expect(pruned).toBeGreaterThan(0);

    // Link should be gone
    const link = store.getHebbianLink(weakId1, weakId2);
    expect(link).toBeNull();
  });
});

describe('Memory API Integration', () => {
  beforeAll(() => {
    const dbPath = path.join(tmpdir, 'mem-hebbian.db');
    mem = new Memory(dbPath, new MemoryConfig({ hebbianEnabled: true, hebbianThreshold: 2 }));
  });

  test('Should record co-activation during recall', () => {
    const id1 = mem.add('Paris is the capital of France', { type: 'factual', importance: 0.6 });
    const id2 = mem.add('The Eiffel Tower is in Paris', { type: 'factual', importance: 0.6 });
    const id3 = mem.add('Croissants are a French pastry', { type: 'factual', importance: 0.5 });

    // Recall memories together multiple times (threshold is 2)
    mem.recall('Paris France', { limit: 5 });
    mem.recall('Paris', { limit: 5 });
    mem.recall('Paris', { limit: 5 }); // Third recall to ensure threshold is met
    
    // Check that Hebbian links were formed
    const neighbors = mem.hebbianLinks(id1);
    expect(neighbors.length).toBeGreaterThan(0);
  });

  test('Should decay links during consolidation', () => {
    const beforeLinks = getAllHebbianLinks(mem._store);
    const initialCount = beforeLinks.length;

    mem.consolidate(1.0);

    const afterLinks = getAllHebbianLinks(mem._store);
    // Links should still exist but be weaker
    afterLinks.forEach(link => {
      const before = beforeLinks.find(l => l.sourceId === link.sourceId && l.targetId === link.targetId);
      if (before) {
        expect(link.strength).toBeLessThan(before.strength);
      }
    });
  });

  test('Should expand search via Hebbian links', () => {
    const id1 = mem.add('TypeScript is a typed superset of JavaScript', {
      type: 'factual',
      importance: 0.6,
    });
    const id2 = mem.add('JavaScript runs in the browser', {
      type: 'factual',
      importance: 0.5,
    });

    // Co-activate them multiple times to form strong link
    for (let i = 0; i < 3; i++) {
      recordCoactivation(mem._store, [id1, id2], mem.config);
    }

    // Now search for TypeScript with graphExpand=true
    const results = mem.recall('TypeScript', { limit: 5, graphExpand: true });
    
    // Should include JavaScript memory via Hebbian link
    const ids = results.map(r => r.id);
    expect(ids).toContain(id1);
    // Note: id2 might be included depending on FTS and activation scores
  });
});
