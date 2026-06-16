/**
 * WebSocket Connection Handler
 * Low-level WebSocket connection management
 */

import { ConnectionError, TimeoutError, RateLimitError, parseRetryAfterMs } from '../errors/index.js';
import type { IncomingMessage } from '../types/messages.js';
import { serializeMessage, deserializeMessage, type SDKOutgoingMessage } from './messages.js';

/**
 * Minimal shape of the Node `ws` library's `unexpected-response` event payload.
 * Browsers do not expose the failing HTTP response for a WebSocket upgrade, so
 * 429 detection at connect time is only available under Node (`ws`).
 */
interface UpgradeResponseLike {
  statusCode?: number;
  headers?: Record<string, string | string[] | undefined>;
}

/** Duck-typed EventEmitter `.on` present on the `ws` library's WebSocket. */
interface WsEventEmitterLike {
  on(event: string, listener: (...args: unknown[]) => void): unknown;
}

function hasOn(ws: unknown): ws is WsEventEmitterLike {
  return typeof (ws as { on?: unknown })?.on === 'function';
}

/**
 * WebSocket connection options
 */
export interface WebSocketConnectionOptions {
  /** WebSocket URL (e.g., "ws://localhost:3001/ws") */
  url: string;
  /** Connection timeout in milliseconds (default: 10000) */
  timeout?: number;
  /** Custom WebSocket implementation (for Node.js compatibility) */
  WebSocket?: typeof WebSocket;
  /** Protocol to use (default: none) */
  protocols?: string | string[];
}

/**
 * Connection state
 */
export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'closing';

/**
 * Connection event handlers
 */
export interface ConnectionEventHandlers {
  onOpen?: () => void;
  onClose?: (code: number, reason: string) => void;
  onError?: (error: Error) => void;
  onMessage?: (message: IncomingMessage) => void;
  onBinaryMessage?: (data: ArrayBuffer) => void;
}

/**
 * WebSocket connection wrapper with typed message handling
 */
export class WebSocketConnection {
  private ws: WebSocket | null = null;
  private url: string;
  private timeout: number;
  private WebSocketImpl: typeof WebSocket;
  private protocols?: string | string[];
  private state: ConnectionState = 'disconnected';
  private handlers: ConnectionEventHandlers = {};
  private connectPromise: Promise<void> | null = null;
  private connectResolve: (() => void) | null = null;
  private connectReject: ((error: Error) => void) | null = null;
  /** Captured HTTP 429 from the `ws` upgrade, if the gateway rate-limited us. */
  private rateLimit: { retryAfterMs?: number; retryAfter?: string } | null = null;

  constructor(options: WebSocketConnectionOptions) {
    this.url = options.url;
    this.timeout = options.timeout ?? 10000;
    this.WebSocketImpl = options.WebSocket ?? globalThis.WebSocket;
    this.protocols = options.protocols;

    if (!this.WebSocketImpl) {
      throw new ConnectionError('WebSocket is not available in this environment', {
        url: this.url,
      });
    }
  }

  /**
   * Get current connection state
   */
  getState(): ConnectionState {
    return this.state;
  }

  /**
   * Check if connection is open
   */
  isConnected(): boolean {
    return this.state === 'connected' && this.ws?.readyState === WebSocket.OPEN;
  }

  /**
   * Set event handlers
   */
  setHandlers(handlers: ConnectionEventHandlers): void {
    this.handlers = { ...this.handlers, ...handlers };
  }

  /**
   * Connect to WebSocket server
   */
  async connect(): Promise<void> {
    if (this.state === 'connected') {
      return;
    }

    if (this.state === 'connecting' && this.connectPromise) {
      return this.connectPromise;
    }

    this.state = 'connecting';

    this.connectPromise = new Promise<void>((resolve, reject) => {
      this.connectResolve = resolve;
      this.connectReject = reject;

      // Flag to prevent double callback after timeout or cleanup
      let settled = false;

      const safeResolve = () => {
        if (settled) return;
        settled = true;
        clearTimeout(timeoutId);
        resolve();
      };

      const safeReject = (error: Error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeoutId);
        reject(error);
      };

      const timeoutId = setTimeout(() => {
        if (this.state === 'connecting' && !settled) {
          this.cleanup();
          safeReject(new TimeoutError(`Connection to ${this.url} timed out after ${this.timeout}ms`, this.timeout, {
            operation: 'connect',
          }));
        }
      }, this.timeout);

      try {
        this.rateLimit = null;
        this.ws = new this.WebSocketImpl(this.url, this.protocols);
        this.ws.binaryType = 'arraybuffer';

        // Under Node (`ws`), the HTTP upgrade response is exposed via the
        // `unexpected-response` event. Capture a 429 + Retry-After so the
        // handshake failure can be surfaced as a typed RateLimitError instead
        // of an opaque ConnectionError. (Browsers cannot see this.)
        if (hasOn(this.ws)) {
          this.ws.on('unexpected-response', (...args: unknown[]) => {
            const res = args[1] as UpgradeResponseLike | undefined;
            if (res?.statusCode === 429) {
              const headers = res.headers ?? {};
              const raw = headers['retry-after'] ?? headers['Retry-After'];
              const retryAfter = Array.isArray(raw) ? raw[0] : raw;
              this.rateLimit = {};
              if (retryAfter !== undefined) {
                this.rateLimit.retryAfter = retryAfter;
                const ms = parseRetryAfterMs(retryAfter);
                if (ms !== undefined) this.rateLimit.retryAfterMs = ms;
              }
            }
          });
        }

        this.ws.onopen = () => {
          this.state = 'connected';
          this.handlers.onOpen?.();
          safeResolve();
        };

        this.ws.onclose = (event) => {
          const wasConnecting = this.state === 'connecting';
          this.state = 'disconnected';
          this.handlers.onClose?.(event.code, event.reason);

          if (wasConnecting) {
            safeReject(this.handshakeError(`Connection closed during handshake: ${event.reason || 'Unknown reason'}`, event.code, event.reason));
          }
        };

        this.ws.onerror = () => {
          const error = this.handshakeError('WebSocket error occurred');
          this.handlers.onError?.(error);

          if (this.state === 'connecting') {
            safeReject(error);
          }
        };

        this.ws.onmessage = (event) => {
          this.handleMessage(event.data);
        };
      } catch (err) {
        this.state = 'disconnected';
        const error = err instanceof Error ? err : new Error(String(err));
        safeReject(new ConnectionError(`Failed to create WebSocket: ${error.message}`, {
          url: this.url,
          cause: error,
        }));
      }
    });

    return this.connectPromise;
  }

  /**
   * Build the error to reject a failed handshake with. If a 429 was captured
   * from the upgrade response, returns a typed RateLimitError (carrying the
   * parsed Retry-After); otherwise a generic ConnectionError.
   */
  private handshakeError(message: string, closeCode?: number, closeReason?: string): ConnectionError | RateLimitError {
    if (this.rateLimit) {
      const rl = this.rateLimit;
      return new RateLimitError('WebSocket upgrade rate-limited by gateway (HTTP 429)', {
        url: this.url,
        ...(rl.retryAfterMs !== undefined ? { retryAfterMs: rl.retryAfterMs } : {}),
        ...(rl.retryAfter !== undefined ? { retryAfter: rl.retryAfter } : {}),
      });
    }
    const ctx: Record<string, unknown> = {};
    if (closeCode !== undefined) ctx.closeCode = closeCode;
    if (closeReason !== undefined) ctx.closeReason = closeReason;
    return new ConnectionError(message, { url: this.url, context: ctx });
  }

  /**
   * Handle incoming message
   */
  private handleMessage(data: unknown): void {
    if (data instanceof ArrayBuffer) {
      this.handlers.onBinaryMessage?.(data);
      return;
    }

    if (typeof data === 'string') {
      try {
        const message = deserializeMessage(data);
        this.handlers.onMessage?.(message);
      } catch (err) {
        const error = err instanceof Error ? err : new Error(String(err));
        this.handlers.onError?.(new ConnectionError(`Failed to parse message: ${error.message}`, {
          cause: error,
        }));
      }
    }
  }

  /**
   * Send a message
   * @returns true if sent successfully, false if not connected
   */
  send(message: SDKOutgoingMessage): boolean {
    if (!this.isConnected()) {
      return false;
    }

    try {
      const data = serializeMessage(message);
      this.ws!.send(data);
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Send binary data
   * @returns true if sent successfully, false if not connected
   */
  sendBinary(data: ArrayBuffer | Uint8Array): boolean {
    if (!this.isConnected()) {
      return false;
    }

    try {
      this.ws!.send(data);
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Close the connection
   */
  async close(code = 1000, reason = 'Client closing'): Promise<void> {
    if (this.state === 'disconnected' || !this.ws) {
      return;
    }

    if (this.state === 'closing') {
      // Wait for existing close to complete
      return new Promise<void>((resolve) => {
        const checkClosed = setInterval(() => {
          if (this.state === 'disconnected') {
            clearInterval(checkClosed);
            resolve();
          }
        }, 50);
      });
    }

    this.state = 'closing';

    return new Promise<void>((resolve) => {
      const closeTimeout = setTimeout(() => {
        this.cleanup();
        resolve();
      }, 5000);

      const wsRef = this.ws!;
      const originalOnClose = wsRef.onclose;
      wsRef.onclose = (event) => {
        clearTimeout(closeTimeout);
        this.state = 'disconnected';
        if (originalOnClose) {
          originalOnClose.call(wsRef, event);
        }
        resolve();
      };

      try {
        this.ws!.close(code, reason);
      } catch {
        clearTimeout(closeTimeout);
        this.cleanup();
        resolve();
      }
    });
  }

  /**
   * Cleanup resources
   */
  private cleanup(): void {
    if (this.ws) {
      this.ws.onopen = null;
      this.ws.onclose = null;
      this.ws.onerror = null;
      this.ws.onmessage = null;

      if (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING) {
        try {
          this.ws.close();
        } catch {
          // Ignore close errors during cleanup
        }
      }

      this.ws = null;
    }

    this.state = 'disconnected';
    this.connectPromise = null;
    this.connectResolve = null;
    this.connectReject = null;
  }
}
