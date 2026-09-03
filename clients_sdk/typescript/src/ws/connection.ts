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
 * Duck-typed `ping()` present on the Node `ws` library's WebSocket (NOT on the
 * browser `WebSocket`, which cannot send control frames). When present, the SDK
 * drives a native WS ping/pong liveness watchdog (D1); browsers fall back to a
 * stale-inbound-data timer in the session.
 */
interface WsPingLike {
  ping(data?: unknown, mask?: boolean, cb?: (err?: Error) => void): void;
}

function hasPing(ws: unknown): ws is WsPingLike {
  return typeof (ws as { ping?: unknown })?.ping === 'function';
}

/**
 * Default WebSocket connect timeout in ms (D6). Sized to comfortably exceed the
 * gateway's ~30s PROVIDER_READY ceiling for an `audio=true` session
 * (config_handler.rs:45). See {@link WebSocketConnectionOptions.timeout}.
 */
export const DEFAULT_CONNECT_TIMEOUT_MS = 35000;

/**
 * WebSocket connection options
 */
export interface WebSocketConnectionOptions {
  /** WebSocket URL (e.g., "ws://localhost:3001/ws") */
  url: string;
  /**
   * Connection timeout in milliseconds (default: 35000). D6: the WS handshake
   * itself is fast (~2ms), but for an `audio=true` session the gateway holds the
   * connect open until PROVIDER_READY, which can take up to ~30s while it builds
   * the STT/TTS upstreams (config_handler.rs:45). The old 10s default tripped
   * that legitimate wait and failed a perfectly-healthy connect, so the default
   * is 35s. A genuinely-dead gateway now takes 35s to fail the FIRST connect;
   * post-connect death is covered far faster by the D1 liveness watchdog.
   */
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
  /**
   * Fired when a native WS `pong` control frame arrives (Node `ws` only — the
   * browser `WebSocket` does not surface pong). Backs the D1 liveness watchdog.
   */
  onPong?: () => void;
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
  /**
   * Captured back-off-worthy HTTP status from the `ws` upgrade: 429 (per-IP
   * throttle) or 503 (global "Server at capacity"). Both are retryable, not
   * fatal — the handshake failure is surfaced as a typed RateLimitError so the
   * connect-backoff loop retries instead of giving up. (Browsers can't see this.)
   */
  private rateLimit: { statusCode: number; retryAfterMs?: number; retryAfter?: string } | null = null;

  constructor(options: WebSocketConnectionOptions) {
    this.url = options.url;
    this.timeout = options.timeout ?? DEFAULT_CONNECT_TIMEOUT_MS;
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
            // D7: BOTH the per-IP 429 throttle and the global 503 "Server at
            // capacity" are retryable back-off signals, not fatal — capture
            // either (with any Retry-After) so the handshake rejects with a
            // typed RateLimitError the connect-backoff loop will retry.
            if (res?.statusCode === 429 || res?.statusCode === 503) {
              const headers = res.headers ?? {};
              const raw = headers['retry-after'] ?? headers['Retry-After'];
              const retryAfter = Array.isArray(raw) ? raw[0] : raw;
              this.rateLimit = { statusCode: res.statusCode };
              if (retryAfter !== undefined) {
                this.rateLimit.retryAfter = retryAfter;
                const ms = parseRetryAfterMs(retryAfter);
                if (ms !== undefined) this.rateLimit.retryAfterMs = ms;
              }
            }
          });
          // Plumb the native `pong` control frame up to the session so the D1
          // watchdog can confirm the socket is alive. The browser `WebSocket`
          // has no equivalent event, so this is Node (`ws`) only.
          this.ws.on('pong', () => {
            this.handlers.onPong?.();
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
      const desc =
        rl.statusCode === 503
          ? 'WebSocket upgrade rejected — gateway at capacity (HTTP 503)'
          : 'WebSocket upgrade rate-limited by gateway (HTTP 429)';
      return new RateLimitError(desc, {
        url: this.url,
        statusCode: rl.statusCode,
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
   * Bytes queued in the WebSocket's send buffer but not yet handed to the OS
   * (the `bufferedAmount`). A rising value means the socket can't drain as fast
   * as we're sending — the signal the D5 uplink shedder uses to apply
   * backpressure. Returns 0 when not connected.
   */
  bufferedAmount(): number {
    return this.ws?.bufferedAmount ?? 0;
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
   * Whether this connection can send native WS ping frames (Node `ws` only).
   * Browsers cannot, so the session uses a stale-inbound-data watchdog instead.
   */
  isPingCapable(): boolean {
    return hasPing(this.ws);
  }

  /**
   * Send a native WS ping control frame (Node `ws` only). The matching `pong`
   * is plumbed back via the `onPong` handler, letting the session run a
   * ping/pong liveness deadline (D1). No-op (returns false) in the browser or
   * when not connected.
   * @returns true if a ping was sent.
   */
  ping(): boolean {
    if (this.state !== 'connected' || !this.ws || !hasPing(this.ws)) {
      return false;
    }
    try {
      this.ws.ping();
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Forcibly tear down a (possibly half-open / zombie) socket WITHOUT a clean
   * close handshake. A graceful `close()` can hang on a frozen peer that never
   * sends FIN/RST, which is exactly the zombie case the D1 watchdog detects, so
   * the watchdog calls this to drop the dead socket immediately and let the
   * reconnect path build a fresh one. Uses Node `ws.terminate()` when available,
   * otherwise a best-effort `close()`. Fires `onClose` so the session reacts.
   */
  terminate(code = 4000, reason = 'liveness watchdog'): void {
    const ws = this.ws as (WebSocket & { terminate?: () => void }) | null;
    if (!ws) return;
    const wasConnected = this.state === 'connected' || this.state === 'connecting';
    this.cleanup();
    try {
      if (typeof ws.terminate === 'function') {
        ws.terminate();
      } else {
        ws.close(code, reason);
      }
    } catch {
      // Ignore — the socket is already dead.
    }
    if (wasConnected) {
      this.handlers.onClose?.(code, reason);
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
