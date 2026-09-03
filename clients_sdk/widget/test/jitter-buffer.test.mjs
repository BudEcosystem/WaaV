/**
 * D9 adaptive jitter buffer.
 *
 * The buffer sits between the websocket onAudio callback and the (D2 scheduled)
 * player. It must:
 *  - PASS THROUGH on a clean network (chunks delivered immediately, zero added
 *    latency) until it actually MEASURES arrival jitter;
 *  - ACTIVATE (re-time delivery) once inter-arrival jitter crosses the threshold,
 *    building a small cushion then draining on a release tick;
 *  - NEVER touch sample data (no resample — the gateway's client_playback_rate is
 *    the only resample stage): a pushed ArrayBuffer reaches the sink byte-identical;
 *  - bound buffered depth (shed oldest under a sustained burst);
 *  - on reset() (barge-in) drop everything WITHOUT forwarding (the player is being
 *    stopped) and return to passthrough.
 *
 * We drive it with an injected clock + injected interval so arrival timing and the
 * release cadence are fully deterministic.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { widget } from './harness.mjs';

const { JitterBuffer } = widget;

const SINK_RATE = 24000;
// One 20ms mono linear16 chunk @24k = 480 samples = 960 bytes.
const CHUNK_BYTES = Math.round(SINK_RATE * 0.02) * 2;

/** A controllable clock + a manual interval registry (one timer for this module). */
function makeHarness() {
  let now = 0;
  const timers = [];
  return {
    now: () => now,
    advance: (ms) => {
      now += ms;
    },
    setIntervalImpl: (cb, ms) => {
      const h = { cb, ms };
      timers.push(h);
      return h;
    },
    clearIntervalImpl: (h) => {
      const i = timers.indexOf(h);
      if (i >= 0) timers.splice(i, 1);
    },
    /** Fire every registered release timer once (one tick). */
    tick: () => {
      for (const t of [...timers]) t.cb();
    },
    timers,
  };
}

function makeChunk(byteLength = CHUNK_BYTES, fill = 1000) {
  const buf = new ArrayBuffer(byteLength);
  const view = new Int16Array(buf);
  for (let i = 0; i < view.length; i++) view[i] = fill;
  return buf;
}

function makeBuffer(h, opts = {}) {
  const out = [];
  const jb = new JitterBuffer(SINK_RATE, 1, (data) => out.push(data), {
    now: h.now,
    setIntervalImpl: h.setIntervalImpl,
    clearIntervalImpl: h.clearIntervalImpl,
    ...opts,
  });
  return { jb, out };
}

test('D9: clean network — every chunk passes straight through (zero added latency)', () => {
  const h = makeHarness();
  const { jb, out } = makeBuffer(h);

  // Feed chunks at EXACTLY the 20ms cadence => no jitter => passthrough.
  for (let i = 0; i < 10; i++) {
    jb.push(makeChunk());
    h.advance(20);
  }

  assert.equal(out.length, 10, 'all chunks forwarded immediately on a clean link');
  assert.equal(jb.isActive, false, 'buffer never activated on a jitter-free stream');
  assert.equal(jb.bufferedMs, 0, 'nothing held back');
});

test('D9: never resamples — forwarded chunk is byte-identical to the pushed chunk', () => {
  const h = makeHarness();
  const { jb, out } = makeBuffer(h);
  const chunk = makeChunk(CHUNK_BYTES, 4242);
  jb.push(chunk);
  assert.equal(out.length, 1);
  assert.equal(out[0].byteLength, chunk.byteLength, 'same byte length (no resample)');
  assert.deepEqual(
    new Int16Array(out[0]),
    new Int16Array(chunk),
    'samples passed through untouched (gateway client_playback_rate is the only resample stage)'
  );
});

test('D9: measured arrival jitter activates re-timing (chunks get held, not instant)', () => {
  const h = makeHarness();
  // Low activation threshold + a visible cushion so the test is deterministic.
  const { jb, out } = makeBuffer(h, { jitterActivateMs: 5, minDepthMs: 40 });

  // Prime the expected cadence with a couple of on-time arrivals.
  jb.push(makeChunk());
  h.advance(20);
  jb.push(makeChunk());

  // Now inject bursty arrivals (alternating very-late / very-early gaps) to push
  // the measured jitter above the 5ms activation threshold.
  const gaps = [60, 2, 60, 2, 60, 2, 60, 2];
  for (const g of gaps) {
    h.advance(g);
    jb.push(makeChunk());
  }

  assert.equal(jb.isActive, true, 'sustained arrival jitter activated the buffer');
  assert.ok(jb.jitterEstimateMs >= 5, 'jitter estimate crossed the activation threshold');
  assert.ok(jb.bufferedMs > 0, 'once active, chunks are held to build a cushion');
});

test('D9: once active, the release tick drains the cushion to the player', () => {
  const h = makeHarness();
  const { jb, out } = makeBuffer(h, {
    jitterActivateMs: 5,
    minDepthMs: 40,
    releaseIntervalMs: 20,
  });

  // Drive it active with bursty arrivals.
  jb.push(makeChunk());
  h.advance(20);
  jb.push(makeChunk());
  for (const g of [60, 2, 60, 2, 60, 2]) {
    h.advance(g);
    jb.push(makeChunk());
  }
  assert.equal(jb.isActive, true);
  const heldBeforeDrain = jb.bufferedMs;
  assert.ok(heldBeforeDrain >= 40, 'cushion filled to >= minDepth before draining');

  // Tick the release timer enough times to drain everything held.
  const forwardedBefore = out.length;
  for (let i = 0; i < 20; i++) h.tick();
  assert.ok(out.length > forwardedBefore, 'release ticks handed held chunks to the player');
  assert.equal(jb.bufferedMs, 0, 'cushion fully drained by the release ticks');
});

test('D9: depth is bounded under a sustained burst (sheds oldest, never grows unbounded)', () => {
  const h = makeHarness();
  const { jb } = makeBuffer(h, {
    jitterActivateMs: 5,
    minDepthMs: 40,
    maxDepthMs: 120,
  });
  // Activate.
  jb.push(makeChunk());
  h.advance(20);
  jb.push(makeChunk());
  for (const g of [60, 2, 60, 2]) {
    h.advance(g);
    jb.push(makeChunk());
  }
  assert.equal(jb.isActive, true);

  // Now flood WITHOUT ticking the release timer: depth must stay <= maxDepthMs.
  for (let i = 0; i < 100; i++) {
    h.advance(2);
    jb.push(makeChunk());
  }
  assert.ok(
    jb.bufferedMs <= 120 + 20,
    `buffered depth bounded near maxDepthMs (got ${jb.bufferedMs}ms)`
  );
});

test('D9: empty/keepalive frames pass through and never perturb jitter', () => {
  const h = makeHarness();
  const { jb, out } = makeBuffer(h, { jitterActivateMs: 5 });
  jb.push(new ArrayBuffer(0)); // keepalive-style empty frame
  jb.push(new ArrayBuffer(0));
  assert.equal(out.length, 2, 'empty frames forwarded straight through');
  assert.equal(jb.isActive, false, 'empty frames do not move the jitter estimate');
  assert.equal(jb.jitterEstimateMs, 0);
});

test('D9: reset() (barge-in) drops held audio WITHOUT forwarding it', () => {
  const h = makeHarness();
  const { jb, out } = makeBuffer(h, { jitterActivateMs: 5, minDepthMs: 40 });
  // Activate + accumulate a cushion.
  jb.push(makeChunk());
  h.advance(20);
  jb.push(makeChunk());
  for (const g of [60, 2, 60, 2]) {
    h.advance(g);
    jb.push(makeChunk());
  }
  assert.ok(jb.bufferedMs > 0, 'cushion present before barge-in');
  const forwardedBeforeReset = out.length;

  jb.reset(); // barge-in: player is being stopped, must NOT replay held chunks

  assert.equal(out.length, forwardedBeforeReset, 'reset() forwarded nothing (no replay of cut audio)');
  assert.equal(jb.bufferedMs, 0, 'held audio dropped');
  assert.equal(jb.isActive, false, 'back to passthrough after reset');
  assert.equal(jb.jitterEstimateMs, 0, 'jitter estimate reset for the next utterance');
  // The release timer must be cleared (no stray ticks).
  assert.equal(h.timers.length, 0, 'release timer cleared on reset');
});

test('D9: close() stops the timer and drops residual audio (no forward on teardown)', () => {
  const h = makeHarness();
  const { jb, out } = makeBuffer(h, { jitterActivateMs: 5, minDepthMs: 40 });
  jb.push(makeChunk());
  h.advance(20);
  jb.push(makeChunk());
  for (const g of [60, 2, 60, 2]) {
    h.advance(g);
    jb.push(makeChunk());
  }
  const forwarded = out.length;
  jb.close();
  assert.equal(out.length, forwarded, 'close() did not forward residual audio');
  assert.equal(h.timers.length, 0, 'release timer cleared on close');
});
