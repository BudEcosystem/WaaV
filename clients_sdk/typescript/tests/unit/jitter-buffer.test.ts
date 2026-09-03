/**
 * D9 adaptive jitter buffer + PLC tests.
 *
 * Drives the buffer with an injectable clock + manual timer queue so engage/
 * passthrough, ordered drain, packet-loss concealment, and time-stretch drain
 * are all deterministic without real timers. Released frames are recorded.
 */
import { describe, it, expect } from 'vitest';
import { JitterBuffer } from '../../src/audio/jitter-buffer.js';

/** A controllable clock + timer queue: tasks are run in due order by advance(). */
class FakeClock {
  t = 0;
  private timers: Array<{ id: number; due: number; cb: () => void }> = [];
  private nextId = 1;
  now = (): number => this.t;
  setTimer = (cb: () => void, ms: number): number => {
    const id = this.nextId++;
    this.timers.push({ id, due: this.t + ms, cb });
    return id;
  };
  clearTimer = (id: number): void => {
    this.timers = this.timers.filter((x) => x.id !== id);
  };
  /** Advance time by `ms`, firing due timers in order (handles re-arming). */
  advance(ms: number): void {
    const target = this.t + ms;
    for (;;) {
      const due = this.timers.filter((x) => x.due <= target).sort((a, b) => a.due - b.due)[0];
      if (!due) break;
      this.timers = this.timers.filter((x) => x.id !== due.id);
      this.t = due.due;
      due.cb();
    }
    this.t = target;
  }
}

/** A distinctively-valued frame so we can identify it after release. */
function frame(value: number, n = 320): Float32Array {
  const f = new Float32Array(n);
  f.fill(value);
  return f;
}

describe('D9 jitter buffer — passthrough on a clean network', () => {
  it('passes frames straight through with zero added latency when arrivals are regular', () => {
    const clock = new FakeClock();
    const released: Float32Array[] = [];
    const jb = new JitterBuffer(
      { sampleRate: 16000, frameMs: 20, engageJitterMs: 8 },
      { release: (f) => released.push(f), now: clock.now, setTimer: clock.setTimer, clearTimer: clock.clearTimer },
    );

    // Perfectly regular 20ms arrivals → jitter stays ~0 → never engages.
    for (let i = 0; i < 10; i++) {
      jb.push(frame(i), i);
      clock.advance(20);
    }

    expect(jb.isEngaged()).toBe(false);
    expect(released.length).toBe(10); // every frame released immediately
    expect(released[0]![0]).toBe(0);
    expect(released[9]![0]).toBe(9);
    expect(jb.getEngageCount()).toBe(0);
  });
});

describe('D9 jitter buffer — engages under arrival jitter', () => {
  it('engages once inter-arrival jitter exceeds the threshold', () => {
    const clock = new FakeClock();
    const released: Float32Array[] = [];
    const jb = new JitterBuffer(
      { sampleRate: 16000, frameMs: 20, engageJitterMs: 8, targetDelayMs: 40 },
      { release: (f) => released.push(f), now: clock.now, setTimer: clock.setTimer, clearTimer: clock.clearTimer },
    );

    // Irregular arrivals: alternating tight/loose gaps drive the jitter estimate up.
    const gaps = [20, 80, 5, 90, 5, 95, 10];
    let seq = 0;
    for (const g of gaps) {
      jb.push(frame(seq), seq);
      seq++;
      clock.advance(g);
    }

    expect(jb.getJitterMs()).toBeGreaterThan(8);
    expect(jb.isEngaged()).toBe(true);
    expect(jb.getEngageCount()).toBeGreaterThanOrEqual(1);
  });

  it('reorders out-of-order frames by sequence number once engaged', () => {
    const clock = new FakeClock();
    const released: Float32Array[] = [];
    const jb = new JitterBuffer(
      { sampleRate: 16000, frameMs: 20, engageJitterMs: 1, targetDelayMs: 60, maxDelayMs: 120 },
      { release: (f) => released.push(f), now: clock.now, setTimer: clock.setTimer, clearTimer: clock.clearTimer },
    );

    // Force engage with a couple of irregular pushes.
    jb.push(frame(0), 0); clock.advance(50);
    jb.push(frame(1), 1); clock.advance(5);
    expect(jb.isEngaged()).toBe(true);

    // Now deliver 4,2,3,5 OUT OF ORDER quickly (seq 2..5).
    jb.push(frame(4), 4); clock.advance(2);
    jb.push(frame(2), 2); clock.advance(2);
    jb.push(frame(3), 3); clock.advance(2);
    jb.push(frame(5), 5); clock.advance(2);

    // Drain the buffer.
    clock.advance(500);

    // Frames after the engage point must come out in ascending seq order.
    const values = released.map((f) => f[0]!);
    // The first two (0,1) were passthrough/early; the reordered tail is monotonic.
    const tail = values.slice(values.indexOf(2));
    const sortedTail = [...tail].sort((a, b) => a - b);
    expect(tail).toEqual(sortedTail);
    expect(values).toContain(2);
    expect(values).toContain(5);
  });
});

describe('D9 jitter buffer — PLC concealment', () => {
  it('conceals a missing frame instead of leaving a gap, and counts it', () => {
    const clock = new FakeClock();
    const released: Float32Array[] = [];
    const jb = new JitterBuffer(
      { sampleRate: 16000, frameMs: 20, engageJitterMs: 1, targetDelayMs: 40, maxConcealFrames: 3 },
      { release: (f) => released.push(f), now: clock.now, setTimer: clock.setTimer, clearTimer: clock.clearTimer },
    );

    // Engage.
    jb.push(frame(0.5), 0); clock.advance(50);
    jb.push(frame(0.5), 1); clock.advance(5);

    // Deliver seq 2 then SKIP 3, deliver 4 and 5 → seq 3 is "lost".
    jb.push(frame(0.5), 2); clock.advance(2);
    jb.push(frame(0.5), 4); clock.advance(2);
    jb.push(frame(0.5), 5); clock.advance(2);

    clock.advance(500); // drain + conceal the gap at seq 3

    expect(jb.getConcealedFrames()).toBeGreaterThanOrEqual(1);
    // A concealed frame is derived from the last real frame faded toward silence:
    // its energy is > 0 (not silence) but ≤ the source frame's level.
    const concealed = released.find((f) => f[0]! > 0 && f[0]! < 0.5);
    expect(concealed).toBeDefined();
  });

  it('emits no concealment beyond the conceal budget', () => {
    const clock = new FakeClock();
    const released: Float32Array[] = [];
    const jb = new JitterBuffer(
      { sampleRate: 16000, frameMs: 20, engageJitterMs: 1, targetDelayMs: 20, maxConcealFrames: 2 },
      { release: (f) => released.push(f), now: clock.now, setTimer: clock.setTimer, clearTimer: clock.clearTimer },
    );
    jb.push(frame(0.4), 0); clock.advance(50);
    jb.push(frame(0.4), 1); clock.advance(5);
    // Big gap then a far-future frame.
    jb.push(frame(0.4), 2); clock.advance(2);
    jb.push(frame(0.4), 20); clock.advance(2);
    clock.advance(1000);
    // Concealment is capped at maxConcealFrames per gap.
    expect(jb.getConcealedFrames()).toBeLessThanOrEqual(2);
  });
});

describe('D9 jitter buffer — time-stretch drain', () => {
  it('drops samples to drain when over-buffered (latency control)', () => {
    const clock = new FakeClock();
    const released: Float32Array[] = [];
    const jb = new JitterBuffer(
      { sampleRate: 16000, frameMs: 20, engageJitterMs: 1, targetDelayMs: 20, maxDelayMs: 40 },
      { release: (f) => released.push(f), now: clock.now, setTimer: clock.setTimer, clearTimer: clock.clearTimer },
    );

    // Engage, then flood many frames at once so depth >> target (over-buffered).
    jb.push(frame(0.3), 0); clock.advance(50);
    for (let i = 1; i <= 12; i++) {
      // Alternate sign within each frame so there's a zero-crossing to splice at.
      const f = new Float32Array(320);
      for (let j = 0; j < f.length; j++) f[j] = (j % 2 === 0 ? 0.3 : -0.3);
      jb.push(f, i);
    }
    clock.advance(500); // drain

    expect(jb.getDroppedSamples()).toBeGreaterThan(0);
    // The over-buffered drain shortens at least one released frame below 320.
    const shortened = released.some((f) => f.length < 320 && f.length > 0);
    expect(shortened).toBe(true);
  });
});

describe('D9 jitter buffer — reset/flush', () => {
  it('reset() drops everything and returns to passthrough', () => {
    const clock = new FakeClock();
    const released: Float32Array[] = [];
    const jb = new JitterBuffer(
      { sampleRate: 16000, frameMs: 20, engageJitterMs: 1 },
      { release: (f) => released.push(f), now: clock.now, setTimer: clock.setTimer, clearTimer: clock.clearTimer },
    );
    jb.push(frame(1), 0); clock.advance(50);
    jb.push(frame(1), 1); clock.advance(2);
    jb.push(frame(1), 2); // queued while engaged
    jb.reset();
    expect(jb.getDepth()).toBe(0);
    expect(jb.isEngaged()).toBe(false);
    expect(jb.getJitterMs()).toBe(0);
  });
});
