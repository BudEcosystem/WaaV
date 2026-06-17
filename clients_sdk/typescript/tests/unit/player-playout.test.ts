/**
 * D2 player tests: 20ms scheduled-chunk playout + TRUE barge-in.
 *
 * A minimal mock AudioContext records every source.start(when) and source.stop()
 * so we can assert: (a) chunks are scheduled at the playout clock
 * (nextStart = max(currentTime, prevStart + prevDuration)), contiguous and never
 * in the past; and (b) a barge-in (interrupt / clearBuffer) stops EVERY
 * scheduled source (the bug the old player had — it never stopped scheduled
 * nodes) and clears the queue, with onClear firing only for interrupt().
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { PCMPlayer } from '../../src/audio/player.js';

class MockBufferSource {
  buffer: MockAudioBuffer | null = null;
  onended: (() => void) | null = null;
  started: number | null = null;
  stopped = false;
  connect = vi.fn();
  disconnect = vi.fn();
  constructor(private ctx: MockAudioContext) {}
  start(when = 0): void {
    this.started = when;
    this.ctx.starts.push(when);
  }
  stop(): void {
    if (this.stopped) throw new Error('already stopped'); // mirror Web Audio
    this.stopped = true;
    this.ctx.stops++;
  }
}

class MockAudioBuffer {
  private data: Float32Array;
  constructor(public numberOfChannels: number, public length: number, public sampleRate: number) {
    this.data = new Float32Array(length);
  }
  getChannelData(): Float32Array {
    return this.data;
  }
}

class MockGain {
  gain = { value: 1 };
  connect = vi.fn();
  disconnect = vi.fn();
}

class MockAudioContext {
  state = 'running';
  currentTime = 0;
  destination = {};
  sampleRate: number;
  starts: number[] = [];
  stops = 0;
  sources: MockBufferSource[] = [];
  constructor(opts?: { sampleRate?: number }) {
    this.sampleRate = opts?.sampleRate ?? 48000;
  }
  createGain(): MockGain {
    return new MockGain();
  }
  createBuffer(ch: number, len: number, rate: number): MockAudioBuffer {
    return new MockAudioBuffer(ch, len, rate);
  }
  createBufferSource(): MockBufferSource {
    const s = new MockBufferSource(this);
    this.sources.push(s);
    return s;
  }
  async resume(): Promise<void> {
    this.state = 'running';
  }
  async close(): Promise<void> {}
}

let lastCtx: MockAudioContext;

beforeEach(() => {
  lastCtx = undefined as unknown as MockAudioContext;
  (globalThis as unknown as { AudioContext: unknown }).AudioContext = class extends MockAudioContext {
    constructor(opts?: { sampleRate?: number }) {
      super(opts);
      lastCtx = this;
    }
  };
});
afterEach(() => {
  delete (globalThis as unknown as { AudioContext?: unknown }).AudioContext;
});

/** 20ms @ 24k = 480 samples per chunk. */
function chunk(samples = 480): Float32Array {
  const f = new Float32Array(samples);
  for (let i = 0; i < samples; i++) f[i] = 0.1;
  return f;
}

describe('D2 scheduled playout', () => {
  it('schedules each chunk at the playout clock (contiguous, never in the past)', async () => {
    const player = new PCMPlayer({ sampleRate: 24000 });
    await player.initialize();
    lastCtx.currentTime = 10; // pretend the clock is at 10s

    const dur = 480 / 24000; // 0.02s
    player.addFloat32(chunk());
    player.addFloat32(chunk());
    player.addFloat32(chunk());
    // addFloat32 is async-internally; let microtasks flush.
    await Promise.resolve();
    await Promise.resolve();

    // First chunk at currentTime (10); each subsequent at prev + duration.
    expect(lastCtx.starts.length).toBe(3);
    expect(lastCtx.starts[0]).toBeCloseTo(10, 5);
    expect(lastCtx.starts[1]).toBeCloseTo(10 + dur, 5);
    expect(lastCtx.starts[2]).toBeCloseTo(10 + 2 * dur, 5);
    // No chunk scheduled before "now".
    for (const w of lastCtx.starts) expect(w).toBeGreaterThanOrEqual(10);
  });

  it('exposes scheduled sources for barge-in', async () => {
    const player = new PCMPlayer({ sampleRate: 24000 });
    await player.initialize();
    player.addFloat32(chunk());
    player.addFloat32(chunk());
    await Promise.resolve();
    await Promise.resolve();
    expect(player.getScheduledCount()).toBe(2);
  });
});

describe('D2 true barge-in', () => {
  it('interrupt() stops ALL scheduled sources, clears the queue, and fires onClear', async () => {
    const onClear = vi.fn();
    const player = new PCMPlayer({ sampleRate: 24000 });
    player.setHandlers({ onClear });
    await player.initialize();

    player.addFloat32(chunk());
    player.addFloat32(chunk());
    player.addFloat32(chunk());
    await Promise.resolve();
    await Promise.resolve();
    expect(lastCtx.sources.length).toBe(3);

    player.interrupt();

    // Every scheduled source was stopped (the bug the old player had).
    expect(lastCtx.stops).toBe(3);
    for (const s of lastCtx.sources) expect(s.stopped).toBe(true);
    // The JS queue / scheduled set is cleared.
    expect(player.getScheduledCount()).toBe(0);
    // onClear fired so the caller can send the gateway `clear` op.
    expect(onClear).toHaveBeenCalledTimes(1);
  });

  it('clearBuffer() stops all scheduled sources but does NOT fire onClear', async () => {
    const onClear = vi.fn();
    const player = new PCMPlayer({ sampleRate: 24000 });
    player.setHandlers({ onClear });
    await player.initialize();
    player.addFloat32(chunk());
    player.addFloat32(chunk());
    await Promise.resolve();
    await Promise.resolve();

    player.clearBuffer();
    expect(lastCtx.stops).toBe(2);
    expect(player.getScheduledCount()).toBe(0);
    expect(onClear).not.toHaveBeenCalled();
  });

  it('double-stop is guarded (no throw when a source already ended)', async () => {
    const player = new PCMPlayer({ sampleRate: 24000 });
    await player.initialize();
    player.addFloat32(chunk());
    await Promise.resolve();
    await Promise.resolve();
    // Simulate the source ending naturally (sets stopped via our mock path):
    const src = lastCtx.sources[0]!;
    src.stop();
    // Now barge-in: clearBuffer must swallow the second stop() throw.
    expect(() => player.clearBuffer()).not.toThrow();
    expect(player.getScheduledCount()).toBe(0);
  });
});
