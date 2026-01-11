/**
 * WebSocket Reconnection Strategy
 * Implements exponential backoff with jitter
 */

/**
 * Reconnection configuration
 */
export interface ReconnectConfig {
  /** Initial delay in milliseconds (default: 1000) */
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
}

/**
 * Default reconnection configuration
 */
export const DEFAULT_RECONNECT_CONFIG: Required<ReconnectConfig> = {
  initialDelay: 1000,
  maxDelay: 30000,
  multiplier: 1.5,
  jitter: 0.2,
  maxAttempts: Infinity,
  resetAfter: 60000,
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
   * Calculate next delay with jitter
   */
  private calculateNextDelay(): number {
    // Apply exponential backoff
    const baseDelay = Math.min(
      this.state.delay * this.config.multiplier,
      this.config.maxDelay
    );

    // Apply jitter (random variation to prevent thundering herd)
    const jitterRange = baseDelay * this.config.jitter;
    const jitter = (Math.random() - 0.5) * 2 * jitterRange;

    return Math.round(Math.max(this.config.initialDelay, baseDelay + jitter));
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
    this.state.reconnecting = false;
  }

  /**
   * Reset state for fresh reconnection series
   */
  reset(): void {
    this.aborted = false;
    this.state = this.createInitialState();
    if (this.timeoutId) {
      clearTimeout(this.timeoutId);
      this.timeoutId = null;
    }
  }

  /**
   * Mark successful connection (call after manual connect succeeds)
   */
  markConnected(): void {
    this.state.lastConnectedAt = Date.now();
    this.state.reconnecting = false;
    this.state.delay = this.config.initialDelay;
  }

  /**
   * Check if should attempt reconnection
   */
  shouldReconnect(): boolean {
    return !this.aborted && !this.state.exhausted;
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
