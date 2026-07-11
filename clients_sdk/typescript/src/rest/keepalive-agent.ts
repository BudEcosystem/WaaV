/**
 * REST connection pooling (D7) — Node keep-alive agent.
 *
 * In the browser, `fetch` already pools + keep-alives connections for you, and so
 * does Node's built-in `fetch` (undici) — it keeps a per-origin connection pool by
 * DEFAULT. So this module is NOT what makes native-fetch calls reuse a socket;
 * undici does that on its own. What attaching an agent adds, per fetch flavor:
 *  - Node's built-in `fetch` (undici): honors an `undici.Agent` via `dispatcher`.
 *    Supplying one only lets us TUNE the pool (socket count / idle TTL). undici is
 *    not exposed as a `node:` builtin, so it resolves only when a `require` is
 *    available; absent that, native fetch still pools with its own defaults.
 *  - `node-fetch`-style fetch: does NOT pool on its own and honors a
 *    `node:http(s).Agent` via `agent` — for those consumers the agent is what
 *    enables reuse. It MUST match the gateway scheme: a `node:https.Agent` used
 *    against an `http://` URL throws ERR_INVALID_PROTOCOL, so we pick http vs
 *    https off the baseUrl (http://localhost:3001 is the common self-hosted case).
 * Native fetch ignores the unused `agent` key, so attaching both is safe.
 * Everything is created LAZILY and guarded so the browser bundle never touches
 * `node:http(s)`/`undici` (rollup externalizes them).
 */

/** Extra per-request init keys carrying the keep-alive agent (Node only). */
export interface KeepAliveInit {
  /** undici dispatcher (Node built-in fetch). */
  dispatcher?: unknown;
  /** node:http(s) Agent (node-fetch-style fetch). */
  agent?: unknown;
}

/** Tunables for the keep-alive agent. */
export interface KeepAliveAgentOptions {
  /** Max sockets per origin (default 64). */
  maxSockets?: number;
  /** Idle keep-alive socket TTL in ms (default 30000). */
  keepAliveMsecs?: number;
}

/** True when running under Node (has `process.versions.node`), not a browser. */
function isNode(): boolean {
  const proc = (globalThis as { process?: { versions?: { node?: string } } }).process;
  return !!proc?.versions?.node;
}

/**
 * Synchronously load a Node builtin (`node:https`, `node:module`, …) WITHOUT a
 * static import (so the browser bundle never references it). Tries, in order:
 *   1. `process.getBuiltinModule` (Node 20.16+/22.3+) — the cleanest sync route;
 *   2. a global `require` (the CJS build);
 *   3. `createRequire(import.meta.url)` obtained via (1)/(2) — the ESM build.
 * Returns null when none works (or in the browser).
 */
function loadNodeModule<T>(id: string): T | null {
  const proc = (globalThis as { process?: { getBuiltinModule?: (id: string) => unknown } }).process;
  // 1. process.getBuiltinModule — synchronous, browser-absent.
  if (typeof proc?.getBuiltinModule === 'function') {
    try {
      const mod = proc.getBuiltinModule(id);
      if (mod) return mod as T;
    } catch {
      // fall through
    }
  }
  // 2/3. require / createRequire for non-builtins (e.g. undici) or older Node.
  const req = resolveRequire();
  if (req) {
    try {
      return req(id) as T;
    } catch {
      // fall through
    }
  }
  return null;
}

/**
 * Build the per-request init additions that enable HTTP keep-alive under Node.
 * Returns `{}` in the browser (or if neither agent type can be constructed), so
 * the caller can spread it unconditionally.
 *
 * `baseUrl` selects the Agent module by scheme (`http://` → `node:http`,
 * `https://` → `node:https`): a node:https.Agent used against an http:// URL
 * throws ERR_INVALID_PROTOCOL under node-fetch-style fetch, so the scheme MUST
 * drive the choice. The agents are created via a runtime `require` resolved off a
 * non-literal specifier so the bundler does NOT inline `node:http(s)`/`undici`
 * into the browser build (they're in rollup's `external` list, but this also
 * dodges static analysis when the SDK is consumed by another bundler).
 */
export function buildKeepAliveInit(baseUrl: string, options: KeepAliveAgentOptions = {}): KeepAliveInit {
  if (!isNode()) return {};

  const maxSockets = options.maxSockets ?? 64;
  const keepAliveMsecs = options.keepAliveMsecs ?? 30000;
  const init: KeepAliveInit = {};

  // node:http(s).Agent — for node-fetch-style fetch (attached as `agent`); native
  // undici fetch ignores this key. The module MUST match the gateway scheme: a
  // node:https.Agent against an http:// URL throws ERR_INVALID_PROTOCOL, so pick
  // http vs https off the baseUrl (default http when unparseable / scheme-less).
  // Both are builtins, so process.getBuiltinModule loads them even in pure ESM.
  try {
    const useHttps = /^https:/i.test(baseUrl.trim());
    const mod = loadNodeModule<{ Agent: new (o: unknown) => unknown }>(
      useHttps ? 'node:https' : 'node:http',
    );
    if (mod) init.agent = new mod.Agent({ keepAlive: true, maxSockets, keepAliveMsecs });
  } catch {
    // ignore — fall through; dispatcher may still cover native fetch.
  }

  // undici.Agent — for Node's built-in fetch (attached as `dispatcher`). undici
  // is bundled with Node but not exposed as a `node:` builtin, so this only
  // resolves when a require/createRequire is available (CJS, or ESM w/ a path).
  try {
    const undici = loadNodeModule<{ Agent: new (o: unknown) => unknown }>('undici');
    if (undici) {
      init.dispatcher = new undici.Agent({
        keepAliveTimeout: keepAliveMsecs,
        connections: maxSockets,
      });
    }
  } catch {
    // ignore — undici not resolvable; the http(s).Agent above (if set) covers
    // node-fetch consumers, and native fetch simply pools per its own defaults.
  }

  return init;
}

/** Best-effort CommonJS `require`, working from both ESM and CJS builds. */
function resolveRequire(): ((id: string) => unknown) | null {
  // CJS: a global require exists.
  const g = globalThis as { require?: (id: string) => unknown };
  if (typeof g.require === 'function') return g.require;
  // ESM: synthesize one via module.createRequire(import.meta.url). Bootstrap
  // `node:module` via process.getBuiltinModule (sync, no static import).
  try {
    const url = (import.meta as unknown as { url?: string }).url;
    const proc = (globalThis as { process?: { getBuiltinModule?: (id: string) => unknown } }).process;
    const nodeModule = proc?.getBuiltinModule?.('node:module') as
      | { createRequire?: (u: string) => (id: string) => unknown }
      | undefined;
    if (nodeModule?.createRequire && url) {
      return nodeModule.createRequire(url);
    }
  } catch {
    // ignore
  }
  return null;
}
