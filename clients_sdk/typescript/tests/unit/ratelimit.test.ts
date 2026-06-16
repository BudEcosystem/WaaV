/**
 * RateLimitError + Retry-After parsing (P0 resilience).
 */
import { describe, it, expect } from 'vitest';
import { RateLimitError, parseRetryAfterMs, BudErrorCode } from '../../src/errors/index.js';

describe('parseRetryAfterMs', () => {
  it('parses delta-seconds', () => {
    expect(parseRetryAfterMs('5')).toBe(5000);
    expect(parseRetryAfterMs('0')).toBe(0);
  });

  it('returns undefined for missing / blank', () => {
    expect(parseRetryAfterMs(null)).toBeUndefined();
    expect(parseRetryAfterMs(undefined)).toBeUndefined();
    expect(parseRetryAfterMs('')).toBeUndefined();
    expect(parseRetryAfterMs('not-a-number')).toBeUndefined();
  });

  it('parses an HTTP-date into a non-negative delay', () => {
    const future = new Date(Date.now() + 10_000).toUTCString();
    const ms = parseRetryAfterMs(future);
    expect(ms).toBeGreaterThan(0);
    expect(ms).toBeLessThanOrEqual(11_000);
  });
});

describe('RateLimitError', () => {
  it('is a 429 with the rate-limited error code', () => {
    const err = new RateLimitError('throttled', { retryAfterMs: 2000, retryAfter: '2' });
    expect(err.statusCode).toBe(429);
    expect(err.code).toBe(BudErrorCode.API_RATE_LIMITED);
    expect(err.isRateLimited()).toBe(true);
    expect(err.retryAfterMs).toBe(2000);
    expect(err.retryAfter).toBe('2');
    expect(err).toBeInstanceOf(Error);
  });

  it('builds from a Response carrying Retry-After', () => {
    const res = new Response('rate limited', {
      status: 429,
      headers: { 'Retry-After': '7' },
    });
    const err = RateLimitError.fromResponse(res, { url: 'http://x/ws' });
    expect(err.statusCode).toBe(429);
    expect(err.retryAfterMs).toBe(7000);
    expect(err.url).toBe('http://x/ws');
  });
});
