/**
 * D6 connect-time: prewarm() + generous connect timeout.
 *
 *  - prewarm() warms an AudioContext (created early, suspended, adopted by the
 *    player on connect) AND fires a best-effort HEAD to the gateway HTTP origin
 *    (mapped from the ws(s):// URL) to prime DNS/TLS. It must be idempotent and
 *    must NEVER throw — a failing/absent fetch is swallowed.
 *  - The widget WS connect timeout default is >= 35s, because the server-side
 *    PROVIDER_READY for an audio=true session can take up to ~30s; a 10s default
 *    would spuriously abort a healthy slow-provider connect. We assert the default
 *    by letting a never-readying socket sit and confirming it does NOT time out
 *    before 35s.
 */
import { test, mock } from 'node:test';
import assert from 'node:assert/strict';
import { widget } from './harness.mjs';

// A controllable fake WebSocket that opens but lets the test decide if/when a
// `ready` is delivered. Used to exercise the connect()-timeout default through the
// public BudWidget.connect() path (which calls ws.connect() with NO timeout arg,
// so it uses the D6 default).
class NeverReadyWS {
  static OPEN = 1;
  static CLOSED = 3;
  static instances = [];
  constructor(url) {
    this.url = url;
    this.readyState = NeverReadyWS.OPEN;
    this.sent = [];
    this.onopen = null;
    this.onmessage = null;
    this.onerror = null;
    this.onclose = null;
    NeverReadyWS.instances.push(this);
    queueMicrotask(() => this.onopen && this.onopen());
  }
  send(d) {
    this.sent.push(d);
  }
  close() {
    if (this.readyState === NeverReadyWS.CLOSED) return;
    this.readyState = NeverReadyWS.CLOSED;
    this.onclose && this.onclose();
  }
}

// --- prewarm() ---------------------------------------------------------------

test('D6 prewarm: warms an AudioContext and HEADs the gateway http origin', async () => {
  const prevAC = globalThis.AudioContext;
  const prevFetch = globalThis.fetch;
  let acCreated = 0;
  let acSampleRate = null;
  globalThis.AudioContext = function (opts) {
    acCreated++;
    acSampleRate = opts && opts.sampleRate;
    return { state: 'suspended', close: async () => {}, resume: async () => {} };
  };
  const fetchCalls = [];
  globalThis.fetch = async (url, init) => {
    fetchCalls.push({ url, init });
    return { ok: true };
  };
  try {
    const el = globalThis.document.createElement('bud-widget');
    el.dataset.gatewayUrl = 'wss://gw.example.com:8443/ws';
    el.dataset.ttsVoice = 'aura-asteria-en';
    el.dataset.ttsSampleRate = '24000';
    globalThis.document.body.appendChild(el);

    await el.prewarm();

    assert.equal(acCreated, 1, 'prewarm warmed exactly one AudioContext');
    assert.equal(acSampleRate, 24000, 'context warmed at the TTS sink rate');
    assert.equal(fetchCalls.length, 1, 'prewarm fired one HEAD');
    assert.equal(fetchCalls[0].init.method, 'HEAD', 'used a HEAD request');
    // wss://host:port/ws -> https://host:port  (origin only, scheme mapped).
    assert.equal(fetchCalls[0].url, 'https://gw.example.com:8443', 'HEAD targets the http(s) origin');

    // Idempotent: a second prewarm does not create a second context.
    await el.prewarm();
    assert.equal(acCreated, 1, 'prewarm is idempotent for the AudioContext');

    el.remove();
  } finally {
    globalThis.AudioContext = prevAC;
    globalThis.fetch = prevFetch;
  }
});

test('D6 prewarm: ws:// origin maps to http:// and a failing fetch never throws', async () => {
  const prevAC = globalThis.AudioContext;
  const prevFetch = globalThis.fetch;
  globalThis.AudioContext = function () {
    return { state: 'suspended', close: async () => {}, resume: async () => {} };
  };
  let seenUrl = null;
  globalThis.fetch = async (url) => {
    seenUrl = url;
    throw new Error('network down'); // prewarm must swallow this
  };
  try {
    const el = globalThis.document.createElement('bud-widget');
    el.dataset.gatewayUrl = 'ws://localhost:3009/ws';
    globalThis.document.body.appendChild(el);

    await assert.doesNotReject(() => el.prewarm(), 'prewarm swallows fetch failures');
    assert.equal(seenUrl, 'http://localhost:3009', 'ws:// origin mapped to http://');

    el.remove();
  } finally {
    globalThis.AudioContext = prevAC;
    globalThis.fetch = prevFetch;
  }
});

// --- connect timeout default -------------------------------------------------
//
// Driven through the PUBLIC BudWidget.connect() path, which calls the internal
// ws.connect() with NO timeout argument -> it uses the D6 default (>=35s). On a
// timeout the connect promise rejects and the widget transitions to 'error'; we
// assert that error edge does NOT fire before 35s.

test('D6: BudWidget.connect() default timeout is generous (>=35s) for slow PROVIDER_READY', async () => {
  const prevWS = globalThis.WebSocket;
  globalThis.WebSocket = NeverReadyWS;
  NeverReadyWS.instances = [];
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const el = globalThis.document.createElement('bud-widget');
    el.dataset.gatewayUrl = 'ws://localhost:3009/ws';
    el.dataset.sttProvider = 'deepgram';
    el.dataset.ttsVoice = 'aura-asteria-en';
    globalThis.document.body.appendChild(el);

    // Record the error edge (the connect-timeout surfaces as an 'error' event).
    let erroredAtTick = null;
    el.addEventListener('error', () => {
      if (erroredAtTick === null) erroredAtTick = true;
    });

    el.connect(); // uses the default timeout (no arg)
    await Promise.resolve();
    assert.ok(NeverReadyWS.instances.length >= 1, 'widget opened a socket');

    // At 10s (the OLD buggy default) it must NOT have errored.
    mock.timers.tick(10_000);
    await Promise.resolve();
    assert.equal(erroredAtTick, null, 'did not time out at 10s — the buggy old default is gone');

    // Still connecting at 34s (PROVIDER_READY for audio=true can take ~30s).
    mock.timers.tick(24_000);
    await Promise.resolve();
    assert.equal(erroredAtTick, null, 'still connecting at 34s');

    // Past 35s it finally times out -> error edge fires.
    mock.timers.tick(2_000);
    await Promise.resolve();
    await Promise.resolve();
    assert.equal(erroredAtTick, true, 'times out only after the >=35s default elapses');

    el.disconnect();
    el.remove();
  } finally {
    mock.timers.reset();
    globalThis.WebSocket = prevWS;
  }
});

test('D6: a `ready` arriving at ~20s (slow PROVIDER_READY) still succeeds under the default', async () => {
  const prevWS = globalThis.WebSocket;
  globalThis.WebSocket = NeverReadyWS;
  NeverReadyWS.instances = [];
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const el = globalThis.document.createElement('bud-widget');
    el.dataset.gatewayUrl = 'ws://localhost:3009/ws';
    el.dataset.sttProvider = 'deepgram';
    el.dataset.ttsVoice = 'aura-asteria-en';
    globalThis.document.body.appendChild(el);

    let readyFired = false;
    // Track only TIMEOUT errors. (In jsdom the player's AudioContext init throws
    // after a successful connect — that is unrelated noise; we must not count it.)
    let timedOut = false;
    el.addEventListener('ready', () => {
      readyFired = true;
    });
    el.addEventListener('error', (e) => {
      if (e.detail && /timeout/i.test(e.detail.message || '')) timedOut = true;
    });

    el.connect();
    await Promise.resolve();
    const ws = NeverReadyWS.instances[NeverReadyWS.instances.length - 1];

    // 20s of PROVIDER_READY elapses with no timeout, THEN the gateway readies.
    mock.timers.tick(20_000);
    await Promise.resolve();
    assert.equal(timedOut, false, 'no timeout during the 20s provider warmup');
    ws.onmessage &&
      ws.onmessage({ data: JSON.stringify({ type: 'ready', stream_id: 's1', protocol_version: '1.0' }) });
    await Promise.resolve();
    await Promise.resolve();

    assert.equal(readyFired, true, 'a ready at 20s is accepted (the 35s window covers it)');

    // Advancing PAST 35s now must NOT produce a timeout — the connect already
    // settled successfully when ready arrived (the timer was cleared).
    mock.timers.tick(20_000);
    await Promise.resolve();
    assert.equal(timedOut, false, 'no spurious timeout fires after a successful ready');

    el.disconnect();
    el.remove();
  } finally {
    mock.timers.reset();
    globalThis.WebSocket = prevWS;
  }
});
