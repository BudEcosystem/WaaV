/**
 * D2 player: 20ms scheduled-chunk playout + true barge-in.
 *
 * The player must schedule each PCM chunk against a monotonic playout clock
 * (playhead = max(ctx.currentTime, nextStartTime); source.start(playhead);
 * nextStartTime += chunk.duration) and KEEP a handle to every scheduled source so
 * a barge-in stop() can source.stop() ALL of them (cutting audio mid-playout) and
 * drop the queue — neither of which the old concatenate-and-start path did.
 *
 * jsdom has no Web Audio, so we install a minimal fake AudioContext that records
 * each source's scheduled start time + whether it was stopped.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { widget } from './harness.mjs';

const { AudioPlayer } = widget;

const SINK_RATE = 24000;

// --- minimal Web Audio fakes -------------------------------------------------
class FakeBufferSource {
  constructor(ctx) {
    this.ctx = ctx;
    this.buffer = null;
    this.onended = null;
    this.startedAt = null;
    this.stopped = false;
    this._connected = false;
  }
  connect() {
    this._connected = true;
  }
  start(when) {
    this.startedAt = when ?? this.ctx.currentTime;
    this.ctx._started.push(this);
  }
  stop() {
    if (this.stopped) {
      // Mirror the real WebAudio throw-on-double-stop so we exercise the guard.
      throw new Error('cannot stop more than once');
    }
    this.stopped = true;
  }
}

class FakeAudioBuffer {
  constructor(channels, length, sampleRate) {
    this.numberOfChannels = channels;
    this.length = length;
    this.sampleRate = sampleRate;
    this.duration = length / sampleRate;
    this._data = new Float32Array(length);
  }
  getChannelData() {
    return this._data;
  }
}

class FakeGain {
  constructor() {
    this.gain = { value: 1 };
  }
  connect() {}
}

class FakeAudioContext {
  constructor(opts) {
    this.sampleRate = (opts && opts.sampleRate) || SINK_RATE;
    this.currentTime = 0;
    this.state = 'running';
    this.destination = {};
    this._started = []; // every source that got start()ed
    FakeAudioContext._last = this; // so tests can reach the player's context
  }
  createGain() {
    return new FakeGain();
  }
  createBufferSource() {
    return new FakeBufferSource(this);
  }
  createBuffer(channels, length, sampleRate) {
    return new FakeAudioBuffer(channels, length, sampleRate);
  }
  async resume() {
    this.state = 'running';
  }
  async close() {
    this.state = 'closed';
  }
}

// One 20ms chunk @ 24k = 480 samples = 960 bytes of Int16 PCM.
function chunk20ms(rate = SINK_RATE) {
  const samples = Math.round(rate * 0.02);
  const buf = new ArrayBuffer(samples * 2);
  const view = new Int16Array(buf);
  for (let i = 0; i < samples; i++) view[i] = 1000; // non-silent
  return buf;
}

async function withFakeAudio(fn) {
  const prev = globalThis.AudioContext;
  globalThis.AudioContext = FakeAudioContext;
  try {
    return await fn();
  } finally {
    globalThis.AudioContext = prev;
  }
}

test('D2: chunks are scheduled back-to-back on the playout clock (no overlap/gap)', async () => {
  await withFakeAudio(async () => {
    const player = new AudioPlayer({ sampleRate: SINK_RATE });
    await player.initialize();

    await player.play(chunk20ms());
    await player.play(chunk20ms());
    await player.play(chunk20ms());

    const started = collectStarted(player);
    assert.equal(started.length, 3, 'three sources scheduled');

    const dur = Math.round(SINK_RATE * 0.02) / SINK_RATE; // 0.02s
    // First pins to currentTime (0), each next starts exactly one chunk later.
    assert.ok(Math.abs(started[0].startedAt - 0) < 1e-9, 'first chunk at currentTime');
    assert.ok(Math.abs(started[1].startedAt - dur) < 1e-9, 'second chunk one chunk later');
    assert.ok(Math.abs(started[2].startedAt - 2 * dur) < 1e-9, 'third chunk two chunks later');
  });
});

test('D2: playhead pins to currentTime after an underrun (does not schedule in the past)', async () => {
  await withFakeAudio(async () => {
    const player = new AudioPlayer({ sampleRate: SINK_RATE });
    await player.initialize();
    const ctx = getCtx(player);

    await player.play(chunk20ms()); // scheduled at t=0..0.02
    // Wall clock advances WELL past the scheduled tail (underrun gap).
    ctx.currentTime = 5.0;
    await player.play(chunk20ms());

    const started = collectStarted(player);
    assert.equal(started.length, 2);
    assert.ok(
      Math.abs(started[1].startedAt - 5.0) < 1e-9,
      'after an underrun the next chunk starts at currentTime, not stacked behind the old tail'
    );
  });
});

test('D2: barge-in stop() stops ALL scheduled sources and clears the clock', async () => {
  await withFakeAudio(async () => {
    const player = new AudioPlayer({ sampleRate: SINK_RATE });
    await player.initialize();

    await player.play(chunk20ms());
    await player.play(chunk20ms());
    await player.play(chunk20ms());
    const started = collectStarted(player);
    assert.equal(started.length, 3);
    assert.equal(player.playing, true, 'player reports playing with sources scheduled');

    player.stop(); // barge-in

    for (const s of started) {
      assert.equal(s.stopped, true, 'every scheduled source was source.stop()ped');
    }
    assert.equal(player.playing, false, 'queue/clock cleared after barge-in');

    // The next chunk after barge-in starts cleanly at currentTime (clock reset).
    const ctx = getCtx(player);
    ctx.currentTime = 10.0;
    await player.play(chunk20ms());
    const after = collectStarted(player).filter((s) => !s.stopped);
    assert.equal(after.length, 1);
    assert.ok(Math.abs(after[0].startedAt - 10.0) < 1e-9, 'post-barge-in chunk starts at currentTime');
  });
});

test('D2: stop() is safe when sources already ended (no double-stop throw escapes)', async () => {
  await withFakeAudio(async () => {
    const player = new AudioPlayer({ sampleRate: SINK_RATE });
    await player.initialize();
    await player.play(chunk20ms());
    const started = collectStarted(player);
    // Simulate the source already having ended + been stopped by something else.
    started[0].stopped = true;
    // Must not throw despite the underlying stop() throwing on a stopped source.
    assert.doesNotThrow(() => player.stop());
    assert.equal(player.playing, false);
  });
});

test('D2: a zero-length chunk does not schedule a source or advance the clock', async () => {
  await withFakeAudio(async () => {
    const player = new AudioPlayer({ sampleRate: SINK_RATE });
    await player.initialize();
    await player.play(new ArrayBuffer(0)); // empty keepalive-style frame
    assert.equal(collectStarted(player).length, 0, 'no source scheduled for an empty chunk');
    assert.equal(player.playing, false);
  });
});

test('D2: onPlaybackEnd fires once the scheduled queue fully drains', async () => {
  await withFakeAudio(async () => {
    const player = new AudioPlayer({ sampleRate: SINK_RATE });
    await player.initialize();
    let ended = 0;
    player.onPlaybackEnd(() => {
      ended++;
    });
    await player.play(chunk20ms());
    await player.play(chunk20ms());
    const started = collectStarted(player);
    // Fire onended in order; only the LAST drain should trigger the callback.
    started[0].onended && started[0].onended();
    assert.equal(ended, 0, 'not done while a source remains scheduled');
    started[1].onended && started[1].onended();
    assert.equal(ended, 1, 'end callback fires exactly once when the queue drains');
  });
});

// --- helpers to reach the player's (fake) AudioContext + scheduled sources ----
// The player keeps its AudioContext private; we recover the SAME fake instance by
// reading the gain node's owning context off the first created source. Simplest:
// the player created the context, and our FakeAudioContext records every started
// source on `_started`. We locate that context via a WeakRef-free probe: the
// player exposes nothing, so we patch createBufferSource bookkeeping through the
// context the player built. To keep this robust we stash the latest context.
function getCtx(player) {
  // FakeAudioContext records itself as the last constructed instance.
  return FakeAudioContext._last;
}
function collectStarted(player) {
  return getCtx(player)._started;
}
