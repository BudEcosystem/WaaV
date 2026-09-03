/**
 * WS connect-concurrency cap (D7).
 *
 * The gateway runs a per-IP token bucket (default 60 rps / burst 10, keyed by
 * peer IP — it applies to WS upgrades too, main.rs:273) and a global connection
 * cap. A burst of sessions all calling `connect()` at once self-DDoSes into a
 * multi-minute 429/503 storm (the measured failure: 20-concurrent raw = 3 OK /
 * 17×429). This gate cooperates with that bucket CLIENT-side: it caps how many
 * WS connects run concurrently AND enforces a small minimum spacing between
 * connect starts, so a burst is paced under the gateway's refill rate instead of
 * stampeding it.
 *
 * It is a plain async semaphore + a paced release; share ONE instance across all
 * sessions from a client (the gateway limit is per-IP, so the cap is per-client,
 * not per-session). Pure timers/promises (no socket), unit-testable.
 */

/** Tunables for the connect gate. */
export interface ConnectGateConfig {
  /**
   * Max WS connects allowed in flight at once (default 4). Kept well under the
   * gateway burst (10) so retries + the steady connect stream share headroom.
   */
  maxConcurrent?: number;
  /**
   * Minimum spacing between successive connect STARTS, ms (default 50). At the
   * gateway's 60 rps refill that's ~3 connects per bucket-refill window — paced,
   * never bursty. Set 0 to disable spacing (concurrency cap only).
   */
  minSpacingMs?: number;
}

export type ResolvedConnectGateConfig = Required<ConnectGateConfig>;

export const DEFAULT_CONNECT_GATE_CONFIG: ResolvedConnectGateConfig = {
  maxConcurrent: 4,
  minSpacingMs: 50,
};

/** A monotonic clock (injectable for tests). */
function defaultNow(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}

/**
 * Caps concurrent WS connects and paces their starts. Call `run(fn)` with the
 * actual connect thunk; it resolves with the thunk's result once a slot is free
 * and the spacing has elapsed, and always releases the slot afterward.
 */
export class ConnectGate {
  private readonly config: ResolvedConnectGateConfig;
  private readonly now: () => number;
  private active = 0;
  private lastStart = 0;
  /** FIFO of waiters released as slots free up. */
  private readonly waiters: Array<() => void> = [];

  constructor(config: ConnectGateConfig = {}, now: () => number = defaultNow) {
    this.config = { ...DEFAULT_CONNECT_GATE_CONFIG, ...config };
    this.now = now;
  }

  /** Connects currently holding a slot (diagnostics/tests). */
  getActiveCount(): number {
    return this.active;
  }

  /** Waiters currently queued for a slot (diagnostics/tests). */
  getQueuedCount(): number {
    return this.waiters.length;
  }

  /**
   * Acquire a slot, enforce the min-spacing, run `fn`, and release the slot
   * (even if `fn` throws). The thunk is the real WS connect.
   */
  async run<T>(fn: () => Promise<T>): Promise<T> {
    await this.acquire();
    try {
      await this.pace();
      return await fn();
    } finally {
      this.release();
    }
  }

  /** Block until a concurrency slot is available, FIFO. */
  private acquire(): Promise<void> {
    if (this.active < this.config.maxConcurrent) {
      this.active++;
      return Promise.resolve();
    }
    return new Promise<void>((resolve) => {
      this.waiters.push(() => {
        this.active++;
        resolve();
      });
    });
  }

  /** Free a slot and wake the next waiter (if any). */
  private release(): void {
    this.active--;
    const next = this.waiters.shift();
    if (next) next();
  }

  /**
   * Enforce the minimum spacing between connect starts. Records the start time
   * and, if the previous start was too recent, waits out the remainder.
   */
  private async pace(): Promise<void> {
    if (this.config.minSpacingMs <= 0) {
      this.lastStart = this.now();
      return;
    }
    const since = this.now() - this.lastStart;
    const wait = this.config.minSpacingMs - since;
    // Reserve this start slot immediately so concurrent acquirers stagger
    // instead of all reading the same stale lastStart and firing together.
    this.lastStart = wait > 0 ? this.lastStart + this.config.minSpacingMs : this.now();
    if (wait > 0) {
      await new Promise((r) => setTimeout(r, wait));
    }
  }
}
