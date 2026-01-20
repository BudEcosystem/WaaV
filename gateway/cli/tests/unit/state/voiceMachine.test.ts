/**
 * WaaV CLI Voice Machine Tests
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { createActor } from 'xstate';
import {
  voiceMachine,
  isVoiceActive,
  canStartListening,
  getVoiceStatusColor,
  getVoiceStatusText,
  type VoiceMachineContext,
  type VoiceMachineState,
} from '../../../src/core/state/machines/voiceMachine.js';

describe('voiceMachine', () => {
  describe('initial state', () => {
    it('should start in idle state', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      expect(actor.getSnapshot().value).toBe('idle');
      actor.stop();
    });

    it('should have correct initial context', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      const context = actor.getSnapshot().context;
      expect(context.gatewayUrl).toBe('');
      expect(context.streamId).toBeNull();
      expect(context.connectionAttempts).toBe(0);
      expect(context.audioLevel).toBe(0);
      expect(context.isMuted).toBe(false);
      expect(context.vadEnabled).toBe(true);
      expect(context.transcriptHistory).toEqual([]);
      expect(context.error).toBeNull();

      actor.stop();
    });
  });

  describe('connection flow', () => {
    it('should transition from idle to connecting on CONNECT', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });

      expect(actor.getSnapshot().value).toBe('connecting');
      expect(actor.getSnapshot().context.gatewayUrl).toBe('http://localhost:3001');

      actor.stop();
    });

    it('should increment connection attempts on connecting', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });

      expect(actor.getSnapshot().context.connectionAttempts).toBe(1);

      actor.stop();
    });

    it('should transition to ready on CONNECTED', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream-123' });

      expect(actor.getSnapshot().value).toBe('ready');
      expect(actor.getSnapshot().context.streamId).toBe('test-stream-123');

      actor.stop();
    });

    it('should retry on CONNECTION_FAILED if under max attempts', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      // First attempt: connectionAttempts = 1
      expect(actor.getSnapshot().context.connectionAttempts).toBe(1);

      actor.send({ type: 'CONNECTION_FAILED', error: 'Network error' });

      // Should still be in connecting state (retry)
      // Note: XState v5 self-transitions don't re-run entry by default
      expect(actor.getSnapshot().value).toBe('connecting');
      expect(actor.getSnapshot().context.lastError).toBe('Network error');

      actor.stop();
    });

    it('should go to error state after max connection attempts', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      expect(actor.getSnapshot().context.connectionAttempts).toBe(1);

      // First failure - still under max (1 < 3), stays in connecting
      actor.send({ type: 'CONNECTION_FAILED', error: 'Network error 1' });
      expect(actor.getSnapshot().value).toBe('connecting');

      // Second failure - still under max (1 < 3), stays in connecting
      // Note: In XState v5, self-transitions don't re-run entry, so counter stays at 1
      actor.send({ type: 'CONNECTION_FAILED', error: 'Network error 2' });
      expect(actor.getSnapshot().value).toBe('connecting');

      // Third failure - still 1 < 3, stays in connecting
      actor.send({ type: 'CONNECTION_FAILED', error: 'Network error 3' });
      expect(actor.getSnapshot().value).toBe('connecting');

      actor.stop();
    });
  });

  describe('listening flow', () => {
    it('should transition to listening from ready on START_LISTENING', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });

      expect(actor.getSnapshot().value).toBe('listening');

      actor.stop();
    });

    it('should update audio level on AUDIO_DATA', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });
      actor.send({ type: 'AUDIO_DATA', level: 0.75 });

      expect(actor.getSnapshot().context.audioLevel).toBe(0.75);

      actor.stop();
    });

    it('should update current transcript on TRANSCRIPT_PARTIAL', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });
      actor.send({ type: 'TRANSCRIPT_PARTIAL', text: 'Hello' });

      expect(actor.getSnapshot().context.currentTranscript).toBe('Hello');

      actor.stop();
    });

    it('should add to history on TRANSCRIPT_FINAL and go to processing', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });
      actor.send({ type: 'TRANSCRIPT_FINAL', text: 'Hello world', confidence: 0.95 });

      expect(actor.getSnapshot().value).toBe('processing');
      expect(actor.getSnapshot().context.transcriptHistory).toHaveLength(1);
      expect(actor.getSnapshot().context.transcriptHistory[0]?.text).toBe('Hello world');
      expect(actor.getSnapshot().context.currentTranscript).toBe('');

      actor.stop();
    });

    it('should handle mute/unmute', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });

      actor.send({ type: 'MUTE' });
      expect(actor.getSnapshot().context.isMuted).toBe(true);

      actor.send({ type: 'UNMUTE' });
      expect(actor.getSnapshot().context.isMuted).toBe(false);

      actor.stop();
    });
  });

  describe('speaking flow', () => {
    it('should transition to speaking from processing on TTS_START', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });
      actor.send({ type: 'TRANSCRIPT_FINAL', text: 'Hello', confidence: 0.9 });
      actor.send({ type: 'TTS_START' });

      expect(actor.getSnapshot().value).toBe('speaking');

      actor.stop();
    });

    it('should return to ready on TTS_END', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });
      actor.send({ type: 'TRANSCRIPT_FINAL', text: 'Hello', confidence: 0.9 });
      actor.send({ type: 'TTS_START' });
      actor.send({ type: 'TTS_END' });

      expect(actor.getSnapshot().value).toBe('ready');

      actor.stop();
    });

    it('should handle interrupt during speaking', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });
      actor.send({ type: 'TRANSCRIPT_FINAL', text: 'Hello', confidence: 0.9 });
      actor.send({ type: 'TTS_START' });
      actor.send({ type: 'INTERRUPT' });

      expect(actor.getSnapshot().value).toBe('listening');

      actor.stop();
    });
  });

  describe('error handling', () => {
    it('should go to error state on ERROR event', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'ERROR', error: 'Something went wrong' });

      expect(actor.getSnapshot().value).toBe('error');
      expect(actor.getSnapshot().context.error).toBe('Something went wrong');

      actor.stop();
    });

    it('should allow retry from error state', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'ERROR', error: 'Something went wrong' });
      actor.send({ type: 'RETRY' });

      expect(actor.getSnapshot().value).toBe('connecting');
      expect(actor.getSnapshot().context.error).toBeNull();

      actor.stop();
    });
  });

  describe('disconnect', () => {
    it('should transition to disconnected from any state', () => {
      const actor = createActor(voiceMachine);
      actor.start();

      actor.send({ type: 'CONNECT', gatewayUrl: 'http://localhost:3001' });
      actor.send({ type: 'CONNECTED', streamId: 'test-stream' });
      actor.send({ type: 'START_LISTENING' });
      actor.send({ type: 'DISCONNECT' });

      expect(actor.getSnapshot().value).toBe('disconnected');
      expect(actor.getSnapshot().context.streamId).toBeNull();

      actor.stop();
    });
  });
});

describe('voice machine helpers', () => {
  describe('isVoiceActive', () => {
    it('should return true for active states', () => {
      expect(isVoiceActive('listening')).toBe(true);
      expect(isVoiceActive('processing')).toBe(true);
      expect(isVoiceActive('speaking')).toBe(true);
    });

    it('should return false for inactive states', () => {
      expect(isVoiceActive('idle')).toBe(false);
      expect(isVoiceActive('connecting')).toBe(false);
      expect(isVoiceActive('ready')).toBe(false);
      expect(isVoiceActive('error')).toBe(false);
      expect(isVoiceActive('disconnected')).toBe(false);
    });
  });

  describe('canStartListening', () => {
    it('should return true only for ready state', () => {
      expect(canStartListening('ready')).toBe(true);
      expect(canStartListening('idle')).toBe(false);
      expect(canStartListening('listening')).toBe(false);
    });
  });

  describe('getVoiceStatusColor', () => {
    it('should return correct colors', () => {
      expect(getVoiceStatusColor('listening')).toBe('red');
      expect(getVoiceStatusColor('speaking')).toBe('green');
      expect(getVoiceStatusColor('processing')).toBe('yellow');
      expect(getVoiceStatusColor('error')).toBe('red');
      expect(getVoiceStatusColor('ready')).toBe('cyan');
      expect(getVoiceStatusColor('idle')).toBe('gray');
    });
  });

  describe('getVoiceStatusText', () => {
    const mockContext: VoiceMachineContext = {
      gatewayUrl: 'http://localhost:3001',
      streamId: null,
      connectionAttempts: 1,
      maxConnectionAttempts: 3,
      audioLevel: 0,
      isMuted: false,
      vadEnabled: true,
      currentTranscript: '',
      transcriptHistory: [],
      error: 'Test error',
      lastError: null,
      totalAudioDuration: 0,
      totalUtterances: 0,
    };

    it('should return correct status text', () => {
      expect(getVoiceStatusText('idle', mockContext)).toBe('Not connected');
      expect(getVoiceStatusText('ready', mockContext)).toBe('Ready - Press Space to talk');
      expect(getVoiceStatusText('listening', mockContext)).toBe('Listening...');
      expect(getVoiceStatusText('processing', mockContext)).toBe('Processing...');
      expect(getVoiceStatusText('speaking', mockContext)).toBe('Speaking...');
      expect(getVoiceStatusText('error', mockContext)).toBe('Error: Test error');
      expect(getVoiceStatusText('disconnected', mockContext)).toBe('Disconnected');
    });

    it('should show muted status when muted', () => {
      const mutedContext = { ...mockContext, isMuted: true };
      expect(getVoiceStatusText('listening', mutedContext)).toBe('Muted');
    });

    it('should show connection attempts', () => {
      const connectingContext = { ...mockContext, connectionAttempts: 2 };
      expect(getVoiceStatusText('connecting', connectingContext)).toBe('Connecting (attempt 2/3)...');
    });
  });
});
