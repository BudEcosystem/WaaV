/**
 * Connection-related error classes
 */

import { BudError, BudErrorCode } from './base.js';

/**
 * Error thrown when connection fails
 */
export class ConnectionError extends BudError {
  /** URL that failed to connect */
  readonly url?: string;

  constructor(
    message: string,
    options?: {
      url?: string;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, BudErrorCode.CONNECTION_FAILED, options);
    this.name = 'ConnectionError';
    this.url = options?.url;
  }
}

/**
 * Error thrown when connection times out
 */
export class TimeoutError extends BudError {
  /** Timeout duration in milliseconds */
  readonly timeoutMs: number;
  /** Operation that timed out */
  readonly operation?: string;

  constructor(
    message: string,
    timeoutMs: number,
    options?: {
      operation?: string;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, BudErrorCode.CONNECTION_TIMEOUT, {
      ...options,
      context: {
        ...options?.context,
        timeoutMs,
        operation: options?.operation,
      },
    });
    this.name = 'TimeoutError';
    this.timeoutMs = timeoutMs;
    this.operation = options?.operation;
  }
}

/**
 * Error thrown when reconnection fails
 */
export class ReconnectError extends BudError {
  /** Number of reconnection attempts made */
  readonly attempts: number;
  /** Maximum attempts allowed */
  readonly maxAttempts: number;
  /** Last error that caused reconnection failure */
  readonly lastError?: Error;

  constructor(
    message: string,
    attempts: number,
    maxAttempts: number,
    options?: {
      lastError?: Error;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    const code =
      attempts >= maxAttempts
        ? BudErrorCode.RECONNECT_MAX_ATTEMPTS
        : BudErrorCode.RECONNECT_FAILED;

    super(message, code, {
      ...options,
      context: {
        ...options?.context,
        attempts,
        maxAttempts,
      },
    });
    this.name = 'ReconnectError';
    this.attempts = attempts;
    this.maxAttempts = maxAttempts;
    this.lastError = options?.lastError;
  }

  /**
   * Check if max attempts were reached
   */
  isMaxAttemptsReached(): boolean {
    return this.attempts >= this.maxAttempts;
  }
}

/**
 * Error thrown when connection is unexpectedly closed
 */
export class ConnectionClosedError extends BudError {
  /** WebSocket close code */
  readonly closeCode?: number;
  /** WebSocket close reason */
  readonly closeReason?: string;
  /** Whether close was clean */
  readonly wasClean?: boolean;

  constructor(
    message: string,
    options?: {
      closeCode?: number;
      closeReason?: string;
      wasClean?: boolean;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, BudErrorCode.CONNECTION_CLOSED, {
      ...options,
      context: {
        ...options?.context,
        closeCode: options?.closeCode,
        closeReason: options?.closeReason,
        wasClean: options?.wasClean,
      },
    });
    this.name = 'ConnectionClosedError';
    this.closeCode = options?.closeCode;
    this.closeReason = options?.closeReason;
    this.wasClean = options?.wasClean;
  }

  /**
   * Check if closure was intentional/clean
   */
  isCleanClose(): boolean {
    return this.wasClean === true;
  }
}
