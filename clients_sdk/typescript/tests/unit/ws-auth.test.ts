/**
 * Where the API key rides on the WebSocket upgrade. Under the Node `ws` library it is the
 * `Authorization` header (every gateway accepts it; no access log records it). A WebSocket that
 * can't send headers (browser, Node's built-in) falls back to `?token=`. The key never appears in
 * the connection's URL as reported in errors.
 */
import { describe, it, expect } from 'vitest';
import { authenticatedUpgrade, WebSocketConnection } from '../../src/ws/connection.js';

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

/** Shaped like a browser WebSocket: no `ping`/`terminate`, two constructor arguments. */
class BrowserWsLike {
  static OPEN = 1;
  static last: { url: string; args: number } | null = null;
  binaryType = 'blob';
  readyState = 0;
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  constructor(url: string, ...rest: unknown[]) {
    BrowserWsLike.last = { url, args: 1 + rest.length };
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

  it('falls back to ?token= where headers are impossible', () => {
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

  it('opens a browser socket with ?token= but reports the URL without it', async () => {
    const conn = new WebSocketConnection({
      url: 'ws://gw/ws',
      apiKey: 'sk-browser',
      timeout: 20,
      WebSocket: BrowserWsLike as unknown as typeof WebSocket,
    });
    const failure = conn.connect().catch((e: Error) => e);
    expect(BrowserWsLike.last).toEqual({ url: 'ws://gw/ws?token=sk-browser', args: 2 });
    const err = await failure;
    expect(String((err as Error).message)).not.toContain('sk-browser');
  });
});
