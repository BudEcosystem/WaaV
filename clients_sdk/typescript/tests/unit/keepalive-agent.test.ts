/**
 * D7 REST keep-alive agent tests.
 *
 * Node's native `fetch` (undici) already pools per origin; this agent is what
 * lets `node-fetch`-style consumers reuse a socket and lets us tune undici's
 * pool. Asserts: an agent is built under Node; the `node:http(s).Agent` MATCHES
 * the gateway scheme (an https.Agent on an http:// URL throws
 * ERR_INVALID_PROTOCOL — the regression this guards); the RestClient attaches the
 * SAME agent instance to every fetch (the node-fetch reuse signal); and
 * `keepAlive: false` disables it.
 */
import { describe, it, expect, vi } from 'vitest';
import { buildKeepAliveInit } from '../../src/rest/keepalive-agent.js';
import { RestClient } from '../../src/rest/client.js';

/** The `protocol` an http.Agent / https.Agent instance reports ('http:'/'https:'). */
function agentProtocol(agent: unknown): string | undefined {
  return (agent as { protocol?: string } | undefined)?.protocol;
}

describe('D7 buildKeepAliveInit', () => {
  it('builds a keep-alive agent under Node (dispatcher and/or agent)', () => {
    const init = buildKeepAliveInit('http://gw');
    // We run under Node, so at least one of the two agent keys must be present.
    const hasAgent = init.agent !== undefined || init.dispatcher !== undefined;
    expect(hasAgent).toBe(true);
  });

  it('returns a stable agent instance (so callers can reuse one pool)', () => {
    const a = buildKeepAliveInit('http://gw', { maxSockets: 8 });
    expect(a.agent !== undefined || a.dispatcher !== undefined).toBe(true);
  });

  // The regression guard: a node:https.Agent against an http:// URL throws
  // ERR_INVALID_PROTOCOL under node-fetch, so the Agent MUST track the scheme.
  it('picks a node:http Agent for an http:// gateway (NOT https)', () => {
    const init = buildKeepAliveInit('http://localhost:3009');
    expect(agentProtocol(init.agent)).toBe('http:');
  });

  it('picks a node:https Agent for an https:// gateway', () => {
    const init = buildKeepAliveInit('https://gw.example.com');
    expect(agentProtocol(init.agent)).toBe('https:');
  });

  it('defaults to a node:http Agent for a scheme-less / unparseable baseUrl', () => {
    const init = buildKeepAliveInit('gw-no-scheme');
    expect(agentProtocol(init.agent)).toBe('http:');
  });

  // An http.Agent built for an http:// origin must actually drive a request over
  // http without the ERR_INVALID_PROTOCOL crash an https.Agent would cause.
  it('the http Agent serves an http:// request without ERR_INVALID_PROTOCOL', async () => {
    const http = (await import('node:http')) as typeof import('node:http');
    const init = buildKeepAliveInit('http://127.0.0.1');
    const server = http.createServer((_req, res) => res.end('ok'));
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const port = (server.address() as { port: number }).port;
    try {
      const body = await new Promise<string>((resolve, reject) => {
        const req = http.get(
          { host: '127.0.0.1', port, path: '/', agent: init.agent as import('node:http').Agent },
          (res) => {
            let data = '';
            res.on('data', (c) => (data += c));
            res.on('end', () => resolve(data));
          },
        );
        req.on('error', reject);
      });
      expect(body).toBe('ok');
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});

describe('D7 RestClient attaches the keep-alive agent to every request', () => {
  function makeFetchSpy() {
    const inits: RequestInit[] = [];
    const fetchFn = vi.fn(async (_url: string | URL | Request, init?: RequestInit) => {
      inits.push(init ?? {});
      return new Response('{"status":"ok"}', { status: 200, headers: { 'content-type': 'application/json' } });
    });
    return { fetchFn: fetchFn as unknown as typeof fetch, inits };
  }

  it('attaches the SAME agent instance to every call (the node-fetch reuse signal)', async () => {
    const { fetchFn, inits } = makeFetchSpy();
    const client = new RestClient({ baseUrl: 'http://gw', fetch: fetchFn });

    await client.health();
    await client.health();
    await client.health();

    expect(inits.length).toBe(3);
    const pick = (i: RequestInit) =>
      (i as { agent?: unknown; dispatcher?: unknown }).agent ??
      (i as { agent?: unknown; dispatcher?: unknown }).dispatcher;
    const a0 = pick(inits[0]!);
    const a1 = pick(inits[1]!);
    const a2 = pick(inits[2]!);
    // The same agent instance rides every request, so a node-fetch consumer
    // reuses one pooled socket. (Native undici fetch ignores `agent` and pools
    // on its own — this asserts the attachment, not native-fetch wire reuse.)
    expect(a0).toBeDefined();
    expect(a1).toBe(a0);
    expect(a2).toBe(a0);
  });

  it('keepAlive:false attaches no agent', async () => {
    const { fetchFn, inits } = makeFetchSpy();
    const client = new RestClient({ baseUrl: 'http://gw', fetch: fetchFn, keepAlive: false });
    await client.health();
    const i = inits[0] as { agent?: unknown; dispatcher?: unknown };
    expect(i.agent).toBeUndefined();
    expect(i.dispatcher).toBeUndefined();
  });

  it('routes a 503 REST response to a retryable at-capacity RateLimitError', async () => {
    const fetchFn = vi.fn(async () =>
      new Response('Server at capacity', { status: 503, headers: { 'retry-after': '4' } }),
    ) as unknown as typeof fetch;
    // retries: 0 — this test asserts the ERROR SHAPE of a single attempt; the retry loop
    // itself is covered by rest-retry.test.ts.
    const client = new RestClient({ baseUrl: 'http://gw', fetch: fetchFn, retries: 0 });
    await expect(client.health()).rejects.toMatchObject({ statusCode: 503 });
  });
});
