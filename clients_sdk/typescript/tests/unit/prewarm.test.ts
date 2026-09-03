/**
 * D6 prepareConnection()/prewarm() + D7 BudClient connect-gate wiring tests.
 *
 * prepareConnection warms DNS/TLS by firing a cheap HEAD (falling back to GET)
 * at the gateway origin, optionally prefetching a token off the critical path,
 * and never throwing. BudClient also constructs ONE shared connect gate it
 * injects into every pipeline/agent session unless disabled.
 */
import { describe, it, expect, vi } from 'vitest';
import { BudClient } from '../../src/bud.js';

function okFetch() {
  return vi.fn(async (_url: string | URL | Request, _init?: RequestInit) =>
    new Response('', { status: 200 }),
  ) as unknown as typeof fetch;
}

describe('D6 prepareConnection / prewarm', () => {
  it('fires a HEAD at the gateway origin and reports success', async () => {
    const fetchFn = okFetch();
    const bud = new BudClient({ baseUrl: 'http://gw.example', fetch: fetchFn });
    const warmed = await bud.prepareConnection();
    expect(warmed).toBe(true);
    const call = (fetchFn as unknown as { mock: { calls: unknown[][] } }).mock.calls[0]!;
    expect(call[0]).toBe('http://gw.example/');
    expect((call[1] as RequestInit).method).toBe('HEAD');
  });

  it('falls back to GET when HEAD rejects', async () => {
    const fetchFn = vi.fn(async (_url: string | URL | Request, init?: RequestInit) => {
      if (init?.method === 'HEAD') throw new Error('405');
      return new Response('', { status: 200 });
    }) as unknown as typeof fetch;
    const bud = new BudClient({ baseUrl: 'http://gw.example', fetch: fetchFn });
    const warmed = await bud.prepareConnection();
    expect(warmed).toBe(true);
    const calls = (fetchFn as unknown as { mock: { calls: unknown[][] } }).mock.calls;
    expect((calls[0]![1] as RequestInit).method).toBe('HEAD');
    expect((calls[1]![1] as RequestInit).method).toBe('GET');
  });

  it('never throws on a failed warmup (best-effort)', async () => {
    const fetchFn = vi.fn(async () => { throw new Error('down'); }) as unknown as typeof fetch;
    const bud = new BudClient({ baseUrl: 'http://gw.example', fetch: fetchFn });
    await expect(bud.prepareConnection()).resolves.toBe(false);
  });

  it('awaits an optional token prefetch (off the connect critical path)', async () => {
    const fetchFn = okFetch();
    const bud = new BudClient({ baseUrl: 'http://gw.example', fetch: fetchFn });
    const prefetchToken = vi.fn(async () => 'tok');
    await bud.prewarm({ prefetchToken });
    expect(prefetchToken).toHaveBeenCalledTimes(1);
  });

  it('swallows a token-prefetch failure', async () => {
    const fetchFn = okFetch();
    const bud = new BudClient({ baseUrl: 'http://gw.example', fetch: fetchFn });
    await expect(bud.prewarm({ prefetchToken: async () => { throw new Error('no token'); } })).resolves.toBe(true);
  });
});

describe('D7 BudClient connect-gate wiring', () => {
  it('injects the SAME shared connect gate into pipelines created from one client', () => {
    const bud = new BudClient({ baseUrl: 'http://gw.example' });
    const a = bud.stt.create({ provider: 'deepgram' });
    const b = bud.tts.create({ provider: 'deepgram' });
    // Reach the underlying session's config to confirm both got a gate, and the
    // SAME instance (one per client, since the gateway limit is per-IP).
    const gateA = (a as unknown as { session: { config: { connectGate?: unknown } } }).session.config.connectGate;
    const gateB = (b as unknown as { session: { config: { connectGate?: unknown } } }).session.config.connectGate;
    expect(gateA).toBeDefined();
    expect(gateB).toBe(gateA);
  });

  it('connectGate:false disables the gate', () => {
    const bud = new BudClient({ baseUrl: 'http://gw.example', connectGate: false });
    const a = bud.stt.create({ provider: 'deepgram' });
    const gateA = (a as unknown as { session: { config: { connectGate?: unknown } } }).session.config.connectGate;
    expect(gateA).toBeUndefined();
  });
});
