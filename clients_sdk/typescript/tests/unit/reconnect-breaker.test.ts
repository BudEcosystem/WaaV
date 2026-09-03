/**
 * D4 reconnect-hardening tests: quick-failure breaker + fast first-hop +
 * decorrelated-jitter backoff bounds.
 *
 * The breaker is exercised on ReconnectStrategy directly: markConnected() starts
 * the survival clock, noteConnectionClosed() grades the survival, and after
 * `maxQuickFailures` consecutive sub-`minStableMs` survivals shouldReconnect()
 * goes false and scheduleReconnect() throws + fires onReconnectFatal. Time is
 * controlled by mocking Date.now so survivals are exact.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { ReconnectStrategy, DEFAULT_RECONNECT_CONFIG } from '../../src/ws/reconnect.js';

afterEach(() => vi.restoreAllMocks());

/** Drive one connect->die cycle of `survivedMs` and return the strategy. */
function cycle(strategy: ReconnectStrategy, clock: { t: number }, survivedMs: number): void {
  strategy.markConnected();
  clock.t += survivedMs;
  strategy.noteConnectionClosed();
}

describe('D4 quick-failure breaker', () => {
  it('trips after 3 consecutive quick failures and stops reconnecting', () => {
    const clock = { t: 1_000_000 };
    vi.spyOn(Date, 'now').mockImplementation(() => clock.t);

    const strat = new ReconnectStrategy({ minStableMs: 5000, maxQuickFailures: 3 });
    expect(strat.shouldReconnect()).toBe(true);

    // Three sub-5s survivals (doomed config: handshake OK, dies immediately).
    cycle(strat, clock, 500);
    expect(strat.getState().consecutiveQuickFailures).toBe(1);
    expect(strat.shouldReconnect()).toBe(true);

    cycle(strat, clock, 500);
    expect(strat.getState().consecutiveQuickFailures).toBe(2);
    expect(strat.shouldReconnect()).toBe(true);

    cycle(strat, clock, 500);
    expect(strat.getState().consecutiveQuickFailures).toBe(3);
    // The breaker is now armed; shouldReconnect stays true until the schedule
    // loop trips it (fatal is set inside scheduleReconnect).
  });

  it('scheduleReconnect throws FATAL + fires onReconnectFatal once tripped', async () => {
    const clock = { t: 2_000_000 };
    vi.spyOn(Date, 'now').mockImplementation(() => clock.t);

    const onFatal = vi.fn();
    const strat = new ReconnectStrategy({ minStableMs: 5000, maxQuickFailures: 3 });
    strat.setHandlers({ onReconnectFatal: onFatal });

    cycle(strat, clock, 100);
    cycle(strat, clock, 100);
    cycle(strat, clock, 100);
    expect(strat.getState().consecutiveQuickFailures).toBe(3);

    // A connectFn that would "succeed" — but the breaker must abandon BEFORE it.
    const connectFn = vi.fn(async () => {});
    await expect(strat.scheduleReconnect(connectFn)).rejects.toThrow(/quick failures|unreachable/i);
    expect(onFatal).toHaveBeenCalledTimes(1);
    expect(connectFn).not.toHaveBeenCalled();
    expect(strat.getState().fatal).toBe(true);
    expect(strat.shouldReconnect()).toBe(false);
  });

  it('a stable connection resets the consecutive-quick-failure counter', () => {
    const clock = { t: 3_000_000 };
    vi.spyOn(Date, 'now').mockImplementation(() => clock.t);

    const strat = new ReconnectStrategy({ minStableMs: 5000, maxQuickFailures: 3 });
    cycle(strat, clock, 200); // quick #1
    cycle(strat, clock, 200); // quick #2
    expect(strat.getState().consecutiveQuickFailures).toBe(2);

    // A connection that survives >= minStableMs resets the counter on close.
    cycle(strat, clock, 8000);
    expect(strat.getState().consecutiveQuickFailures).toBe(0);

    // So a subsequent quick failure starts the count fresh (no false trip).
    cycle(strat, clock, 200);
    expect(strat.getState().consecutiveQuickFailures).toBe(1);
    expect(strat.shouldReconnect()).toBe(true);
  });
});

describe('D4 fast first-hop + decorrelated-jitter bounds', () => {
  it('defaults to a ~200ms fast first hop (not 1000ms)', () => {
    expect(DEFAULT_RECONNECT_CONFIG.initialDelay).toBe(200);
    const strat = new ReconnectStrategy();
    expect(strat.getState().delay).toBe(200);
  });

  it('decorrelated backoff stays within [initialDelay, maxDelay] and grows', async () => {
    vi.useFakeTimers();
    try {
      const initialDelay = 200;
      const maxDelay = 30000;
      const strat = new ReconnectStrategy({ initialDelay, maxDelay, minStableMs: 5000, maxQuickFailures: 1000 });

      const delays: number[] = [];
      let n = 0;
      strat.setHandlers({
        onReconnectFailed: (_e, s) => {
          delays.push(s.delay);
          if (++n >= 6) strat.abort();
        },
      });

      // connectFn always fails -> the loop computes a sequence of backoffs. Fake
      // timers drive the internal wait()s; advance generously to flush them all.
      const p = strat
        .scheduleReconnect(async () => {
          throw new Error('connect failed');
        })
        .catch((e: unknown) => e); // swallow the abort rejection for assertion below

      // Step time forward in chunks so each awaited wait() resolves.
      for (let i = 0; i < 40 && n < 6; i++) {
        await vi.advanceTimersByTimeAsync(maxDelay);
      }
      const result = await p;
      expect(String(result)).toMatch(/aborted/i);

      expect(delays.length).toBeGreaterThanOrEqual(5);
      for (const d of delays) {
        expect(d).toBeGreaterThanOrEqual(initialDelay);
        expect(d).toBeLessThanOrEqual(maxDelay);
      }
      // Decorrelated jitter trends upward overall (last >= first).
      expect(delays[delays.length - 1]!).toBeGreaterThanOrEqual(delays[0]!);
    } finally {
      vi.useRealTimers();
    }
  });
});
