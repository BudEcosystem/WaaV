/**
 * Rate-limit error class
 *
 * The gateway enforces a per-IP token bucket (default 60 rps / burst 10, keyed
 * by peer IP — this applies to WebSocket upgrades too). When a request or WS
 * upgrade is throttled the gateway responds with HTTP 429 and (optionally) a
 * `Retry-After` header. This error type surfaces that as a first-class,
 * inspectable condition with a parsed backoff delay so callers (and the SDK's
 * own WS-connect backoff) can retry intelligently instead of treating it as an
 * opaque connection failure.
 */

import { BudError, BudErrorCode } from './base.js';

/**
 * Parse a `Retry-After` header value into milliseconds.
 *
 * Per RFC 7231 the value is either a non-negative integer number of seconds, or
 * an HTTP-date. Returns `undefined` when absent or unparseable.
 */
export function parseRetryAfterMs(value: string | null | undefined): number | undefined {
  if (value === null || value === undefined) return undefined;
  const trimmed = value.trim();
  if (trimmed === '') return undefined;

  // delta-seconds form
  if (/^\d+$/.test(trimmed)) {
    return parseInt(trimmed, 10) * 1000;
  }

  // HTTP-date form
  const dateMs = Date.parse(trimmed);
  if (!Number.isNaN(dateMs)) {
    const delta = dateMs - Date.now();
    return delta > 0 ? delta : 0;
  }

  return undefined;
}

/**
 * Error thrown when the gateway rate-limits a request or WebSocket upgrade
 * (HTTP 429).
 */
export class RateLimitError extends BudError {
  /** HTTP status code (always 429) */
  readonly statusCode = 429;
  /**
   * Suggested delay before retrying, in milliseconds, parsed from the
   * `Retry-After` response header when present.
   */
  readonly retryAfterMs?: number;
  /** Raw `Retry-After` header value, if any */
  readonly retryAfter?: string;
  /** Request/connection URL that was throttled */
  readonly url?: string;

  constructor(
    message: string,
    options?: {
      retryAfterMs?: number;
      retryAfter?: string;
      url?: string;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    super(message, BudErrorCode.API_RATE_LIMITED, {
      ...(options?.cause !== undefined ? { cause: options.cause } : {}),
      context: {
        ...options?.context,
        statusCode: 429,
        retryAfterMs: options?.retryAfterMs,
        url: options?.url,
      },
    });
    this.name = 'RateLimitError';
    if (options?.retryAfterMs !== undefined) this.retryAfterMs = options.retryAfterMs;
    if (options?.retryAfter !== undefined) this.retryAfter = options.retryAfter;
    if (options?.url !== undefined) this.url = options.url;
  }

  /** Always true — this error type is exactly the 429 case. */
  isRateLimited(): boolean {
    return true;
  }

  /**
   * Construct a RateLimitError from a fetch `Response` (status 429),
   * extracting the `Retry-After` header.
   */
  static fromResponse(response: Response, options?: { url?: string }): RateLimitError {
    const retryAfter = response.headers.get('retry-after');
    const retryAfterMs = parseRetryAfterMs(retryAfter);
    return new RateLimitError(
      `Request rate-limited by gateway (HTTP 429)${retryAfterMs !== undefined ? `; retry after ${retryAfterMs}ms` : ''}`,
      {
        ...(retryAfterMs !== undefined ? { retryAfterMs } : {}),
        ...(retryAfter !== null ? { retryAfter } : {}),
        ...(options?.url !== undefined ? { url: options.url } : {}),
      }
    );
  }
}
