/**
 * D6 connect-timeout + D7 WS 503 back-off tests.
 *
 * The SDK connect timeout is sized to exceed the gateway's ~30s PROVIDER_READY
 * for an audio=true session (config_handler.rs:45) — the old 10s default tripped
 * that legitimate wait. Asserts the default is 35s (constant + constructor), that
 * an explicit override wins, and that a 429/503 upgrade response surfaces as a
 * typed, retryable RateLimitError (Node `ws` only — browser can't see it).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { WebSocketConnection, DEFAULT_CONNECT_TIMEOUT_MS } from '../../src/ws/connection.js';
import { RateLimitError } from '../../src/errors/index.js';

/**
 * Minimal Node-`ws`-like mock: exposes `on` (so the unexpected-response/pong
 * plumbing attaches) and lets the test fire `unexpected-response` to simulate a
 * 429/503 upgrade rejection followed by a close.
 */
class MockWs {
  binaryType = 'arraybuffer';
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  bufferedAmount = 0;
  private listeners: Record<string, Array<(...a: unknown[]) => void>> = {};
  static OPEN = 1;
  readyState = 0;
  constructor(public url: string) {}
  on(event: string, cb: (...a: unknown[]) => void): void {
    (this.listeners[event] ??= []).push(cb);
  }
  emit(event: string, ...args: unknown[]): void {
    (this.listeners[event] ?? []).forEach((cb) => cb(...args));
  }
  close(): void {
    this.readyState = 3;
  }
  ping(): void {}
}

describe('D6 connect timeout default', () => {
  it('exposes the 35s default constant (PROVIDER_READY headroom)', () => {
    expect(DEFAULT_CONNECT_TIMEOUT_MS).toBe(35000);
  });

  it('defaults the connection timeout to 35s', () => {
    const conn = new WebSocketConnection({
      url: 'ws://x/ws',
      WebSocket: MockWs as unknown as typeof WebSocket,
    });
    expect((conn as unknown as { timeout: number }).timeout).toBe(35000);
  });

  it('honours an explicit timeout override', () => {
    const conn = new WebSocketConnection({
      url: 'ws://x/ws',
      timeout: 12345,
      WebSocket: MockWs as unknown as typeof WebSocket,
    });
    expect((conn as unknown as { timeout: number }).timeout).toBe(12345);
  });
});

describe('D6/D7 connect timeout firing + 503 handling', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('rejects with a TimeoutError after the configured timeout when the socket never opens', async () => {
    const conn = new WebSocketConnection({
      url: 'ws://x/ws',
      timeout: 35000,
      WebSocket: MockWs as unknown as typeof WebSocket,
    });
    const p = conn.connect();
    // Attach a catch synchronously so the rejection is observed.
    const settled = p.then(() => 'open').catch((e: Error) => e.name);
    // Not yet timed out at 34s.
    await vi.advanceTimersByTimeAsync(34000);
    // Past 35s -> TimeoutError.
    await vi.advanceTimersByTimeAsync(2000);
    expect(await settled).toBe('TimeoutError');
  });

  it('surfaces a 503 upgrade rejection as a retryable at-capacity RateLimitError', async () => {
    let made: MockWs | null = null;
    const WS = class extends MockWs {
      constructor(url: string) {
        super(url);
        made = this as unknown as MockWs;
      }
    };
    const conn = new WebSocketConnection({
      url: 'ws://x/ws',
      timeout: 35000,
      WebSocket: WS as unknown as typeof WebSocket,
    });
    const p = conn.connect();
    const settled = p.then(() => null).catch((e: unknown) => e);
    // Simulate the gateway 503 upgrade rejection, then the close that follows.
    made!.emit('unexpected-response', {}, { statusCode: 503, headers: { 'retry-after': '2' } });
    made!.onclose?.({ code: 1006, reason: 'capacity' });
    const err = await settled;
    expect(err).toBeInstanceOf(RateLimitError);
    expect((err as RateLimitError).statusCode).toBe(503);
    expect((err as RateLimitError).isAtCapacity()).toBe(true);
    expect((err as RateLimitError).retryAfterMs).toBe(2000);
  });

  it('surfaces a 429 upgrade rejection as a RateLimitError', async () => {
    let made: MockWs | null = null;
    const WS = class extends MockWs {
      constructor(url: string) {
        super(url);
        made = this as unknown as MockWs;
      }
    };
    const conn = new WebSocketConnection({
      url: 'ws://x/ws',
      WebSocket: WS as unknown as typeof WebSocket,
    });
    const settled = conn.connect().then(() => null).catch((e: unknown) => e);
    made!.emit('unexpected-response', {}, { statusCode: 429, headers: { 'retry-after': '1' } });
    made!.onclose?.({ code: 1006, reason: 'rate limit' });
    const err = await settled;
    expect(err).toBeInstanceOf(RateLimitError);
    expect((err as RateLimitError).statusCode).toBe(429);
    expect((err as RateLimitError).isAtCapacity()).toBe(false);
  });
});
