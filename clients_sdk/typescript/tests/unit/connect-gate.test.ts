/**
 * D7 WS connect-concurrency gate tests.
 *
 * Asserts: no more than `maxConcurrent` connect thunks run at once (a burst
 * queues), the slot is released even when a thunk throws, and min-spacing paces
 * successive starts. Uses a manual gate of the thunks' resolution to observe
 * concurrency.
 */
import { describe, it, expect } from 'vitest';
import { ConnectGate } from '../../src/ws/connect-gate.js';

describe('D7 connect gate — concurrency cap', () => {
  it('never runs more than maxConcurrent thunks at once', async () => {
    const gate = new ConnectGate({ maxConcurrent: 2, minSpacingMs: 0 });
    let active = 0;
    let peak = 0;
    const release: Array<() => void> = [];

    const make = () =>
      gate.run(
        () =>
          new Promise<void>((resolve) => {
            active++;
            peak = Math.max(peak, active);
            release.push(() => {
              active--;
              resolve();
            });
          }),
      );

    const flush = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };

    const p1 = make();
    const p2 = make();
    const p3 = make(); // should queue behind the 2 in flight
    const p4 = make();

    await flush();
    // Only 2 may be active; the gate reports 2 queued.
    expect(active).toBe(2);
    expect(gate.getActiveCount()).toBe(2);
    expect(gate.getQueuedCount()).toBe(2);

    // Release one → the next queued starts.
    release.shift()!();
    await flush();
    expect(active).toBe(2);

    // Drain the rest.
    while (release.length) {
      release.shift()!();
      await flush();
    }
    await Promise.all([p1, p2, p3, p4]);
    expect(peak).toBe(2); // concurrency never exceeded the cap
    expect(gate.getActiveCount()).toBe(0);
  });

  it('releases the slot even when the thunk throws', async () => {
    const gate = new ConnectGate({ maxConcurrent: 1, minSpacingMs: 0 });
    await expect(gate.run(async () => { throw new Error('boom'); })).rejects.toThrow('boom');
    // Slot freed → a subsequent run proceeds.
    const ok = await gate.run(async () => 42);
    expect(ok).toBe(42);
    expect(gate.getActiveCount()).toBe(0);
  });
});

describe('D7 connect gate — min spacing', () => {
  it('paces successive starts by minSpacingMs (real-clock timing)', async () => {
    const gate = new ConnectGate({ maxConcurrent: 4, minSpacingMs: 40 });
    const startTimes: number[] = [];
    const t0 = Date.now();
    const make = () => gate.run(async () => { startTimes.push(Date.now() - t0); });

    await Promise.all([make(), make(), make()]);

    expect(startTimes.length).toBe(3);
    // The 3rd start is paced well past the 1st (≥ ~1 spacing of real delay).
    expect(startTimes[2]!).toBeGreaterThanOrEqual(35);
    // Starts are monotonically non-decreasing (paced, never reordered earlier).
    expect(startTimes[1]!).toBeGreaterThanOrEqual(startTimes[0]!);
    expect(startTimes[2]!).toBeGreaterThanOrEqual(startTimes[1]!);
  });

  it('with minSpacingMs=0 runs back-to-back (no pacing delay)', async () => {
    const gate = new ConnectGate({ maxConcurrent: 4, minSpacingMs: 0 });
    const order: number[] = [];
    await Promise.all([1, 2, 3].map((n) => gate.run(async () => { order.push(n); })));
    expect(order.sort()).toEqual([1, 2, 3]);
  });
});
