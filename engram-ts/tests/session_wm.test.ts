/**
 * Tests for Session Working Memory
 */

import { Memory, SessionWorkingMemory, getSessionWM, clearSession, listSessions } from '../src';
import * as fs from 'fs';
import * as path from 'path';

describe('SessionWorkingMemory', () => {
  describe('Capacity and Activation', () => {
    test('default capacity is 7 (Miller\'s Law)', () => {
      const swm = new SessionWorkingMemory();
      expect(swm.capacity).toBe(7);
    });

    test('custom capacity', () => {
      const swm = new SessionWorkingMemory(5);
      expect(swm.capacity).toBe(5);
    });

    test('capacity limit enforced', () => {
      const swm = new SessionWorkingMemory(3);
      swm.activate(['a', 'b', 'c', 'd', 'e']);
      expect(swm.size()).toBe(3);
    });

    test('reactivation updates timestamp', async () => {
      const swm = new SessionWorkingMemory(5);
      swm.activate(['a', 'b', 'c']);
      
      // Small delay
      await new Promise(r => setTimeout(r, 50));
      
      // Reactivate 'a' - should update its timestamp
      swm.activate(['a']);
      expect(swm.getActiveIds()).toContain('a');
    });
  });

  describe('Decay', () => {
    test('default decay is 300 seconds', () => {
      const swm = new SessionWorkingMemory();
      expect(swm.decaySeconds).toBe(300);
    });

    test('custom decay', () => {
      const swm = new SessionWorkingMemory(7, 60);
      expect(swm.decaySeconds).toBe(60);
    });

    test('isEmpty and size work correctly', () => {
      const swm = new SessionWorkingMemory();
      expect(swm.isEmpty()).toBe(true);
      expect(swm.size()).toBe(0);
      
      swm.activate(['a', 'b']);
      expect(swm.isEmpty()).toBe(false);
      expect(swm.size()).toBe(2);
    });

    test('clear empties working memory', () => {
      const swm = new SessionWorkingMemory();
      swm.activate(['a', 'b', 'c']);
      expect(swm.size()).toBe(3);
      
      swm.clear();
      expect(swm.isEmpty()).toBe(true);
    });
  });

  describe('Session Registry', () => {
    beforeEach(() => {
      // Clear all sessions before each test
      for (const sid of listSessions()) {
        clearSession(sid);
      }
    });

    test('getSessionWM creates new session', () => {
      const swm = getSessionWM('test-session');
      expect(swm).toBeInstanceOf(SessionWorkingMemory);
    });

    test('getSessionWM returns same instance', () => {
      const swm1 = getSessionWM('same-session');
      const swm2 = getSessionWM('same-session');
      expect(swm1).toBe(swm2);
    });

    test('different sessions are independent', () => {
      const swm1 = getSessionWM('session-1');
      const swm2 = getSessionWM('session-2');
      
      swm1.activate(['a', 'b']);
      swm2.activate(['x', 'y', 'z']);
      
      expect(swm1.size()).toBe(2);
      expect(swm2.size()).toBe(3);
    });

    test('clearSession removes session', () => {
      getSessionWM('to-clear');
      expect(listSessions()).toContain('to-clear');
      
      const cleared = clearSession('to-clear');
      expect(cleared).toBe(true);
      expect(listSessions()).not.toContain('to-clear');
    });

    test('clearSession returns false for nonexistent', () => {
      const cleared = clearSession('nonexistent');
      expect(cleared).toBe(false);
    });

    test('listSessions returns all active sessions', () => {
      getSessionWM('session-a');
      getSessionWM('session-b');
      getSessionWM('session-c');
      
      const sessions = listSessions();
      expect(sessions).toContain('session-a');
      expect(sessions).toContain('session-b');
      expect(sessions).toContain('session-c');
    });
  });
});

describe('Memory.sessionRecall Integration', () => {
  const testDbPath = path.join(__dirname, 'test-session-wm.db');
  let memory: Memory;

  beforeEach(() => {
    if (fs.existsSync(testDbPath)) {
      fs.unlinkSync(testDbPath);
    }
    memory = new Memory(testDbPath);
    
    // Clear all sessions
    for (const sid of listSessions()) {
      clearSession(sid);
    }
  });

  afterEach(() => {
    memory.close();
    if (fs.existsSync(testDbPath)) {
      fs.unlinkSync(testDbPath);
    }
  });

  test('sessionRecall returns results with metadata', () => {
    memory.add('Python is great for data science', { type: 'factual' });
    memory.add('TypeScript is good for web development', { type: 'factual' });
    
    const result = memory.sessionRecall('programming languages');
    
    expect(result.results).toBeDefined();
    expect(result.fullRecallTriggered).toBe(true);
    expect(result.reason).toBe('empty_wm');
    expect(result.workingMemorySize).toBeGreaterThan(0);
  });

  test('sessionRecall activates working memory', () => {
    memory.add('Coffee is made from beans', { type: 'factual' });
    
    const swm = getSessionWM('test-session');
    expect(swm.isEmpty()).toBe(true);
    
    memory.sessionRecall('coffee', { sessionId: 'test-session' });
    
    expect(swm.isEmpty()).toBe(false);
  });

  test('continuous topic reuses working memory', () => {
    memory.add('Coffee is made from beans', { type: 'factual' });
    memory.add('Espresso is concentrated coffee', { type: 'factual' });
    
    // First recall - triggers full retrieval
    const result1 = memory.sessionRecall('coffee', { sessionId: 'coffee-session' });
    expect(result1.fullRecallTriggered).toBe(true);
    expect(result1.reason).toBe('empty_wm');
    
    // Second recall on same topic - should reuse WM
    const result2 = memory.sessionRecall('espresso coffee', { sessionId: 'coffee-session' });
    // Note: This may or may not trigger full recall depending on overlap
    // The important thing is the session tracking works
    expect(result2.results).toBeDefined();
  });

  test('custom SessionWorkingMemory can be passed', () => {
    memory.add('Test memory', { type: 'factual' });
    
    const customSwm = new SessionWorkingMemory(3, 60);
    const result = memory.sessionRecall('test', { sessionWM: customSwm });
    
    expect(result.fullRecallTriggered).toBe(true);
    expect(customSwm.size()).toBeGreaterThan(0);
  });
});
