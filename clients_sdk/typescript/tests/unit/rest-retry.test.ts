/**
 * P1-7 parity fix: the TS REST client auto-retries transient 429/503 with
 * Retry-After-aware jittered backoff, exactly like the Python SDK — previously
 * Python rode out a rate-limit blip while TypeScript threw on the first 429.
 */
import { describe, it, expect, vi } from 'vitest';
import { RestClient } from '../../src/rest/client.js';
import { RateLimitError } from '../../src/errors/ratelimit.js';

const ok = () => new Response(JSON.stringify({ status: 'ok' }), {
  status: 200,
  headers: { 'content-type': 'application/json' },
});

describe('REST retry on transient saturation (Python parity)', () => {
  it('retries a 429 then succeeds', async () => {
    let calls = 0;
    const fetchFn = vi.fn(async () => {
      calls += 1;
      if (calls === 1) {
        return new Response('slow down', { status: 429, headers: { 'retry-after': '0' } });
      }
      return ok();
    }) as unknown as typeof fetch;
    const client = new RestClient({ baseUrl: 'http://gw', fetch: fetchFn });
    await expect(client.health()).resolves.toEqual({ status: 'ok' });
    expect(calls).toBe(2);
  });

  it('retries a 503 then succeeds', async () => {
    let calls = 0;
    const fetchFn = vi.fn(async () => {
      calls += 1;
      if (calls <= 2) {
        return new Response('at capacity', { status: 503, headers: { 'retry-after': '0' } });
      }
      return ok();
    }) as unknown as typeof fetch;
    const client = new RestClient({ baseUrl: 'http://gw', fetch: fetchFn });
    await expect(client.health()).resolves.toEqual({ status: 'ok' });
    expect(calls).toBe(3);
  });

  it('gives up after maxRetries and surfaces the typed RateLimitError', async () => {
    const fetchFn = vi.fn(async () =>
      new Response('slow down', { status: 429, headers: { 'retry-after': '0' } }),
    ) as unknown as typeof fetch;
    const client = new RestClient({ baseUrl: 'http://gw', fetch: fetchFn, retries: 2 });
    await expect(client.health()).rejects.toBeInstanceOf(RateLimitError);
    expect(fetchFn).toHaveBeenCalledTimes(3); // 1 attempt + 2 retries
  });

  it('does not retry non-transient errors', async () => {
    const fetchFn = vi.fn(async () => new Response('nope', { status: 400 })) as unknown as typeof fetch;
    const client = new RestClient({ baseUrl: 'http://gw', fetch: fetchFn });
    await expect(client.health()).rejects.toThrow();
    expect(fetchFn).toHaveBeenCalledTimes(1);
  });
});
