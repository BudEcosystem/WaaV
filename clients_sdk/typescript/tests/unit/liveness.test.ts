/**
 * D1 liveness / zombie-watchdog tests.
 *
 * Drives LivenessWatchdog directly through injected hooks + an injectable clock
 * + vitest fake timers, so both the NODE (native ping + pong deadline) and the
 * BROWSER (stale-inbound) mechanisms are covered deterministically without a
 * real socket. Asserts: a missed pong fires the zombie callback; an inbound
 * frame / pong satisfies the deadline; stale inbound fires; live traffic does
 * not; and the resume probe reconnects when inbound is stale.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { LivenessWatchdog } from '../../src/ws/liveness.js';

describe('LivenessWatchdog (Node native-ping mode)', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('fires zombie when no pong arrives within the deadline', () => {
    let now = 0;
    const onZombie = vi.fn();
    const wd = new LivenessWatchdog(
      {
        ping: () => true, // pretend a ping was sent; the peer never pongs
        isPingCapable: () => true,
        now: () => now,
        onZombie,
      },
      { pingIntervalMs: 5000, pongTimeoutMs: 3000 }
    );
    expect(wd.isNodeMode()).toBe(true);
    wd.start();

    // First ping fires at t=5000; pong deadline is +3000.
    now = 5000;
    vi.advanceTimersByTime(5000);
    expect(onZombie).not.toHaveBeenCalled();

    // No pong within 3000ms -> zombie.
    now = 8000;
    vi.advanceTimersByTime(3000);
    expect(onZombie).toHaveBeenCalledTimes(1);
    expect(onZombie.mock.calls[0]![0]).toMatch(/pong/i);
  });

  it('does NOT fire when a pong arrives before the deadline', () => {
    let now = 0;
    const onZombie = vi.fn();
    const wd = new LivenessWatchdog(
      { ping: () => true, isPingCapable: () => true, now: () => now, onZombie },
      { pingIntervalMs: 5000, pongTimeoutMs: 3000 }
    );
    wd.start();

    now = 5000;
    vi.advanceTimersByTime(5000); // ping sent, deadline armed
    now = 6000;
    wd.markPong(); // pong arrives within 3000ms
    now = 9000;
    vi.advanceTimersByTime(3000); // deadline window passes
    expect(onZombie).not.toHaveBeenCalled();
  });

  it('stops firing after stop()', () => {
    const onZombie = vi.fn();
    const wd = new LivenessWatchdog(
      { ping: () => true, isPingCapable: () => true, onZombie },
      { pingIntervalMs: 1000, pongTimeoutMs: 500 }
    );
    wd.start();
    wd.stop();
    vi.advanceTimersByTime(10000);
    expect(onZombie).not.toHaveBeenCalled();
  });
});

describe('LivenessWatchdog (browser stale-inbound mode)', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function makeBrowserWatchdog(onZombie: () => void, nowRef: { t: number }) {
    return new LivenessWatchdog(
      {
        ping: () => false, // browser cannot ping
        isPingCapable: () => false,
        now: () => nowRef.t,
        onZombie,
      },
      { staleInboundMs: 12000, resumeProbeStaleMs: 2000 }
    );
  }

  it('fires zombie when no inbound frame for the stale window', () => {
    const nowRef = { t: 0 };
    const onZombie = vi.fn();
    const wd = makeBrowserWatchdog(onZombie, nowRef);
    expect(wd.isNodeMode()).toBe(false);
    wd.start();

    // Advance past the 12s stale window with no inbound.
    nowRef.t = 13000;
    vi.advanceTimersByTime(13000);
    expect(onZombie).toHaveBeenCalledTimes(1);
    expect(onZombie.mock.calls[0]![0]).toMatch(/inbound/i);
  });

  it('does NOT fire while inbound frames keep arriving', () => {
    const nowRef = { t: 0 };
    const onZombie = vi.fn();
    const wd = makeBrowserWatchdog(onZombie, nowRef);
    wd.start();

    // Every 4s an inbound frame resets the staleness clock.
    for (let step = 0; step < 6; step++) {
      nowRef.t += 4000;
      vi.advanceTimersByTime(4000);
      wd.markInbound();
    }
    expect(onZombie).not.toHaveBeenCalled();
  });

  it('probeNow() reconnects when inbound is stale on resume', () => {
    const nowRef = { t: 0 };
    const onZombie = vi.fn();
    const wd = makeBrowserWatchdog(onZombie, nowRef);
    wd.start();

    // 3s since last inbound (> resumeProbeStaleMs 2000) -> probe reconnects.
    nowRef.t = 3000;
    wd.probeNow();
    expect(onZombie).toHaveBeenCalledTimes(1);
    expect(onZombie.mock.calls[0]![0]).toMatch(/resume/i);
  });

  it('probeNow() does NOT reconnect when inbound is fresh', () => {
    const nowRef = { t: 0 };
    const onZombie = vi.fn();
    const wd = makeBrowserWatchdog(onZombie, nowRef);
    wd.start();

    nowRef.t = 1000; // < resumeProbeStaleMs 2000
    wd.probeNow();
    expect(onZombie).not.toHaveBeenCalled();
  });
});
