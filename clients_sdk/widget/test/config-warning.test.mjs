/**
 * config_warning surfacing: a gateway `{type:"config_warning", code, message,
 * detail}` frame must be surfaced to the developer as a typed, non-fatal
 * `configWarning` event (and never close the session). This is what turns a
 * silent server-side no-op ("you sent a key that doesn't exist", "emotion
 * ignored by deepgram") into something a beginner can actually see.
 *
 * We mount a real <bud-widget> and stub the global WebSocket with a fake that
 * (a) emits `ready` so connect() resolves, then (b) emits a `config_warning`
 * frame — exercising the real widget message path end to end.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { widget } from './harness.mjs';

// Minimal fake WebSocket that lets the test drive inbound frames.
class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  constructor(url) {
    this.url = url;
    this.readyState = FakeWebSocket.OPEN;
    this.sent = [];
    FakeWebSocket.last = this;
    // Open on next tick so onopen handlers are attached first.
    queueMicrotask(() => this.onopen && this.onopen());
  }
  send(data) {
    this.sent.push(data);
  }
  close() {
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose && this.onclose();
  }
  /** Simulate an inbound text frame from the gateway. */
  emit(obj) {
    this.onmessage && this.onmessage({ data: JSON.stringify(obj) });
  }
}

test('config_warning frame surfaces as a typed configWarning event (non-fatal)', async () => {
  const prevWS = globalThis.WebSocket;
  globalThis.WebSocket = FakeWebSocket;
  // jsdom's getUserMedia / AudioContext aren't needed: we only assert the event.
  try {
    const el = globalThis.document.createElement('bud-widget');
    el.dataset.gatewayUrl = 'ws://localhost:3086/ws';
    el.dataset.sttProvider = 'deepgram';
    el.dataset.ttsVoice = 'aura-asteria-en';
    el.dataset.llmBaseUrl = 'http://127.0.0.1:11434/v1';
    el.dataset.llmModel = 'm';
    globalThis.document.body.appendChild(el);

    const warnings = [];
    el.addEventListener('configWarning', (e) => warnings.push(e.detail));

    // Kick off connect; resolve it by emitting `ready`.
    const connectP = el.connect();
    await Promise.resolve();
    const ws = FakeWebSocket.last;
    assert.ok(ws, 'widget must have opened a WebSocket');
    ws.emit({ type: 'ready', stream_id: 'test-stream', protocol_version: '1.0' });
    // connect() resolves on ready (audio player init may reject in jsdom — ignore).
    await connectP.catch(() => {});

    // Gateway emits the unknown-key advisory (P0 live-verified shape).
    ws.emit({
      type: 'config_warning',
      code: 'unknown_config_keys',
      message:
        "Ignored unknown config key(s): turn_detection. Did you mean to nest it under stt_config?",
      detail: { ignored_keys: ['turn_detection'] },
    });

    assert.equal(warnings.length, 1, 'exactly one configWarning event must fire');
    const w = warnings[0];
    assert.equal(w.code, 'unknown_config_keys');
    assert.match(w.message, /Ignored unknown config key/);
    assert.deepEqual(w.detail, { ignored_keys: ['turn_detection'] });

    // Non-fatal: the socket is NOT closed by a config_warning.
    assert.equal(ws.readyState, FakeWebSocket.OPEN, 'config_warning must not close the session');

    el.remove();
  } finally {
    globalThis.WebSocket = prevWS;
  }
});

test('config_warning with missing fields degrades gracefully', async () => {
  const prevWS = globalThis.WebSocket;
  globalThis.WebSocket = FakeWebSocket;
  try {
    const el = globalThis.document.createElement('bud-widget');
    el.dataset.gatewayUrl = 'ws://localhost:3086/ws';
    el.dataset.sttProvider = 'deepgram';
    globalThis.document.body.appendChild(el);
    const warnings = [];
    el.addEventListener('configWarning', (e) => warnings.push(e.detail));

    const connectP = el.connect();
    await Promise.resolve();
    const ws = FakeWebSocket.last;
    ws.emit({ type: 'ready', stream_id: 's' });
    await connectP.catch(() => {});

    // A future/odd advisory with only a code still surfaces without throwing.
    ws.emit({ type: 'config_warning', code: 'reasoning_model_on_voice_path' });
    assert.equal(warnings.length, 1);
    assert.equal(warnings[0].code, 'reasoning_model_on_voice_path');
    assert.equal(warnings[0].message, '');
    assert.equal(warnings[0].detail, undefined);
    el.remove();
  } finally {
    globalThis.WebSocket = prevWS;
  }
});
