/**
 * Liveness / zombie-socket watchdog (D1).
 *
 * The gateway NEVER initiates WS pings to clients (it only auto-pongs client
 * pings, handler.rs:683) and exposes no JSON ping op, so a frozen-but-OPEN
 * socket (SIGSTOP'd peer: TCP ESTABLISHED, no FIN/RST) goes undetected — the
 * client keeps reporting `connected` for the full ~300s gateway idle-close. An
 * app-level watchdog is the only way to detect this, and it has a hard runtime
 * split:
 *
 *  - NODE (`ws`): send a native WS ping every `pingIntervalMs`, expect the
 *    matching pong within `pongTimeoutMs`. A missed pong => zombie.
 *  - BROWSER: cannot send raw WS pings, so track the time of the last inbound
 *    frame and, during an active session, declare a zombie when no frame has
 *    arrived for `staleInboundMs` (the gateway emits frequent transcript/audio/
 *    keepalive frames during any live call, well under the 300s idle-close).
 *
 * On a detected zombie the watchdog invokes `onZombie()`, which the session
 * wires to terminate + reconnect. The watchdog is pure timers + injected hooks
 * (no direct socket/DOM access) so it is trivially unit-testable with fake
 * timers.
 */

/** Tunable thresholds for the liveness watchdog. */
export interface LivenessConfig {
  /** NODE: interval between native WS pings, ms (default 5000). */
  pingIntervalMs?: number;
  /** NODE: max wait for a pong after a ping before declaring zombie, ms (default 3000). */
  pongTimeoutMs?: number;
  /**
   * BROWSER: max gap with no inbound frame during an active session before
   * declaring zombie, ms (default 12000 — far under the gateway's 300s idle-close).
   */
  staleInboundMs?: number;
  /**
   * BROWSER: on a resume probe (online/visibilitychange), if the last inbound
   * frame is older than this, proactively reconnect rather than wait out the
   * full staleInboundMs window, ms (default 2000).
   */
  resumeProbeStaleMs?: number;
}

/** Resolved liveness config. */
export type ResolvedLivenessConfig = Required<LivenessConfig>;

export const DEFAULT_LIVENESS_CONFIG: ResolvedLivenessConfig = {
  pingIntervalMs: 5000,
  pongTimeoutMs: 3000,
  staleInboundMs: 12000,
  resumeProbeStaleMs: 2000,
};

/** Hooks the watchdog needs from its host (the session). */
export interface LivenessHooks {
  /**
   * Send one native WS ping (Node). Returns true if a ping was actually sent.
   * Returning false (e.g. browser, or not connected) makes the watchdog skip
   * the ping/pong path for this tick and rely on inbound staleness only.
   */
  ping: () => boolean;
  /** True if this runtime/socket can send native WS pings (Node `ws`). */
  isPingCapable: () => boolean;
  /** Monotonic clock in ms (injectable for tests). Defaults to performance.now(). */
  now?: () => number;
  /** Called when the socket is judged dead. The host should terminate+reconnect. */
  onZombie: (reason: string) => void;
}

/** A monotonic clock that never goes backwards (immune to wall-clock jumps). */
function defaultNow(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}

/**
 * Runs the liveness watchdog for a single connected socket. Create it on
 * `ready` (or open), call `start()`, feed it `markInbound()` on every frame and
 * `markPong()` on every native pong, and `stop()` it on close. One instance per
 * socket lifetime; the session makes a fresh one after each reconnect.
 */
export class LivenessWatchdog {
  private readonly config: ResolvedLivenessConfig;
  private readonly hooks: LivenessHooks;
  private readonly now: () => number;
  private readonly nodeMode: boolean;

  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private pongDeadline: ReturnType<typeof setTimeout> | null = null;
  private staleTimer: ReturnType<typeof setInterval> | null = null;
  private lastInbound: number;
  private awaitingPong = false;
  private running = false;
  private fired = false;

  constructor(hooks: LivenessHooks, config: LivenessConfig = {}) {
    this.config = { ...DEFAULT_LIVENESS_CONFIG, ...config };
    this.hooks = hooks;
    this.now = hooks.now ?? defaultNow;
    // Decide the mechanism ONCE per socket so Node never fights the browser
    // staleness path (avoids a double-close race).
    this.nodeMode = hooks.isPingCapable();
    this.lastInbound = this.now();
  }

  /** Whether this watchdog is using the Node native-ping mechanism. */
  isNodeMode(): boolean {
    return this.nodeMode;
  }

  /** Start the watchdog timers. Idempotent. */
  start(): void {
    if (this.running) return;
    this.running = true;
    this.fired = false;
    this.lastInbound = this.now();

    if (this.nodeMode) {
      // NODE: ping on an interval; arm a pong deadline after each ping.
      this.pingTimer = setInterval(() => this.sendPing(), this.config.pingIntervalMs);
    } else {
      // BROWSER: poll inbound staleness. Poll at a fraction of the window so
      // detection latency is bounded by ~staleInboundMs (+ one poll period).
      const poll = Math.max(1000, Math.floor(this.config.staleInboundMs / 4));
      this.staleTimer = setInterval(() => this.checkStale(), poll);
    }
  }

  /** Stop all timers. Idempotent. Call on socket close/teardown. */
  stop(): void {
    this.running = false;
    this.awaitingPong = false;
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
    if (this.pongDeadline) {
      clearTimeout(this.pongDeadline);
      this.pongDeadline = null;
    }
    if (this.staleTimer) {
      clearInterval(this.staleTimer);
      this.staleTimer = null;
    }
  }

  /**
   * Record that an inbound frame (any JSON or binary message, or a pong)
   * arrived. Resets the browser staleness clock and, in Node, satisfies a
   * pending pong deadline — any inbound traffic proves the socket is alive.
   */
  markInbound(): void {
    this.lastInbound = this.now();
    if (this.awaitingPong) {
      this.clearPongDeadline();
    }
  }

  /** Record a native WS pong (Node). Treated as inbound liveness. */
  markPong(): void {
    this.markInbound();
  }

  /** ms since the last inbound frame (for diagnostics/tests). */
  msSinceLastInbound(): number {
    return this.now() - this.lastInbound;
  }

  /**
   * Fire an immediate liveness probe on resume from a network/visibility change
   * (browser). In Node, ping now (don't wait for the next interval). In the
   * browser, if inbound has been stale beyond `resumeProbeStaleMs`, reconnect
   * proactively rather than wait out the full staleness window.
   */
  probeNow(): void {
    if (!this.running) return;
    if (this.nodeMode) {
      this.sendPing();
    } else if (this.msSinceLastInbound() > this.config.resumeProbeStaleMs) {
      this.fire('resume probe: inbound stale on network/visibility resume');
    }
  }

  private sendPing(): void {
    if (!this.running) return;
    if (this.awaitingPong) {
      // A prior ping is still outstanding; its deadline governs.
      return;
    }
    // Arm the pong deadline BEFORE sending so a synchronously-delivered pong
    // (test mocks; loopback) that lands during ping() still satisfies it via
    // markInbound() — otherwise the pong would be missed and we'd false-trip.
    this.awaitingPong = true;
    this.pongDeadline = setTimeout(() => {
      this.awaitingPong = false;
      this.pongDeadline = null;
      this.fire(`no pong within ${this.config.pongTimeoutMs}ms (zombie socket)`);
    }, this.config.pongTimeoutMs);

    const sent = this.hooks.ping();
    if (!sent) {
      // Couldn't ping (disconnected mid-flight); disarm and let close handling
      // take over rather than false-trip on an un-sent ping.
      this.clearPongDeadline();
    }
  }

  private clearPongDeadline(): void {
    this.awaitingPong = false;
    if (this.pongDeadline) {
      clearTimeout(this.pongDeadline);
      this.pongDeadline = null;
    }
  }

  private checkStale(): void {
    if (!this.running) return;
    if (this.msSinceLastInbound() > this.config.staleInboundMs) {
      this.fire(`no inbound frame for ${this.config.staleInboundMs}ms (zombie socket)`);
    }
  }

  private fire(reason: string): void {
    if (this.fired) return;
    this.fired = true;
    this.stop();
    this.hooks.onZombie(reason);
  }
}
