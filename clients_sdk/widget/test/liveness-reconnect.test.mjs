/**
 * D1 (liveness/zombie watchdog) + D4 (post-ready reconnect) for the widget WS.
 *
 * The browser cannot send raw WS pings and the gateway has no JSON ping op, so:
 *  - D4: a socket drop AFTER `ready` must reconnect (the old code gated reconnect
 *    on `!settled`, leaving a post-ready drop permanently dead). On reconnect the
 *    full config envelope is re-sent.
 *  - D1: with no inbound frame for ~12s during an active session the watchdog
 *    closes the (zombie) socket, which routes into the same reconnect path. An
 *    inbound frame keeps refreshing the liveness clock so a live call never trips.
 *
 * We mount a real <bud-widget>, stub the global WebSocket with a controllable
 * fake, fake the timers (node:test mock.timers), and override performance.now so
 * the 12s stale window is exercised deterministically.
 */
import { test, mock } from 'node:test';
import assert from 'node:assert/strict';
import { widget } from './harness.mjs';

// A controllable fake WebSocket: records every instance + every frame it sent,
// and lets the test drive open / inbound / close explicitly.
class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances = [];
  static reset() {
    FakeWebSocket.instances = [];
  }
  constructor(url) {
    this.url = url;
    this.readyState = FakeWebSocket.OPEN;
    this.sent = [];
    this.onopen = null;
    this.onmessage = null;
    this.onerror = null;
    this.onclose = null;
    FakeWebSocket.instances.push(this);
    // Open synchronously on the next microtask so onopen handlers are attached.
    queueMicrotask(() => this.onopen && this.onopen());
  }
  send(data) {
    this.sent.push(data);
  }
  close() {
    if (this.readyState === FakeWebSocket.CLOSED) return;
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose && this.onclose();
  }
  /** Simulate an inbound JSON frame from the gateway. */
  emit(obj) {
    this.onmessage && this.onmessage({ data: JSON.stringify(obj) });
  }
  /** Frames this socket sent that are a `config` envelope (parsed). */
  configFrames() {
    return this.sent
      .map((s) => {
        try {
          return JSON.parse(s);
        } catch {
          return null;
        }
      })
      .filter((m) => m && m.type === 'config');
  }
}

async function mountConnected() {
  const el = globalThis.document.createElement('bud-widget');
  el.dataset.gatewayUrl = 'ws://localhost:3099/ws';
  el.dataset.sttProvider = 'deepgram';
  el.dataset.ttsVoice = 'aura-asteria-en';
  globalThis.document.body.appendChild(el);
  const connectP = el.connect();
  await Promise.resolve(); // let the FakeWebSocket open + sendConfig fire
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1];
  assert.ok(ws, 'widget opened a WebSocket');
  ws.emit({ type: 'ready', stream_id: 's1', protocol_version: '1.0' });
  await connectP.catch(() => {}); // player init may reject in jsdom — ignore
  return { el, ws };
}

test('D4: a POST-ready socket drop reconnects and re-sends the config envelope', async () => {
  const prevWS = globalThis.WebSocket;
  globalThis.WebSocket = FakeWebSocket;
  FakeWebSocket.reset();
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const { el, ws } = await mountConnected();
    assert.equal(FakeWebSocket.instances.length, 1, 'one socket so far');
    assert.equal(ws.configFrames().length, 1, 'config sent on the first connection');

    // Server-initiated drop AFTER ready (the case the old !settled gate ignored).
    ws.close();

    // Backoff timer is faked; advance past the first reconnect delay (~1000ms+jitter).
    mock.timers.tick(2000);
    await Promise.resolve(); // new FakeWebSocket opens + re-sends config

    assert.equal(FakeWebSocket.instances.length, 2, 'a POST-ready drop reconnected');
    const ws2 = FakeWebSocket.instances[1];
    assert.equal(ws2.configFrames().length, 1, 'the config envelope was re-sent on reconnect');
    // The re-sent envelope still carries the D2 audio knobs.
    assert.equal(ws2.configFrames()[0].tts_config.audio_out_chunk_ms, 20);

    el.remove();
  } finally {
    mock.timers.reset();
    globalThis.WebSocket = prevWS;
  }
});

test('D4: a user-initiated disconnect() does NOT reconnect', async () => {
  const prevWS = globalThis.WebSocket;
  globalThis.WebSocket = FakeWebSocket;
  FakeWebSocket.reset();
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const { el } = await mountConnected();
    assert.equal(FakeWebSocket.instances.length, 1);

    // Public teardown — must clear the active flag so no reconnect is scheduled.
    el.disconnect();
    mock.timers.tick(5000);
    await Promise.resolve();

    assert.equal(FakeWebSocket.instances.length, 1, 'disconnect() must not spawn a new socket');
    el.remove();
  } finally {
    mock.timers.reset();
    globalThis.WebSocket = prevWS;
  }
});

test('D1: no inbound for the stale window closes the zombie socket and reconnects', async () => {
  const prevWS = globalThis.WebSocket;
  const prevNow = globalThis.performance.now;
  globalThis.WebSocket = FakeWebSocket;
  FakeWebSocket.reset();
  // Deterministic clock the watchdog reads via performance.now().
  let fakeNow = 1_000_000;
  globalThis.performance.now = () => fakeNow;
  mock.timers.enable({ apis: ['setTimeout', 'setInterval'] });
  try {
    const { el } = await mountConnected();
    assert.equal(FakeWebSocket.instances.length, 1);
    const ws1 = FakeWebSocket.instances[0];
    assert.equal(ws1.readyState, FakeWebSocket.OPEN, 'socket open after ready');

    // Advance wall-clock past the 12s stale window with NO inbound frame, then
    // let the watchdog interval (every 3s) wake and observe the stale deadline.
    fakeNow += 13_000;
    mock.timers.tick(3000); // watchdog wakes -> sees idle >= 12s -> recycles socket

    assert.equal(ws1.readyState, FakeWebSocket.CLOSED, 'zombie socket was closed by the watchdog');

    // Closing routes into the reconnect path; advance the backoff timer.
    mock.timers.tick(2000);
    await Promise.resolve();
    assert.equal(FakeWebSocket.instances.length, 2, 'watchdog-triggered close reconnected');

    el.remove();
  } finally {
    mock.timers.reset();
    globalThis.performance.now = prevNow;
    globalThis.WebSocket = prevWS;
  }
});

test('D1: a steady inbound stream keeps refreshing liveness (no false zombie trip)', async () => {
  const prevWS = globalThis.WebSocket;
  const prevNow = globalThis.performance.now;
  globalThis.WebSocket = FakeWebSocket;
  FakeWebSocket.reset();
  let fakeNow = 2_000_000;
  globalThis.performance.now = () => fakeNow;
  mock.timers.enable({ apis: ['setTimeout', 'setInterval'] });
  try {
    const { el } = await mountConnected();
    const ws1 = FakeWebSocket.instances[0];

    // Simulate ~30s of a live call: an inbound frame every 2s, watchdog wakes every 3s.
    for (let t = 0; t < 30_000; t += 1000) {
      fakeNow += 1000;
      if (t % 2000 === 0) {
        ws1.emit({ type: 'stt_result', transcript: 'hi', is_final: false });
      }
      mock.timers.tick(1000);
    }

    assert.equal(ws1.readyState, FakeWebSocket.OPEN, 'a continuously-fed socket never trips the watchdog');
    assert.equal(FakeWebSocket.instances.length, 1, 'no spurious reconnect during a live call');
    el.remove();
  } finally {
    mock.timers.reset();
    globalThis.performance.now = prevNow;
    globalThis.WebSocket = prevWS;
  }
});
