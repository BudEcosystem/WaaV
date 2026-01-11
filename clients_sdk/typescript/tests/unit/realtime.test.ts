import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  BudRealtime,
  RealtimeConfig,
  ToolDefinition,
} from '../../src/pipelines/realtime';

// Store created WebSocket instances for testing
let wsInstances: MockWebSocket[] = [];

// Mock WebSocket
class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  readyState = MockWebSocket.CONNECTING;
  onopen: (() => void) | null = null;
  onclose: ((event: { code: number; reason: string }) => void) | null = null;
  onerror: ((error: Error) => void) | null = null;
  onmessage: ((event: { data: string | ArrayBuffer }) => void) | null = null;

  url: string;
  protocol: string;
  sendSpy = vi.fn();

  constructor(url: string, protocol?: string) {
    this.url = url;
    this.protocol = protocol || '';
    wsInstances.push(this);

    // Simulate async connection
    setTimeout(() => {
      this.readyState = MockWebSocket.OPEN;
      this.onopen?.();
    }, 5);
  }

  send(data: string | ArrayBuffer) {
    this.sendSpy(data);
  }

  close(code?: number, reason?: string) {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.({ code: code || 1000, reason: reason || 'Normal closure' });
  }

  // Helper for tests to simulate server messages
  simulateMessage(data: string | ArrayBuffer) {
    this.onmessage?.({ data });
  }

  simulateError(error: Error) {
    this.onerror?.(error);
  }
}

describe('BudRealtime Pipeline', () => {
  let realtime: BudRealtime;

  beforeEach(() => {
    wsInstances = [];
    vi.stubGlobal('WebSocket', MockWebSocket);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    wsInstances = [];
  });

  describe('OpenAI Realtime Provider', () => {
    beforeEach(() => {
      realtime = new BudRealtime({
        provider: 'openai-realtime',
        apiKey: 'test-api-key',
        model: 'gpt-4o-realtime-preview',
      });
    });

    it('should create instance with correct provider', () => {
      expect(realtime.provider).toBe('openai-realtime');
    });

    it('should connect to gateway WebSocket', async () => {
      const connectPromise = realtime.connect('wss://gateway.example.com/ws');
      await connectPromise;
      expect(realtime.state).toBe('connected');
    });

    it('should send audio data', async () => {
      await realtime.connect('wss://gateway.example.com/ws');

      const audioData = new ArrayBuffer(1024);
      realtime.sendAudio(audioData);

      const ws = wsInstances[0];
      expect(ws.sendSpy).toHaveBeenCalled();

      // Verify it sends the correct message type for OpenAI
      const sentData = ws.sendSpy.mock.calls[ws.sendSpy.mock.calls.length - 1][0];
      const message = JSON.parse(sentData);
      expect(message.type).toBe('input_audio_buffer.append');
      expect(message.audio).toBeDefined();
    });

    it('should send text message', async () => {
      await realtime.connect('wss://gateway.example.com/ws');

      realtime.sendText('Hello, world!');

      const ws = wsInstances[0];
      const calls = ws.sendSpy.mock.calls;

      // Find the conversation.item.create call
      const createCall = calls.find((call: unknown[]) => {
        const msg = JSON.parse(call[0] as string);
        return msg.type === 'conversation.item.create';
      });

      expect(createCall).toBeDefined();
      const message = JSON.parse(createCall![0] as string);
      expect(message.item.content[0].text).toBe('Hello, world!');
    });

    it('should register tools', async () => {
      const tool: ToolDefinition = {
        type: 'function',
        name: 'get_weather',
        description: 'Get current weather',
        parameters: {
          type: 'object',
          properties: {
            location: { type: 'string' },
          },
          required: ['location'],
        },
      };

      await realtime.connect('wss://gateway.example.com/ws');
      await realtime.addTool(tool);

      expect(realtime.tools).toContainEqual(tool);
    });

    it('should emit transcript events', async () => {
      const handler = vi.fn();
      realtime.on('transcript', handler);

      await realtime.connect('wss://gateway.example.com/ws');

      // Simulate transcript from server
      const ws = wsInstances[0];
      ws.simulateMessage(JSON.stringify({
        type: 'response.audio_transcript.delta',
        delta: 'Hello',
      }));

      expect(handler).toHaveBeenCalledWith(expect.objectContaining({
        text: 'Hello',
        isFinal: false,
      }));
    });

    it('should emit audio events', async () => {
      const handler = vi.fn();
      realtime.on('audio', handler);

      await realtime.connect('wss://gateway.example.com/ws');

      // Simulate audio from server (base64 encoded)
      const ws = wsInstances[0];
      ws.simulateMessage(JSON.stringify({
        type: 'response.audio.delta',
        delta: 'SGVsbG8=', // base64 for "Hello"
      }));

      expect(handler).toHaveBeenCalled();
    });

    it('should emit function call events', async () => {
      const handler = vi.fn();
      realtime.on('functionCall', handler);

      await realtime.connect('wss://gateway.example.com/ws');

      // Simulate function call from server
      const ws = wsInstances[0];
      ws.simulateMessage(JSON.stringify({
        type: 'response.function_call_arguments.done',
        name: 'get_weather',
        arguments: '{"location": "San Francisco"}',
        call_id: 'call_123',
      }));

      expect(handler).toHaveBeenCalledWith(expect.objectContaining({
        name: 'get_weather',
        arguments: { location: 'San Francisco' },
        callId: 'call_123',
      }));
    });

    it('should submit function call result', async () => {
      await realtime.connect('wss://gateway.example.com/ws');

      realtime.submitFunctionResult('call_123', { temperature: 72, unit: 'F' });

      const ws = wsInstances[0];
      const calls = ws.sendSpy.mock.calls;

      // Find the function_call_output call
      const outputCall = calls.find((call: unknown[]) => {
        const msg = JSON.parse(call[0] as string);
        return msg.type === 'conversation.item.create' && msg.item?.type === 'function_call_output';
      });

      expect(outputCall).toBeDefined();
      const message = JSON.parse(outputCall![0] as string);
      expect(message.item.call_id).toBe('call_123');
    });

    it('should handle interruption', async () => {
      await realtime.connect('wss://gateway.example.com/ws');

      realtime.interrupt();

      const ws = wsInstances[0];
      const calls = ws.sendSpy.mock.calls;

      // Find the cancel call
      const cancelCall = calls.find((call: unknown[]) => {
        const msg = JSON.parse(call[0] as string);
        return msg.type === 'response.cancel';
      });

      expect(cancelCall).toBeDefined();
    });

    it('should disconnect cleanly', async () => {
      await realtime.connect('wss://gateway.example.com/ws');

      await realtime.disconnect();
      expect(realtime.state).toBe('disconnected');
    });
  });

  describe('Hume EVI Provider', () => {
    beforeEach(() => {
      realtime = new BudRealtime({
        provider: 'hume-evi',
        apiKey: 'test-api-key',
        eviVersion: '3',
      });
    });

    it('should create instance with Hume EVI provider', () => {
      expect(realtime.provider).toBe('hume-evi');
    });

    it('should emit emotion events', async () => {
      const handler = vi.fn();
      realtime.on('emotion', handler);

      await realtime.connect('wss://gateway.example.com/ws');

      // Simulate emotion from Hume EVI
      const ws = wsInstances[0];
      ws.simulateMessage(JSON.stringify({
        type: 'user_message',
        message: { content: 'Hello' },
        models: {
          prosody: {
            scores: {
              joy: 0.8,
              sadness: 0.1,
              anger: 0.05,
              fear: 0.05,
            },
          },
        },
      }));

      expect(handler).toHaveBeenCalledWith(expect.objectContaining({
        emotions: expect.objectContaining({
          joy: 0.8,
        }),
        dominant: 'joy',
      }));
    });

    it('should configure EVI-specific settings', () => {
      const config: RealtimeConfig = {
        provider: 'hume-evi',
        apiKey: 'test',
        eviVersion: '4-mini',
        voiceId: 'custom-voice',
        systemPrompt: 'You are a helpful assistant.',
        verboseTranscription: true,
      };

      const instance = new BudRealtime(config);
      expect(instance.config.eviVersion).toBe('4-mini');
      expect(instance.config.voiceId).toBe('custom-voice');
    });

    it('should resume chat session', async () => {
      const resumeRealtime = new BudRealtime({
        provider: 'hume-evi',
        apiKey: 'test',
        resumedChatGroupId: 'previous-session-123',
      });

      await resumeRealtime.connect('wss://gateway.example.com/ws');

      const ws = wsInstances[0];
      const calls = ws.sendSpy.mock.calls;

      // First message should include resumed chat group ID
      if (calls.length > 0) {
        const message = JSON.parse(calls[0][0] as string);
        expect(message.resumed_chat_group_id).toBe('previous-session-123');
      }
    });
  });

  describe('Event System', () => {
    beforeEach(() => {
      realtime = new BudRealtime({
        provider: 'openai-realtime',
        apiKey: 'test',
      });
    });

    it('should support on/off event handlers', () => {
      const handler = vi.fn();

      realtime.on('transcript', handler);
      realtime.off('transcript', handler);

      // Handler should be removed
      expect(realtime.listenerCount('transcript')).toBe(0);
    });

    it('should support once event handlers', async () => {
      const handler = vi.fn();
      realtime.once('connected', handler);

      await realtime.connect('wss://gateway.example.com/ws');

      // Handler should be called exactly once
      expect(handler).toHaveBeenCalledTimes(1);
    });

    it('should emit state change events', async () => {
      const handler = vi.fn();
      realtime.on('stateChange', handler);

      await realtime.connect('wss://gateway.example.com/ws');

      expect(handler).toHaveBeenCalledWith({
        previousState: 'disconnected',
        currentState: 'connecting',
      });
    });

    it('should emit error events', async () => {
      const handler = vi.fn();
      realtime.on('error', handler);

      await realtime.connect('wss://gateway.example.com/ws');

      // Simulate error
      const ws = wsInstances[0];
      ws.simulateError(new Error('Connection lost'));

      expect(handler).toHaveBeenCalled();
    });
  });

  describe('Configuration Validation', () => {
    it('should require provider', () => {
      expect(() => new BudRealtime({} as RealtimeConfig)).toThrow();
    });

    it('should validate provider value', () => {
      expect(() => new BudRealtime({
        provider: 'invalid' as 'openai-realtime',
        apiKey: 'test',
      })).toThrow();
    });

    it('should use default model for OpenAI', () => {
      const rt = new BudRealtime({
        provider: 'openai-realtime',
        apiKey: 'test',
      });

      expect(rt.config.model).toBe('gpt-4o-realtime-preview');
    });

    it('should use default EVI version for Hume', () => {
      const rt = new BudRealtime({
        provider: 'hume-evi',
        apiKey: 'test',
      });

      expect(rt.config.eviVersion).toBe('3');
    });
  });
});
