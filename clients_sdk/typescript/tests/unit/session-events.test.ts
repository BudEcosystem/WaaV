/**
 * Session-level wire mapping tests (P0).
 *
 * Drives a WebSocketSession through a minimal mock WebSocket and asserts that
 * the events the SDK surfaces to user code carry the correct values from the
 * gateway wire — in particular that a transcript arrives NON-EMPTY, the ready
 * event exposes streamId/protocolVersion, and a protocol_version mismatch is
 * surfaced as a typed error event.
 */
import { describe, it, expect } from 'vitest';
import { WebSocketSession } from '../../src/ws/session.js';
import type { TranscriptEvent, ReadyEvent, SessionErrorEvent } from '../../src/ws/events.js';

/** Minimal browser-WebSocket-shaped mock that lets a test push frames in. */
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
    // Open on next tick so the caller can attach handlers first.
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
  /** Test helper: deliver a JSON frame as if it came from the gateway. */
  emit(obj: unknown): void {
    this.onmessage?.({ data: JSON.stringify(obj) });
  }
}

function makeSession(): { session: WebSocketSession; getWs: () => MockWebSocket } {
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
    autoConfig: false,
    reconnect: false,
  });
  return { session, getWs: () => ws };
}

describe('WebSocketSession transcript event', () => {
  it('surfaces a NON-EMPTY transcript on the transcript event', async () => {
    const { session, getWs } = makeSession();
    const events: TranscriptEvent[] = [];
    session.on('transcript', (e) => events.push(e));

    await session.connect();
    getWs().emit({
      type: 'stt_result',
      transcript: 'hello world',
      is_final: true,
      is_speech_final: true,
      confidence: 0.95,
    });

    expect(events).toHaveLength(1);
    expect(events[0]!.text).toBe('hello world');
    expect(events[0]!.text).not.toBe('');
    expect(events[0]!.isFinal).toBe(true);
    expect(events[0]!.isSpeechFinal).toBe(true);
    expect(events[0]!.confidence).toBeCloseTo(0.95);

    await session.disconnect();
  });
});

describe('WebSocketSession ready event', () => {
  it('exposes streamId + protocolVersion from the ready frame', async () => {
    const { session, getWs } = makeSession();
    const ready: ReadyEvent[] = [];
    session.on('ready', (e) => ready.push(e));

    await session.connect();
    getWs().emit({
      type: 'ready',
      protocol_version: '1.0',
      stream_id: 'stream-xyz',
      livekit_url: 'ws://localhost:7880',
    });

    expect(ready).toHaveLength(1);
    expect(ready[0]!.streamId).toBe('stream-xyz');
    expect(ready[0]!.protocolVersion).toBe('1.0');
    expect(ready[0]!.livekitUrl).toBe('ws://localhost:7880');
    expect(session.getSessionId()).toBe('stream-xyz');

    await session.disconnect();
  });

  it('emits a typed error on protocol_version mismatch', async () => {
    const { session, getWs } = makeSession();
    const errors: SessionErrorEvent[] = [];
    session.on('error', (e) => errors.push(e));

    await session.connect();
    getWs().emit({ type: 'ready', protocol_version: '2.0', stream_id: 's1' });

    expect(errors.some((e) => e.code === 'PROTOCOL_VERSION_MISMATCH')).toBe(true);

    await session.disconnect();
  });
});

describe('WebSocketSession keepalive', () => {
  it('does NOT send a JSON ping frame', async () => {
    const { session, getWs } = makeSession();
    await session.connect();
    // Give any (now-removed) ping interval a chance to fire.
    await new Promise((r) => setTimeout(r, 30));
    const pingFrames = getWs().sent.filter((d) => typeof d === 'string' && d.includes('"ping"'));
    expect(pingFrames).toHaveLength(0);
    await session.disconnect();
  });
});

describe('WebSocketSession control ops reach the wire', () => {
  it('clear() sends {type:"clear"} and audioEnd() sends {type:"audio_end"}', async () => {
    const { session, getWs } = makeSession();
    await session.connect();
    session.clear();
    session.audioEnd();
    const strings = getWs().sent.filter((d): d is string => typeof d === 'string');
    expect(strings.some((s) => JSON.parse(s).type === 'clear')).toBe(true);
    expect(strings.some((s) => JSON.parse(s).type === 'audio_end')).toBe(true);
    await session.disconnect();
  });
});
