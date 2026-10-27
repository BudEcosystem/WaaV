/**
 * Where the API key rides on the WebSocket upgrade. Under the Node `ws` library it is the
 * `Authorization` header (every gateway accepts it; no access log records it). A WebSocket that
 * can't send headers (browser, Node's built-in) offers it as the `openai-insecure-api-key.<key>`
 * subprotocol beside `realtime`; only a key no subprotocol can carry falls back to `?token=`. The
 * key never appears in the connection's URL as reported in errors.
 */
import { describe, it, expect } from 'vitest';
import { authenticatedUpgrade, keySubprotocols, WebSocketConnection } from '../../src/ws/connection.js';
import { GatewayRealtime } from '../../src/pipelines/realtime-gateway.js';

/** Shaped like the `ws` library's class: `ping` and `terminate` on the prototype. */
class NodeWsLike {
  static OPEN = 1;
  static last: { url: string; protocols: unknown; options: unknown } | null = null;
  binaryType = 'arraybuffer';
  readyState = 0;
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  constructor(url: string, protocols?: unknown, options?: unknown) {
    NodeWsLike.last = { url, protocols, options };
  }
  ping(): void {}
  terminate(): void {}
  close(): void {}
}

/** RFC 7230 token characters, which is what a browser checks each offered subprotocol against. */
const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

/**
 * Shaped like a browser WebSocket: no `ping`/`terminate`, and a constructor that throws the way
 * a browser's does on a subprotocol that is not a token (an options object stringifies to one).
 */
class BrowserWsLike {
  static OPEN = 1;
  static last: { url: string; args: number; protocols?: unknown } | null = null;
  binaryType = 'blob';
  readyState = 0;
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  constructor(url: string, ...rest: unknown[]) {
    const protocols = rest[0];
    const offered =
      protocols === undefined ? [] : Array.isArray(protocols) ? protocols : [String(protocols)];
    if (offered.some((p) => !TOKEN.test(String(p)))) {
      throw new SyntaxError('The subprotocol is invalid.');
    }
    BrowserWsLike.last = { url, args: 1 + rest.length, protocols };
  }
  close(): void {}
}

describe('WebSocket upgrade authentication', () => {
  it('sends the key as a header under the Node ws library', () => {
    expect(authenticatedUpgrade('ws://gw/ws', 'sk-1', NodeWsLike)).toEqual({
      url: 'ws://gw/ws',
      headers: { Authorization: 'Bearer sk-1' },
    });
  });

  it('offers the key as a subprotocol where headers are impossible', () => {
    expect(authenticatedUpgrade('ws://gw/ws?x=1', 'sk-1.a_b', BrowserWsLike)).toEqual({
      url: 'ws://gw/ws?x=1',
      protocols: ['realtime', 'openai-insecure-api-key.sk-1.a_b'],
    });
  });

  it("keeps the caller's subprotocols and offers realtime once", () => {
    expect(keySubprotocols('k', 'realtime')).toEqual(['realtime', 'openai-insecure-api-key.k']);
    expect(keySubprotocols('k', ['x-app'])).toEqual(['x-app', 'realtime', 'openai-insecure-api-key.k']);
  });

  it('falls back to ?token= only for a key no subprotocol can carry', () => {
    expect(keySubprotocols('sk 1')).toBeUndefined();
    expect(keySubprotocols('a/b=')).toBeUndefined();
    expect(authenticatedUpgrade('ws://gw/ws?x=1', 'sk 1', BrowserWsLike)).toEqual({
      url: 'ws://gw/ws?x=1&token=sk+1',
    });
  });

  it('adds nothing without a key', () => {
    expect(authenticatedUpgrade('ws://gw/ws', undefined, NodeWsLike)).toEqual({ url: 'ws://gw/ws' });
  });

  it('opens the socket with the header and keeps the key out of the URL', () => {
    const conn = new WebSocketConnection({
      url: 'ws://gw/ws',
      apiKey: 'sk-node',
      timeout: 50,
      WebSocket: NodeWsLike as unknown as typeof WebSocket,
    });
    void conn.connect().catch(() => {});
    expect(NodeWsLike.last?.url).toBe('ws://gw/ws');
    expect(NodeWsLike.last?.options).toEqual({ headers: { Authorization: 'Bearer sk-node' } });
    conn.terminate();
  });

  it('opens a browser socket with the key subprotocol, its URL clean', async () => {
    const conn = new WebSocketConnection({
      url: 'ws://gw/ws',
      apiKey: 'sk-browser',
      timeout: 20,
      WebSocket: BrowserWsLike as unknown as typeof WebSocket,
    });
    const failure = conn.connect().catch((e: Error) => e);
    expect(BrowserWsLike.last).toEqual({
      url: 'ws://gw/ws',
      args: 2,
      protocols: ['realtime', 'openai-insecure-api-key.sk-browser'],
    });
    const err = await failure;
    expect(String((err as Error).message)).not.toContain('sk-browser');
  });

  it('the realtime pipeline uses the key subprotocol in a browser too', async () => {
    BrowserWsLike.last = null;
    const session = new GatewayRealtime({
      url: 'ws://gw/realtime',
      apiKey: 'sk-rt',
      config: { provider: 'openai' },
      WebSocket: BrowserWsLike as unknown as typeof WebSocket,
    });
    void session.connect().catch(() => {});
    expect(BrowserWsLike.last).toEqual({
      url: 'ws://gw/realtime',
      args: 2,
      protocols: ['realtime', 'openai-insecure-api-key.sk-rt'],
    });
    await session.disconnect();
  });
});
