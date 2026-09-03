/**
 * D3 capture: off-main-thread AudioWorklet (with a ScriptProcessor legacy
 * fallback). The recorder must:
 *  - prefer AudioWorklet when available: register the inline worklet module
 *    (audioWorklet.addModule on a Blob URL), create an AudioWorkletNode, and NOT
 *    create a ScriptProcessorNode;
 *  - fall back to ScriptProcessorNode only when AudioWorkletNode is undefined;
 *  - forward each received Int16 frame to onData and run RMS VAD (onSpeech/onSilence)
 *    identically on both paths.
 *
 * jsdom has no Web Audio / getUserMedia, so we install minimal fakes that record
 * which capture node was created and let the test push frames through the worklet
 * port.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { widget } from './harness.mjs';

const { AudioRecorder } = widget;

// --- fakes -------------------------------------------------------------------
class FakeWorkletNode {
  constructor(ctx, name, opts) {
    this.ctx = ctx;
    this.name = name;
    this.opts = opts;
    this.port = { onmessage: null, postMessage: () => {} };
    this._connectedFrom = null;
    this.disconnected = false;
    FakeWorkletNode.last = this;
  }
  disconnect() {
    this.disconnected = true;
  }
}

class FakeScriptProcessor {
  constructor() {
    this.onaudioprocess = null;
    this.disconnected = false;
    this.connectedTo = null;
    FakeScriptProcessor.last = this;
  }
  connect(dest) {
    this.connectedTo = dest;
  }
  disconnect() {
    this.disconnected = true;
  }
}

class FakeMediaStreamSource {
  constructor() {
    this.connectedTo = [];
  }
  connect(node) {
    this.connectedTo.push(node);
  }
  disconnect() {}
}

function makeFakeAudioContext({ withWorklet }) {
  const addModuleCalls = [];
  const ctx = {
    sampleRate: 16000,
    state: 'running',
    destination: {},
    createMediaStreamSource: () => new FakeMediaStreamSource(),
    createScriptProcessor: () => new FakeScriptProcessor(),
    close: async () => {},
  };
  if (withWorklet) {
    ctx.audioWorklet = {
      addModule: async (url) => {
        addModuleCalls.push(url);
      },
    };
  }
  ctx._addModuleCalls = addModuleCalls;
  return ctx;
}

// globalThis.navigator is a getter-only accessor in modern Node; override it via
// defineProperty and restore the original descriptor afterwards.
function setNavigator(value) {
  const prevDesc = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  Object.defineProperty(globalThis, 'navigator', {
    value,
    configurable: true,
    writable: true,
  });
  return prevDesc;
}

async function withEnv({ withWorklet }, fn) {
  const prev = {
    AudioContext: globalThis.AudioContext,
    AudioWorkletNode: globalThis.AudioWorkletNode,
    Blob: globalThis.Blob,
    URL: globalThis.URL,
  };

  let lastCtx = null;
  globalThis.AudioContext = function (opts) {
    lastCtx = makeFakeAudioContext({ withWorklet });
    if (opts && opts.sampleRate) lastCtx.sampleRate = opts.sampleRate;
    return lastCtx;
  };
  globalThis.AudioWorkletNode = withWorklet ? FakeWorkletNode : undefined;

  // Blob + URL.createObjectURL/revokeObjectURL for the inline worklet module.
  globalThis.Blob = class {
    constructor(parts, opts) {
      this.parts = parts;
      this.opts = opts;
    }
  };
  const created = [];
  const revoked = [];
  globalThis.URL = {
    createObjectURL: (b) => {
      const u = `blob:fake/${created.length}`;
      created.push(b);
      return u;
    },
    revokeObjectURL: (u) => revoked.push(u),
  };

  // navigator.mediaDevices.getUserMedia returning a fake stream.
  const tracks = [{ stop() {} }];
  const prevNavDesc = setNavigator({
    mediaDevices: {
      getUserMedia: async () => ({ getTracks: () => tracks }),
    },
  });

  try {
    return await fn({ getCtx: () => lastCtx, created, revoked });
  } finally {
    globalThis.AudioContext = prev.AudioContext;
    globalThis.AudioWorkletNode = prev.AudioWorkletNode;
    globalThis.Blob = prev.Blob;
    globalThis.URL = prev.URL;
    if (prevNavDesc) {
      Object.defineProperty(globalThis, 'navigator', prevNavDesc);
    } else {
      delete globalThis.navigator;
    }
  }
}

test('D3: AudioWorklet path is preferred — addModule + AudioWorkletNode, no ScriptProcessor', async () => {
  await withEnv({ withWorklet: true }, async ({ getCtx }) => {
    FakeWorkletNode.last = null;
    FakeScriptProcessor.last = null;
    const rec = new AudioRecorder({ sampleRate: 16000 });
    await rec.start();

    const ctx = getCtx();
    assert.equal(ctx._addModuleCalls.length, 1, 'the inline worklet module was registered');
    assert.match(ctx._addModuleCalls[0], /^blob:/, 'registered from an inline Blob URL');
    assert.ok(FakeWorkletNode.last, 'an AudioWorkletNode was created (off-main-thread capture)');
    assert.equal(FakeWorkletNode.last.name, 'bud-capture', 'the bud-capture processor was used');
    assert.equal(FakeScriptProcessor.last, null, 'ScriptProcessor must NOT be used when worklet exists');
    rec.stop();
  });
});

test('D3: worklet frames reach onData + drive RMS VAD (onSpeech on a loud frame)', async () => {
  await withEnv({ withWorklet: true }, async () => {
    // vadStartSecs:0 keeps the original "one loud frame starts speech" semantics
    // for this capture-path test; the D3b hysteresis (sustained start/stop secs)
    // is exercised separately in recorder-constraints.test.mjs.
    const rec = new AudioRecorder({ sampleRate: 16000, vadStartSecs: 0 });
    const datas = [];
    let speeches = 0;
    rec.onData((d) => datas.push(d));
    rec.onSpeech(() => speeches++);
    await rec.start();

    // Push one 20ms (320-sample) loud Int16 frame through the worklet port.
    const frame = new Int16Array(320).fill(8000); // RMS ~0.24 > 0.01 threshold
    FakeWorkletNode.last.port.onmessage({ data: frame.buffer });

    assert.equal(datas.length, 1, 'the Int16 frame was forwarded to onData');
    assert.equal(datas[0].length, 320, 'frame is a 20ms@16k Int16 frame');
    assert.equal(speeches, 1, 'a loud frame triggers the speech-start VAD edge (vadStartSecs=0)');
    rec.stop();
  });
});

test('D3: falls back to ScriptProcessor when AudioWorkletNode is undefined', async () => {
  await withEnv({ withWorklet: false }, async () => {
    FakeWorkletNode.last = null;
    FakeScriptProcessor.last = null;
    const rec = new AudioRecorder({ sampleRate: 16000 });
    const datas = [];
    rec.onData((d) => datas.push(d));
    await rec.start();

    assert.ok(FakeScriptProcessor.last, 'ScriptProcessor fallback was created (no AudioWorklet)');

    // Drive the legacy onaudioprocess path with a Float32 buffer.
    const input = new Float32Array(4096).fill(0.25);
    FakeScriptProcessor.last.onaudioprocess({
      inputBuffer: { getChannelData: () => input },
    });
    assert.equal(datas.length, 1, 'fallback path also forwards an Int16 frame to onData');
    assert.equal(datas[0].length, 4096);
    rec.stop();
  });
});

test('D3: stop() releases the worklet node and revokes the inline Blob URL', async () => {
  await withEnv({ withWorklet: true }, async ({ revoked }) => {
    const rec = new AudioRecorder({ sampleRate: 16000 });
    await rec.start();
    const node = FakeWorkletNode.last;
    rec.stop();
    assert.equal(node.disconnected, true, 'worklet node disconnected on stop');
    assert.equal(node.port.onmessage, null, 'worklet port handler detached on stop');
    assert.equal(revoked.length, 1, 'the inline worklet Blob URL was revoked');
    assert.equal(rec.isRecording, false, 'recorder reports stopped');
  });
});
