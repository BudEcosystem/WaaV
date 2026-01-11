/**
 * WebSocket Connection Handler
 * Low-level WebSocket connection management
 */

import { ConnectionError, TimeoutError } from '../errors/index.js';
import type { IncomingMessage, OutgoingMessage } from '../types/messages.js';
import { serializeMessage, deserializeMessage } from './messages.js';

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
        this.ws = new this.WebSocketImpl(this.url, this.protocols);
        this.ws.binaryType = 'arraybuffer';

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
            safeReject(new ConnectionError(`Connection closed during handshake: ${event.reason || 'Unknown reason'}`, {
              url: this.url,
              code: event.code,
            }));
          }
        };

        this.ws.onerror = () => {
          const error = new ConnectionError('WebSocket error occurred', {
            url: this.url,
          });
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
  send(message: OutgoingMessage): boolean {
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

      const originalOnClose = this.ws!.onclose;
      this.ws!.onclose = (event) => {
        clearTimeout(closeTimeout);
        this.state = 'disconnected';
        if (originalOnClose) {
          originalOnClose.call(this.ws, event);
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
