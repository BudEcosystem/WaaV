/**
 * WebSocket Reconnection Strategy
 * Implements exponential backoff with jitter
 */

/**
 * Reconnection configuration
 */
export interface ReconnectConfig {
  /**
   * Initial / first-hop delay in milliseconds (default: 200). D4 lowers this
   * from the old 1000ms so a transient blip recovers in well under 300ms; the
   * decorrelated-jitter backoff + 30s ceiling still bound a sustained outage.
   */
  initialDelay?: number;
  /** Maximum delay in milliseconds (default: 30000) */
  maxDelay?: number;
  /** Multiplier for exponential backoff (default: 1.5) */
  multiplier?: number;
  /** Maximum jitter as percentage of delay (default: 0.2) */
  jitter?: number;
  /** Maximum number of attempts (default: Infinity) */
  maxAttempts?: number;
  /** Reset delay after successful connection lasting this long in ms (default: 60000) */
  resetAfter?: number;
  /**
   * D4 quick-failure breaker: a connection that SURVIVES less than this many ms
   * after opening is counted as a "quick failure" (default: 5000). A doomed
   * config (bad key/unfixable error) still handshakes successfully, so survival
   * time — not handshake success — is what detects it.
   */
  minStableMs?: number;
  /**
   * D4 quick-failure breaker: after this many CONSECUTIVE quick failures, stop
   * reconnecting and surface a typed FATAL "gateway unreachable / likely
   * unrecoverable" error instead of storming (default: 3). Ports Pipecat
   * websocket_service.py _MAX_CONSECUTIVE_QUICK_FAILURES.
   */
  maxQuickFailures?: number;
}

/**
 * Default reconnection configuration
 */
export const DEFAULT_RECONNECT_CONFIG: Required<ReconnectConfig> = {
  initialDelay: 200,
  maxDelay: 30000,
  multiplier: 1.5,
  jitter: 0.2,
  maxAttempts: Infinity,
  resetAfter: 60000,
  minStableMs: 5000,
  maxQuickFailures: 3,
};

/**
 * Reconnection state
 */
export interface ReconnectState {
  /** Current attempt number (1-based) */
  attempt: number;
  /** Current delay in milliseconds */
  delay: number;
  /** Timestamp of last successful connection */
  lastConnectedAt: number | null;
  /** Whether reconnection is in progress */
  reconnecting: boolean;
  /** Whether max attempts has been reached */
  exhausted: boolean;
  /** D4: consecutive quick (sub-minStableMs) failures observed. */
  consecutiveQuickFailures: number;
  /** D4: the quick-failure breaker tripped — reconnection has been abandoned. */
  fatal: boolean;
}

/**
 * Reconnection event handlers
 */
export interface ReconnectHandlers {
  /** Called when reconnection attempt starts */
  onReconnecting?: (state: ReconnectState) => void;
  /** Called when reconnection succeeds */
  onReconnected?: (state: ReconnectState) => void;
  /** Called when reconnection fails (single attempt) */
  onReconnectFailed?: (error: Error, state: ReconnectState) => void;
  /** Called when all reconnection attempts exhausted */
  onReconnectExhausted?: (state: ReconnectState) => void;
  /**
   * D4: called when the quick-failure breaker trips (>= maxQuickFailures
   * consecutive sub-minStableMs survivals). Reconnection is abandoned; the host
   * should surface a typed FATAL error. NOT recoverable by further retries.
   */
  onReconnectFatal?: (state: ReconnectState) => void;
}

/**
 * Reconnection strategy with exponential backoff and jitter
 */
export class ReconnectStrategy {
  private config: Required<ReconnectConfig>;
  private state: ReconnectState;
  private handlers: ReconnectHandlers = {};
  private timeoutId: ReturnType<typeof setTimeout> | null = null;
  private aborted = false;
  /** D4: wall-clock time the current socket opened, for survival measurement. */
  private connectedAt: number | null = null;
  /** D4: one-shot "reset-after-stable" timer armed on each markConnected(). */
  private stableTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(config: ReconnectConfig = {}) {
    this.config = { ...DEFAULT_RECONNECT_CONFIG, ...config };
    this.state = this.createInitialState();
  }

  /**
   * Create initial state
   */
  private createInitialState(): ReconnectState {
    return {
      attempt: 0,
      delay: this.config.initialDelay,
      lastConnectedAt: null,
      reconnecting: false,
      exhausted: false,
      consecutiveQuickFailures: 0,
      fatal: false,
    };
  }

  /**
   * Get current state
   */
  getState(): ReconnectState {
    return { ...this.state };
  }

  /**
   * Set event handlers
   */
  setHandlers(handlers: ReconnectHandlers): void {
    this.handlers = { ...this.handlers, ...handlers };
  }

  /**
   * Calculate the next delay using DECORRELATED jitter (AWS "Exponential
   * Backoff And Jitter"): sleep = min(cap, random_between(base, prev * 3)).
   * This spreads retries far better than `2^attempt ± jitter` (which compounds
   * into a synchronized storm) while keeping the 30s ceiling, and never drops
   * below the fast first-hop `initialDelay`. D4(b).
   */
  private calculateNextDelay(): number {
    const base = this.config.initialDelay;
    const prev = this.state.delay;
    const upper = Math.max(base, prev * 3);
    // Uniformly sample in [base, upper], then clamp to the ceiling.
    const sampled = base + Math.random() * (upper - base);
    return Math.round(Math.min(this.config.maxDelay, Math.max(base, sampled)));
  }

  /**
   * D4 quick-failure breaker: record that the live socket just closed. If it
   * survived less than `minStableMs`, count a consecutive quick failure; a
   * stable-enough connection resets the counter. Must be called by the host on
   * EVERY post-open close, BEFORE scheduling the next reconnect, so the breaker
   * can gate the fast path. Returns the updated consecutive-quick-failure count.
   */
  noteConnectionClosed(): number {
    if (this.connectedAt !== null) {
      const survived = Date.now() - this.connectedAt;
      if (survived < this.config.minStableMs) {
        this.state.consecutiveQuickFailures++;
      } else {
        this.state.consecutiveQuickFailures = 0;
      }
      this.connectedAt = null;
    }
    return this.state.consecutiveQuickFailures;
  }

  /**
   * Schedule a reconnection attempt
   * @param connectFn Function that attempts to connect (returns Promise)
   * @returns Promise that resolves when connection succeeds or rejects when exhausted
   */
  async scheduleReconnect(connectFn: () => Promise<void>): Promise<void> {
    // Use iterative approach to avoid stack overflow from recursive calls
    while (true) {
      if (this.aborted) {
        throw new Error('Reconnection was aborted');
      }

      if (this.state.exhausted) {
        throw new Error('Reconnection attempts exhausted');
      }

      // D4 quick-failure breaker (gates the fast path too): if too many
      // consecutive connections died almost immediately, the config is likely
      // doomed (bad key / unfixable). STOP storming and surface a FATAL error.
      if (this.state.consecutiveQuickFailures >= this.config.maxQuickFailures) {
        this.state.fatal = true;
        this.state.reconnecting = false;
        this.handlers.onReconnectFatal?.(this.getState());
        throw new Error(
          `Reconnection abandoned after ${this.state.consecutiveQuickFailures} consecutive quick failures (connection unstable < ${this.config.minStableMs}ms each); the gateway is likely unreachable or the config is unrecoverable.`
        );
      }

      // Check if we should reset based on last connection time
      if (
        this.state.lastConnectedAt &&
        Date.now() - this.state.lastConnectedAt > this.config.resetAfter
      ) {
        this.reset();
      }

      this.state.attempt++;
      this.state.reconnecting = true;

      // Check max attempts
      if (this.state.attempt > this.config.maxAttempts) {
        this.state.exhausted = true;
        this.state.reconnecting = false;
        this.handlers.onReconnectExhausted?.(this.getState());
        throw new Error('Maximum reconnection attempts reached');
      }

      this.handlers.onReconnecting?.(this.getState());

      // Wait for delay before attempting
      await this.wait(this.state.delay);

      if (this.aborted) {
        throw new Error('Reconnection was aborted');
      }

      try {
        await connectFn();

        // Success
        this.state.lastConnectedAt = Date.now();
        this.state.reconnecting = false;
        this.handlers.onReconnected?.(this.getState());

        // Reset delay for next time (but keep attempt count for metrics)
        this.state.delay = this.config.initialDelay;

        // Exit loop on success
        return;
      } catch (err) {
        const error = err instanceof Error ? err : new Error(String(err));
        this.handlers.onReconnectFailed?.(error, this.getState());

        // Calculate next delay
        this.state.delay = this.calculateNextDelay();

        // Continue loop for next attempt (no recursive call)
      }
    }
  }

  /**
   * Wait for specified duration
   */
  private wait(ms: number): Promise<void> {
    return new Promise((resolve, reject) => {
      this.timeoutId = setTimeout(() => {
        this.timeoutId = null;
        if (this.aborted) {
          reject(new Error('Reconnection was aborted'));
        } else {
          resolve();
        }
      }, ms);
    });
  }

  /**
   * Abort any pending reconnection
   */
  abort(): void {
    this.aborted = true;
    if (this.timeoutId) {
      clearTimeout(this.timeoutId);
      this.timeoutId = null;
    }
    if (this.stableTimer) {
      clearTimeout(this.stableTimer);
      this.stableTimer = null;
    }
    this.state.reconnecting = false;
  }

  /**
   * Reset state for fresh reconnection series (also clears the quick-failure
   * breaker counters and the survival/stable timers).
   */
  reset(): void {
    this.aborted = false;
    this.connectedAt = null;
    this.state = this.createInitialState();
    if (this.timeoutId) {
      clearTimeout(this.timeoutId);
      this.timeoutId = null;
    }
    if (this.stableTimer) {
      clearTimeout(this.stableTimer);
      this.stableTimer = null;
    }
  }

  /**
   * Mark a successful (re)connection — call from the host's open handler. Starts
   * the survival clock for the quick-failure breaker and arms a one-shot
   * "reset-after-stable" timer: if THIS connection lives at least `minStableMs`,
   * the consecutive-quick-failure counter is cleared (a genuinely recovered
   * link should not inherit a near-trip count). The host clears the timer on
   * close via noteConnectionClosed()/markDisconnected() implicitly (a new
   * markConnected re-arms it; abort()/reset() cancel it).
   */
  markConnected(): void {
    this.state.lastConnectedAt = Date.now();
    this.connectedAt = Date.now();
    this.state.reconnecting = false;
    this.state.delay = this.config.initialDelay;

    if (this.stableTimer) clearTimeout(this.stableTimer);
    this.stableTimer = setTimeout(() => {
      this.stableTimer = null;
      // Still up after minStableMs -> the link is healthy; forgive prior quick
      // failures so a later unrelated blip starts from a clean breaker.
      if (this.connectedAt !== null) {
        this.state.consecutiveQuickFailures = 0;
      }
    }, this.config.minStableMs);
  }

  /**
   * Check if should attempt reconnection
   */
  shouldReconnect(): boolean {
    return !this.aborted && !this.state.exhausted && !this.state.fatal;
  }

  /**
   * Get estimated time until next attempt (if reconnecting)
   */
  getTimeUntilNextAttempt(): number | null {
    if (!this.state.reconnecting || !this.timeoutId) {
      return null;
    }
    return this.state.delay;
  }
}
