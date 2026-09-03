/**
 * D1 end-to-end (session level): a frozen-but-OPEN socket is detected by the
 * liveness watchdog and healed via terminate + the EXISTING reconnect path.
 *
 * Uses a Node-`ws`-shaped mock that exposes `.on('pong')` + `.ping()` +
 * `.terminate()` but NEVER answers a ping (the SIGSTOP/zombie case). With fake
 * timers driving the ~5s ping / ~3s pong-deadline, the session must terminate
 * the dead socket and open a NEW one. A second test proves a pong keeps the
 * socket alive (no false zombie).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { WebSocketSession } from '../../src/ws/session.js';

/** A Node-`ws`-shaped mock: has on()/ping()/terminate() + bufferedAmount. */
class NodeWsMock {
  static OPEN = 1;
  static CONNECTING = 0;
  static CLOSING = 2;
  static CLOSED = 3;
  OPEN = NodeWsMock.OPEN;
  readyState = NodeWsMock.CONNECTING;
  binaryType = 'arraybuffer';
  bufferedAmount = 0;
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  sent: unknown[] = [];
  pings = 0;
  terminated = false;
  /** When true, a ping is auto-answered with a pong (healthy socket). */
  autoPong = false;
  private listeners: Record<string, Array<(...a: unknown[]) => void>> = {};

  constructor(public url: string) {
    setTimeout(() => {
      this.readyState = NodeWsMock.OPEN;
      this.onopen?.();
    }, 0);
  }
  on(event: string, cb: (...a: unknown[]) => void): void {
    (this.listeners[event] ??= []).push(cb);
  }
  private fire(event: string, ...args: unknown[]): void {
    for (const cb of this.listeners[event] ?? []) cb(...args);
  }
  ping(): void {
    this.pings++;
    if (this.autoPong) this.fire('pong');
  }
  send(data: unknown): void {
    this.sent.push(data);
  }
  terminate(): void {
    this.terminated = true;
    this.readyState = NodeWsMock.CLOSED;
    // Node `ws` terminate() does not call onclose synchronously, but the
    // session's terminate() fires its own onClose; nothing to do here.
  }
  close(code = 1000, reason = ''): void {
    this.readyState = NodeWsMock.CLOSED;
    this.onclose?.({ code, reason });
  }
  emit(obj: unknown): void {
    this.onmessage?.({ data: JSON.stringify(obj) });
  }
}

function makeSession(opts?: { autoPong?: boolean }) {
  const created: NodeWsMock[] = [];
  const Impl = class extends NodeWsMock {
    constructor(url: string) {
      super(url);
      this.autoPong = opts?.autoPong ?? false;
      created.push(this);
    }
  };
  const session = new WebSocketSession({
    url: 'ws://127.0.0.1:3009/ws',
    WebSocket: Impl as unknown as typeof WebSocket,
    autoConfig: false,
    // Fast thresholds so the test is quick under fake timers.
    liveness: { pingIntervalMs: 5000, pongTimeoutMs: 3000 },
    reconnect: { initialDelay: 10, minStableMs: 5000, maxQuickFailures: 99 },
  });
  return { session, created };
}

describe('D1 session zombie detection', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('detects a frozen socket (no pong) and reconnects with a NEW socket', async () => {
    const { session, created } = makeSession({ autoPong: false });
    const errors: string[] = [];
    session.on('error', (e) => errors.push(e.code));

    // The mock opens on a setTimeout(0); under fake timers we must advance to
    // fire it, so kick off connect() then drive the clock for the open.
    const connectP = session.connect();
    await vi.advanceTimersByTimeAsync(1); // let the mock open
    await connectP;
    expect(created.length).toBe(1);
    const first = created[0]!;
    expect(first.terminated).toBe(false);

    // Drive past one ping (5s) + its pong deadline (3s). No pong -> zombie.
    await vi.advanceTimersByTimeAsync(5000);
    expect(first.pings).toBeGreaterThanOrEqual(1);
    await vi.advanceTimersByTimeAsync(3000);

    // The zombie socket was terminated and a LIVENESS_ZOMBIE error surfaced.
    expect(first.terminated).toBe(true);
    expect(errors).toContain('LIVENESS_ZOMBIE');

    // The existing reconnect path opened a fresh socket (initialDelay 10ms).
    await vi.advanceTimersByTimeAsync(50);
    expect(created.length).toBeGreaterThanOrEqual(2);

    await session.disconnect();
  });

  it('a healthy socket that pongs is NOT declared a zombie', async () => {
    const { session, created } = makeSession({ autoPong: true });
    const errors: string[] = [];
    session.on('error', (e) => errors.push(e.code));

    const connectP = session.connect();
    await vi.advanceTimersByTimeAsync(1);
    await connectP;
    const first = created[0]!;

    // Several ping/pong cycles — the auto-pong keeps it alive.
    for (let i = 0; i < 5; i++) {
      await vi.advanceTimersByTimeAsync(5000); // ping; auto-pong answers
    }
    expect(first.terminated).toBe(false);
    expect(errors).not.toContain('LIVENESS_ZOMBIE');
    expect(created.length).toBe(1);

    await session.disconnect();
  });
});
