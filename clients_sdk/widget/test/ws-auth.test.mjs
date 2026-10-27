/**
 * Where the widget's API key rides on the WebSocket upgrade. A browser WebSocket can't send
 * headers, so the key is offered as the `openai-insecure-api-key.<key>` subprotocol beside
 * `realtime`, never as `?token=` in the URL (which access logs and proxies record). Only a key
 * with a character no subprotocol may carry falls back to `?token=`.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { widget } from './harness.mjs';

test('the key rides a subprotocol, the URL stays clean', () => {
  assert.deepEqual(widget.authenticatedUpgrade('wss://gw.example.com/ws', 'sk-live.a_b'), {
    url: 'wss://gw.example.com/ws',
    protocols: ['realtime', 'openai-insecure-api-key.sk-live.a_b'],
  });
});

test('no key, no subprotocol and no token', () => {
  assert.deepEqual(widget.authenticatedUpgrade('ws://gw/ws', undefined), { url: 'ws://gw/ws' });
});

test('only a key no subprotocol can carry falls back to ?token=', () => {
  assert.deepEqual(widget.authenticatedUpgrade('ws://gw/ws', 'a/b='), {
    url: 'ws://gw/ws?token=a%2Fb%3D',
  });
});

test('BudWidget.connect() opens the socket with the key subprotocol', async () => {
  const opened = [];
  class RecordingWS {
    static OPEN = 1;
    static CLOSED = 3;
    constructor(url, protocols) {
      opened.push({ url, protocols });
      this.readyState = RecordingWS.OPEN;
      this.onopen = this.onmessage = this.onerror = this.onclose = null;
    }
    send() {}
    close() {
      this.readyState = RecordingWS.CLOSED;
    }
  }
  const prevWS = globalThis.WebSocket;
  globalThis.WebSocket = RecordingWS;
  try {
    const el = globalThis.document.createElement('bud-widget');
    el.dataset.gatewayUrl = 'ws://localhost:3009/ws';
    el.dataset.apiKey = 'sk-widget';
    globalThis.document.body.appendChild(el);
    void el.connect().catch(() => {});
    await new Promise((r) => setTimeout(r, 0));
    assert.equal(opened.length, 1, 'one socket opened');
    assert.equal(opened[0].url, 'ws://localhost:3009/ws', 'no token in the URL');
    assert.deepEqual(opened[0].protocols, ['realtime', 'openai-insecure-api-key.sk-widget']);
    el.disconnect?.();
    el.remove();
  } finally {
    globalThis.WebSocket = prevWS;
  }
});
