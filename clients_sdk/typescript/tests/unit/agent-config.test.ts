/**
 * Voice agents on `/ws` (spec 025), and the fields the gateway no longer requires.
 *
 * `{"type":"config","agent":{"id":"support"}}` names a Bud voice agent: the agent decides both
 * speech legs. Under the Bud control plane a deployment decides the vendor (`provider` may be
 * omitted on both legs) and the conversation loop's LLM is a Bud deployment (the gateway refuses a
 * `base_url`, and requires only `model`).
 */
import { describe, it, expect } from 'vitest';
import { serializeMessage, createConfigMessage } from '../../src/ws/messages.js';
import { conversationConfigToWire } from '../../src/types/conversation.js';
import { WebSocketSession } from '../../src/ws/session.js';
import { BudTalk } from '../../src/pipelines/talk.js';

function wireOf(extra: Parameters<typeof createConfigMessage>[4], stt?: Parameters<typeof createConfigMessage>[0]) {
  return JSON.parse(serializeMessage(createConfigMessage(stt, undefined, undefined, undefined, extra)));
}

class MockWebSocket {
  static OPEN = 1;
  static CONNECTING = 0;
  static CLOSING = 2;
  static CLOSED = 3;
  OPEN = MockWebSocket.OPEN;
  readyState = MockWebSocket.CONNECTING;
  binaryType = 'arraybuffer';
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  sent: unknown[] = [];
  constructor(public url: string) {
    setTimeout(() => {
      this.readyState = MockWebSocket.OPEN;
      this.onopen?.();
    }, 0);
  }
  send(data: unknown): void {
    this.sent.push(data);
  }
  close(code = 1000, reason = ''): void {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.({ code, reason });
  }
}

describe('agent — the config envelope', () => {
  it('sends the agent with only the fields set, in wire spelling', () => {
    expect(wireOf({ agent: { id: 'support', version: 3, variables: { tier: 'gold' } } }).agent).toEqual({
      id: 'support',
      version: 3,
      variables: { tier: 'gold' },
    });
    expect(wireOf({ agent: { id: 'support', textOnly: true } }).agent).toEqual({ id: 'support', text_only: true });
  });

  it('omits the agent when unset', () => {
    expect(wireOf({ audio: false }).agent).toBeUndefined();
  });

  it('a session given an agent name sends it on connect, with no speech legs of its own', async () => {
    let ws!: MockWebSocket;
    const Impl = class extends MockWebSocket {
      constructor(url: string) {
        super(url);
        ws = this;
      }
    };
    const session = new WebSocketSession({
      url: 'ws://127.0.0.1:3009/ws',
      WebSocket: Impl as unknown as typeof WebSocket,
      reconnect: false,
      agent: 'support',
    });
    await session.connect();
    const config = JSON.parse(ws.sent[0] as string);
    expect(config.type).toBe('config');
    expect(config.agent).toEqual({ id: 'support' });
    expect(config.stt_config).toBeUndefined();
    expect(config.tts_config).toBeUndefined();
    await session.disconnect();
  });
});

describe('fields the gateway no longer requires', () => {
  it('a speech leg may leave the provider to its Bud deployment', () => {
    const wire = wireOf(undefined, { model: 'my-stt-deployment', language: 'en' });
    expect(wire.stt_config.model).toBe('my-stt-deployment');
    expect('provider' in wire.stt_config).toBe(false);
  });

  it('a conversation may leave the base URL to the gateway', () => {
    expect(conversationConfigToWire({ model: 'chat' })).toEqual({ model: 'chat' });
    expect(conversationConfigToWire({ baseUrl: 'https://llm/v1', model: 'chat' }).base_url).toBe('https://llm/v1');
  });
});

describe('agent — the Talk pipeline', () => {
  it('threads the agent to its session', () => {
    const prev = (globalThis as any).WebSocket;
    (globalThis as any).WebSocket = MockWebSocket;
    try {
      const talk = new BudTalk({ url: 'ws://127.0.0.1:3009/ws', agent: 'support', autoPlay: false });
      expect(((talk as any).session.config as { agent?: unknown }).agent).toBe('support');
    } finally {
      (globalThis as any).WebSocket = prev;
    }
  });
});
