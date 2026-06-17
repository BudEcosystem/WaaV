/**
 * D10 (getUserMedia / duplex-echo hardening) + D3b (VAD hysteresis) for the
 * widget AudioRecorder.
 *
 * D10:
 *  - getUserMedia is requested with echoCancellation + noiseSuppression +
 *    autoGainControl = true by DEFAULT, each independently overridable;
 *  - voiceIsolation is only sent when explicitly opted in (non-standard);
 *  - a mic that goes fully silent for ~micSilenceMs fires onMicSilence ONCE, and
 *    re-arms once audio returns.
 *
 * D3b:
 *  - the energy VAD uses start/stop-secs hysteresis: a single loud blip does NOT
 *    start speech (needs sustained-loud >= vadStartSecs) and a brief mid-utterance
 *    dip does NOT stop it (needs sustained-quiet >= vadStopSecs).
 *
 * jsdom has no Web Audio / getUserMedia, so we install minimal fakes (mirroring
 * recorder-worklet.test.mjs) and capture the constraints passed to getUserMedia.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { widget } from './harness.mjs';

const { AudioRecorder } = widget;

class FakeWorkletNode {
  constructor(ctx, name, opts) {
    this.port = { onmessage: null, postMessage: () => {} };
    FakeWorkletNode.last = this;
  }
  disconnect() {}
}
class FakeMediaStreamSource {
  connect() {}
  disconnect() {}
}
function makeFakeAudioContext() {
  return {
    sampleRate: 16000,
    state: 'running',
    destination: {},
    createMediaStreamSource: () => new FakeMediaStreamSource(),
    createScriptProcessor: () => ({ connect() {}, disconnect() {}, onaudioprocess: null }),
    audioWorklet: { addModule: async () => {} },
    close: async () => {},
  };
}
function setNavigator(value) {
  const prev = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  Object.defineProperty(globalThis, 'navigator', { value, configurable: true, writable: true });
  return prev;
}

/**
 * Install the fake Web Audio env. Returns the recorded getUserMedia constraints +
 * a restore fn. Optionally fakes performance.now via a `clock` getter for the
 * hysteresis / mic-silence timing tests.
 */
function installEnv({ clock } = {}) {
  const prev = {
    AudioContext: globalThis.AudioContext,
    AudioWorkletNode: globalThis.AudioWorkletNode,
    Blob: globalThis.Blob,
    URL: globalThis.URL,
    now: globalThis.performance.now,
  };
  globalThis.AudioContext = function () {
    return makeFakeAudioContext();
  };
  globalThis.AudioWorkletNode = FakeWorkletNode;
  globalThis.Blob = class {
    constructor(parts, opts) {
      this.parts = parts;
      this.opts = opts;
    }
  };
  globalThis.URL = {
    createObjectURL: () => 'blob:fake/0',
    revokeObjectURL: () => {},
  };
  if (clock) globalThis.performance.now = clock;

  const captured = { constraints: null };
  const prevNav = setNavigator({
    mediaDevices: {
      getUserMedia: async (c) => {
        captured.constraints = c;
        return { getTracks: () => [{ stop() {} }] };
      },
    },
  });

  return {
    captured,
    restore() {
      globalThis.AudioContext = prev.AudioContext;
      globalThis.AudioWorkletNode = prev.AudioWorkletNode;
      globalThis.Blob = prev.Blob;
      globalThis.URL = prev.URL;
      globalThis.performance.now = prev.now;
      if (prevNav) Object.defineProperty(globalThis, 'navigator', prevNav);
      else delete globalThis.navigator;
    },
  };
}

/** Push one Int16 frame of the given RMS-ish fill through the recorder's worklet. */
function pushFrame(rec, fill, samples = 320) {
  const frame = new Int16Array(samples).fill(fill);
  FakeWorkletNode.last.port.onmessage({ data: frame.buffer });
}

test('D10: getUserMedia defaults echoCancellation + noiseSuppression + autoGainControl=true', async () => {
  const env = installEnv();
  try {
    const rec = new AudioRecorder({ sampleRate: 16000 });
    await rec.start();
    const a = env.captured.constraints.audio;
    assert.equal(a.echoCancellation, true, 'AEC on by default');
    assert.equal(a.noiseSuppression, true, 'NS on by default');
    assert.equal(a.autoGainControl, true, 'AGC on by default (D10 — was previously absent)');
    assert.equal('voiceIsolation' in a, false, 'voiceIsolation NOT sent unless opted in');
    rec.stop();
  } finally {
    env.restore();
  }
});

test('D10: each constraint is independently overridable; voiceIsolation opt-in is emitted', async () => {
  const env = installEnv();
  try {
    const rec = new AudioRecorder({
      sampleRate: 16000,
      echoCancellation: false,
      noiseSuppression: false,
      autoGainControl: false,
      voiceIsolation: true,
    });
    await rec.start();
    const a = env.captured.constraints.audio;
    assert.equal(a.echoCancellation, false, 'AEC override honored');
    assert.equal(a.noiseSuppression, false, 'NS override honored');
    assert.equal(a.autoGainControl, false, 'AGC override honored');
    assert.equal(a.voiceIsolation, true, 'voiceIsolation emitted when explicitly opted in');
    rec.stop();
  } finally {
    env.restore();
  }
});

test('D10: mic-silence — onMicSilence fires once after micSilenceMs of pure silence', async () => {
  let now = 1_000_000;
  const env = installEnv({ clock: () => now });
  try {
    const rec = new AudioRecorder({ sampleRate: 16000, micSilenceMs: 500 });
    let micSilenceEvents = 0;
    rec.onMicSilence(() => micSilenceEvents++);
    await rec.start();

    // Feed only SILENT frames (the watchdog's lastAudibleFrameTime never refreshes).
    pushFrame(rec, 0);
    // Advance the clock past the deadline; manually invoke the watchdog by ticking
    // its interval (we can't reach it directly, so drive time + call the internal
    // check via a real timer would be flaky — instead advance now and let the
    // recorder's own setInterval fire). We emulate that by advancing + waiting a
    // real tick boundary using the period (250ms). Use fake timers instead:
    now += 600;
    await waitMicrotasks();
    // The watchdog runs on a real setInterval(period=250ms); to keep the test
    // deterministic we directly assert via the public behavior after enough real
    // time. Since real timers are slow, we instead trigger the check by pushing a
    // silent frame AFTER the deadline is irrelevant — the watchdog is time-based.
    // So we wait for one real interval period.
    await delay(300);
    assert.ok(micSilenceEvents >= 1, 'onMicSilence fired after sustained silence');
    const afterFirst = micSilenceEvents;

    // It must NOT keep firing every tick (debounced for the silent run).
    await delay(300);
    assert.equal(micSilenceEvents, afterFirst, 'onMicSilence debounced — fires once per silent run');

    rec.stop();
  } finally {
    env.restore();
  }
});

test('D10: audio returning re-arms the mic-silence detector', async () => {
  let now = 2_000_000;
  const env = installEnv({ clock: () => now });
  try {
    const rec = new AudioRecorder({ sampleRate: 16000, micSilenceMs: 300 });
    let events = 0;
    rec.onMicSilence(() => events++);
    await rec.start();

    pushFrame(rec, 0); // silent
    now += 400;
    await delay(220); // > period (150ms) so the watchdog observes the deadline
    assert.equal(events, 1, 'first silent run fired once');

    // Audio returns: refreshes lastAudibleFrameTime + clears the fired flag.
    pushFrame(rec, 8000); // loud
    now += 50;
    await delay(220);
    assert.equal(events, 1, 'no new event while audio is flowing');

    // Go silent again -> should be able to fire a SECOND time.
    pushFrame(rec, 0);
    now += 400;
    await delay(220);
    assert.equal(events, 2, 'detector re-armed and fired again on the next silent run');

    rec.stop();
  } finally {
    env.restore();
  }
});

test('D10: a quiet-but-LIVE room (low nonzero RMS) does NOT trigger mic-silence', async () => {
  let now = 2_500_000;
  const env = installEnv({ clock: () => now });
  try {
    const rec = new AudioRecorder({ sampleRate: 16000, micSilenceMs: 300 });
    let events = 0;
    rec.onMicSilence(() => events++);
    await rec.start();

    // Feed frames that are BELOW the VAD threshold but carry a nonzero sample
    // (room tone). The device is alive, so mic-silence must NOT fire even though
    // these would count as VAD "silence".
    for (let i = 0; i < 6; i++) {
      pushFrame(rec, 1); // tiny nonzero -> RMS ~3e-5, well under 0.01, but alive
      now += 100;
      await delay(80);
    }
    assert.equal(events, 0, 'a live-but-quiet mic does not false-trigger the muted event');

    rec.stop();
  } finally {
    env.restore();
  }
});

test('D3b: a single loud blip does NOT start speech (needs sustained-loud >= vadStartSecs)', async () => {
  let now = 3_000_000;
  const env = installEnv({ clock: () => now });
  try {
    // vadStartSecs=0.2 (200ms). One 20ms loud frame is far short of that.
    const rec = new AudioRecorder({ sampleRate: 16000, vadStartSecs: 0.2, vadStopSecs: 0.8 });
    let speeches = 0;
    rec.onSpeech(() => speeches++);
    await rec.start();

    pushFrame(rec, 8000); // one loud blip @ t=0
    now += 20;
    pushFrame(rec, 0); // immediately quiet again
    assert.equal(speeches, 0, 'a single loud blip must not trigger a speech-start edge');

    rec.stop();
  } finally {
    env.restore();
  }
});

test('D3b: sustained loud crosses vadStartSecs -> exactly one speech-start edge', async () => {
  let now = 4_000_000;
  const env = installEnv({ clock: () => now });
  try {
    const rec = new AudioRecorder({ sampleRate: 16000, vadStartSecs: 0.2, vadStopSecs: 0.8 });
    let speeches = 0;
    rec.onSpeech(() => speeches++);
    await rec.start();

    // Feed loud frames spanning > 200ms of wall time.
    for (let t = 0; t <= 240; t += 20) {
      pushFrame(rec, 8000);
      now += 20;
    }
    assert.equal(speeches, 1, 'sustained loud beyond vadStartSecs fires speech-start exactly once');

    rec.stop();
  } finally {
    env.restore();
  }
});

test('D3b: a brief mid-utterance dip does NOT stop speech (needs sustained-quiet >= vadStopSecs)', async () => {
  let now = 5_000_000;
  const env = installEnv({ clock: () => now });
  try {
    const rec = new AudioRecorder({ sampleRate: 16000, vadStartSecs: 0.1, vadStopSecs: 0.8 });
    let speeches = 0;
    let silences = 0;
    rec.onSpeech(() => speeches++);
    rec.onSilence(() => silences++);
    await rec.start();

    // Start speech (sustained loud > 100ms).
    for (let t = 0; t <= 140; t += 20) {
      pushFrame(rec, 8000);
      now += 20;
    }
    assert.equal(speeches, 1, 'speech started');

    // A brief 100ms quiet dip (< vadStopSecs 800ms) must NOT end speech.
    for (let t = 0; t < 100; t += 20) {
      pushFrame(rec, 0);
      now += 20;
    }
    assert.equal(silences, 0, 'a short dip does not mis-segment into a silence edge');

    // Loud resumes — still one continuous utterance.
    for (let t = 0; t < 60; t += 20) {
      pushFrame(rec, 8000);
      now += 20;
    }
    assert.equal(silences, 0, 'utterance stays open across the dip');

    // Now a LONG quiet run (> 800ms) actually ends it.
    for (let t = 0; t <= 840; t += 20) {
      pushFrame(rec, 0);
      now += 20;
    }
    assert.equal(silences, 1, 'sustained quiet beyond vadStopSecs fires exactly one silence edge');

    rec.stop();
  } finally {
    env.restore();
  }
});

// --- tiny async helpers ------------------------------------------------------
function delay(ms) {
  return new Promise((r) => setTimeout(r, ms));
}
function waitMicrotasks() {
  return Promise.resolve();
}
