/**
 * State Management Tests
 */

import { jest, describe, test, expect, beforeEach } from '@jest/globals';
import { State } from '../js/state.js';
import { SessionHistory } from '../js/sessionHistory.js';

describe('State', () => {
  let state;

  beforeEach(() => {
    state = new State();
  });

  describe('initialization', () => {
    test('should initialize with default values', () => {
      expect(state.connected).toBe(false);
      expect(state.recording).toBe(false);
      expect(state.playing).toBe(false);
      expect(state.transcript).toBe('');
      expect(state.interimTranscript).toBe('');
    });

    test('should have currentTab property', () => {
      expect(state.currentTab).toBeDefined();
      expect(state.currentTab).toBe('home');
    });

    test('should have sidebarCollapsed property', () => {
      expect(state.sidebarCollapsed).toBeDefined();
      expect(state.sidebarCollapsed).toBe(false);
    });

    test('should have environment property', () => {
      expect(state.environment).toBeDefined();
      expect(state.environment).toBe('development');
    });
  });

  describe('connection state', () => {
    test('should track connection status', () => {
      state.connected = true;
      expect(state.connected).toBe(true);
    });

    test('should track connection details', () => {
      state.connectionDetails = {
        serverUrl: 'wss://localhost:3001/ws',
        connectedAt: Date.now(),
        latency: 50,
      };
      expect(state.connectionDetails.serverUrl).toBe('wss://localhost:3001/ws');
    });

    test('should track reconnection attempts', () => {
      state.reconnectAttempts = 3;
      expect(state.reconnectAttempts).toBe(3);
    });
  });

  describe('STT state', () => {
    test('should track STT start time', () => {
      const now = Date.now();
      state.sttStartTime = now;
      expect(state.sttStartTime).toBe(now);
      expect(state.sttFirstResult).toBe(false);
    });

    test('should track STT first result', () => {
      state.sttFirstResult = true;
      expect(state.sttFirstResult).toBe(true);
    });

    test('should track STT provider', () => {
      state.sttProvider = 'deepgram';
      expect(state.sttProvider).toBe('deepgram');
    });
  });

  describe('TTS state', () => {
    test('should track TTS start time', () => {
      const now = Date.now();
      state.ttsStartTime = now;
      expect(state.ttsStartTime).toBe(now);
      expect(state.ttsFirstAudio).toBe(false);
    });

    test('should track TTS queue', () => {
      state.ttsQueue = ['Hello', 'World'];
      expect(state.ttsQueue).toHaveLength(2);
    });
  });

  describe('reset', () => {
    test('should reset all state', () => {
      state.connected = true;
      state.recording = true;
      state.transcript = 'Hello';
      state.currentTab = 'stt';

      state.reset();

      expect(state.connected).toBe(false);
      expect(state.recording).toBe(false);
      expect(state.transcript).toBe('');
      expect(state.currentTab).toBe('home');
    });
  });

  describe('persistence', () => {
    test('should save preferences to localStorage', () => {
      state.sidebarCollapsed = true;
      state.savePreferences();

      // Verify localStorage was called with the right key
      const stored = localStorage.getItem('waav_preferences');
      expect(stored).toBeTruthy();
      const parsed = JSON.parse(stored);
      expect(parsed.sidebarCollapsed).toBe(true);
    });

    test('should load preferences from localStorage', () => {
      // Set up localStorage with test data
      localStorage.setItem('waav_preferences', JSON.stringify({
        sidebarCollapsed: true,
        theme: 'dark',
        environment: 'production',
      }));

      // Create fresh state that will load from localStorage
      const newState = new State();

      expect(newState.sidebarCollapsed).toBe(true);
      expect(newState.theme).toBe('dark');
      expect(newState.environment).toBe('production');
    });
  });

  describe('state change events', () => {
    test('should emit events on state change', () => {
      const listener = jest.fn();
      state.on('connectionChange', listener);

      state.setConnected(true);

      expect(listener).toHaveBeenCalledWith(true);
    });

    test('should support multiple listeners', () => {
      const listener1 = jest.fn();
      const listener2 = jest.fn();

      state.on('tabChange', listener1);
      state.on('tabChange', listener2);

      state.setCurrentTab('stt');

      // tabChange emits (newTab, oldTab)
      expect(listener1).toHaveBeenCalledWith('stt', 'home');
      expect(listener2).toHaveBeenCalledWith('stt', 'home');
    });

    test('should support removing listeners', () => {
      const listener = jest.fn();
      state.on('connectionChange', listener);
      state.off('connectionChange', listener);

      state.setConnected(true);

      expect(listener).not.toHaveBeenCalled();
    });
  });
});

describe('SessionHistory', () => {
  let sessionHistory;

  beforeEach(() => {
    sessionHistory = new SessionHistory();
  });

  describe('session creation', () => {
    test('should create a new session', async () => {
      const session = await sessionHistory.createSession({
        type: 'stt',
        provider: 'deepgram',
        config: { language: 'en-US' },
      });

      expect(session.id).toBeDefined();
      expect(session.type).toBe('stt');
      expect(session.provider).toBe('deepgram');
      expect(session.startTime).toBeDefined();
    });

    test('should generate unique session IDs', async () => {
      const session1 = await sessionHistory.createSession({ type: 'stt' });
      const session2 = await sessionHistory.createSession({ type: 'stt' });

      expect(session1.id).not.toBe(session2.id);
    });
  });

  describe('session updates', () => {
    test('should update session transcript', async () => {
      const session = await sessionHistory.createSession({ type: 'stt' });

      await sessionHistory.updateSession(session.id, {
        transcript: 'Hello world',
        isFinal: true,
      });

      const updated = await sessionHistory.getSession(session.id);
      expect(updated.transcript).toBe('Hello world');
    });

    test('should track session metrics', async () => {
      const session = await sessionHistory.createSession({ type: 'stt' });

      await sessionHistory.updateMetrics(session.id, {
        ttft: 150,
        duration: 5000,
      });

      const updated = await sessionHistory.getSession(session.id);
      expect(updated.metrics.ttft).toBe(150);
    });
  });

  describe('session retrieval', () => {
    test('should list all sessions', async () => {
      await sessionHistory.createSession({ type: 'stt' });
      await sessionHistory.createSession({ type: 'tts' });

      const sessions = await sessionHistory.listSessions();
      expect(sessions).toHaveLength(2);
    });

    test('should filter sessions by type', async () => {
      await sessionHistory.createSession({ type: 'stt' });
      await sessionHistory.createSession({ type: 'tts' });
      await sessionHistory.createSession({ type: 'stt' });

      const sttSessions = await sessionHistory.listSessions({ type: 'stt' });
      expect(sttSessions).toHaveLength(2);
    });

    test('should filter sessions by date range', async () => {
      const yesterday = Date.now() - 86400000;
      const today = Date.now();

      await sessionHistory.createSession({ type: 'stt', startTime: yesterday });
      await sessionHistory.createSession({ type: 'stt', startTime: today });

      const sessions = await sessionHistory.listSessions({
        startDate: today - 3600000,
        endDate: today + 3600000,
      });

      expect(sessions).toHaveLength(1);
    });
  });

  describe('session deletion', () => {
    test('should delete a session', async () => {
      const session = await sessionHistory.createSession({ type: 'stt' });
      await sessionHistory.deleteSession(session.id);

      const deleted = await sessionHistory.getSession(session.id);
      expect(deleted).toBeNull();
    });

    test('should clear all sessions', async () => {
      await sessionHistory.createSession({ type: 'stt' });
      await sessionHistory.createSession({ type: 'tts' });

      await sessionHistory.clearAll();

      const sessions = await sessionHistory.listSessions();
      expect(sessions).toHaveLength(0);
    });
  });

  describe('session export', () => {
    test('should export session as JSON', async () => {
      const session = await sessionHistory.createSession({
        type: 'stt',
        transcript: 'Hello world',
      });

      const json = await sessionHistory.exportSession(session.id, 'json');
      const parsed = JSON.parse(json);

      expect(parsed.type).toBe('stt');
      expect(parsed.transcript).toBe('Hello world');
    });

    test('should export transcript as text', async () => {
      const session = await sessionHistory.createSession({
        type: 'stt',
        transcript: 'Hello world',
      });

      const text = await sessionHistory.exportSession(session.id, 'text');
      expect(text).toBe('Hello world');
    });
  });
});
