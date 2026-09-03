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
 * (HTTP 429) OR signals it is at global capacity (HTTP 503 "Server at
 * capacity"). Both are back-off-and-retry conditions, not fatal failures, so
 * they share this typed error; `statusCode` tells them apart.
 */
export class RateLimitError extends BudError {
  /** HTTP status that triggered this (429 per-IP throttle, or 503 at-capacity). */
  readonly statusCode: number;
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
      /** HTTP status (default 429; pass 503 for the at-capacity case). */
      statusCode?: number;
      cause?: Error;
      context?: Record<string, unknown>;
    }
  ) {
    const statusCode = options?.statusCode ?? 429;
    super(message, BudErrorCode.API_RATE_LIMITED, {
      ...(options?.cause !== undefined ? { cause: options.cause } : {}),
      context: {
        ...options?.context,
        statusCode,
        retryAfterMs: options?.retryAfterMs,
        url: options?.url,
      },
    });
    this.name = 'RateLimitError';
    this.statusCode = statusCode;
    if (options?.retryAfterMs !== undefined) this.retryAfterMs = options.retryAfterMs;
    if (options?.retryAfter !== undefined) this.retryAfter = options.retryAfter;
    if (options?.url !== undefined) this.url = options.url;
  }

  /** True for both the 429 throttle and the 503 at-capacity back-off case. */
  isRateLimited(): boolean {
    return true;
  }

  /** Whether this is the gateway's global 503 "Server at capacity" signal. */
  isAtCapacity(): boolean {
    return this.statusCode === 503;
  }

  /**
   * Construct a RateLimitError from a fetch `Response` (status 429 per-IP
   * throttle, or 503 "Server at capacity"), extracting the `Retry-After` header.
   * The message + `statusCode` reflect the ACTUAL status so a 503 isn't
   * mislabelled as a 429.
   */
  static fromResponse(response: Response, options?: { url?: string }): RateLimitError {
    const retryAfter = response.headers.get('retry-after');
    const retryAfterMs = parseRetryAfterMs(retryAfter);
    const status = response.status;
    const what =
      status === 503
        ? 'Gateway at capacity (HTTP 503)'
        : 'Request rate-limited by gateway (HTTP 429)';
    return new RateLimitError(
      `${what}${retryAfterMs !== undefined ? `; retry after ${retryAfterMs}ms` : ''}`,
      {
        statusCode: status === 503 ? 503 : 429,
        ...(retryAfterMs !== undefined ? { retryAfterMs } : {}),
        ...(retryAfter !== null ? { retryAfter } : {}),
        ...(options?.url !== undefined ? { url: options.url } : {}),
      }
    );
  }
}
