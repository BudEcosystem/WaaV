/**
 * Live (credential-free) integration test for the gateway-native bud.realtime.
 *
 * Connects `bud.realtime` to a FRESH gateway's provider-agnostic `/realtime`
 * endpoint and asserts it REACHES the realtime handler — proven by a clean
 * "API key not configured" (`missing_api_key`) error. That single fact proves the
 * SDK speaks the right wire end-to-end: the gateway parsed the `{type:"config"}`
 * message (no `parse_error`), recognized the provider (no `invalid_provider`),
 * selected it, and tried to connect — failing only at the credential step. A real
 * S2S round-trip needs a vendor key (out of scope).
 *
 * Gated on WAAV_GATEWAY_URL (only runs when pointed at a live gateway), e.g.:
 *   WAAV_GATEWAY_URL=ws://127.0.0.1:3022 \
 *     npx vitest run --config vitest.integration.config.ts tests/integration/realtime-gateway.test.ts
 */
import { describe, it, expect } from 'vitest';
import { WebSocket as NodeWebSocket } from 'ws';
import { BudClient } from '../../src/bud.js';
import { REALTIME_PROVIDERS, type GatewayRealtimeProvider } from '../../src/pipelines/realtime-gateway.js';

const RAW_URL = process.env.WAAV_GATEWAY_URL;
const describeLive = RAW_URL ? describe : describe.skip;

const BASE_URL = (RAW_URL ?? 'ws://127.0.0.1:3022')
  .replace(/^http:\/\//, 'ws://')
  .replace(/^https:\/\//, 'wss://')
  .replace(/\/(ws|realtime)$/, '');

/**
 * Connect bud.realtime and COLLECT every event for a short window, then report
 * the coded gateway error (e.g. `missing_api_key`) or `session_created` if one
 * arrived. Collecting (rather than racing on the first event) is deterministic:
 * the gateway sends its JSON `error` frame and then closes the socket, and on
 * Node `ws` the close surfaces as an extra codeless `'error'` — so the coded
 * JSON error and the transport error race within a tick. We just look across all
 * of them for the coded one.
 */
async function probe(
  provider: GatewayRealtimeProvider,
): Promise<{ code?: string; message?: string; sessionId?: string; bare?: string }> {
  const bud = new BudClient({ baseUrl: BASE_URL, WebSocket: NodeWebSocket as unknown as typeof WebSocket });
  const session = bud.realtime({ provider, voice: 'x', instructions: 'hi' });
  const errors: Array<{ code?: string; message: string }> = [];
  let sessionId: string | undefined;
  session.on('error', (e) => errors.push({ code: e.code, message: e.message }));
  session.on('sessionCreated', (e) => {
    sessionId = e.sessionId;
  });
  await session.connect().catch(() => {
    /* connection-level reject is fine; we read events below */
  });
  // Let the gateway emit its config-time error/session frame.
  await new Promise((r) => setTimeout(r, 1500));
  await session.disconnect();

  const coded = errors.find((e) => e.code);
  if (sessionId) return { sessionId };
  if (coded) return { code: coded.code, message: coded.message };
  return { bare: errors[0]?.message ?? '(no event)' };
}

describeLive('LIVE bud.realtime reaches the gateway /realtime handler (credential-free)', () => {
  it('openai: clean missing_api_key (proves the provider-agnostic wire is right)', async () => {
    const r = await probe('openai');
    expect(r.code).toBe('missing_api_key');
    expect(r.message).toContain('API key not configured');
    // NOT a protocol error.
    expect(r.code).not.toBe('parse_error');
    expect(r.code).not.toBe('invalid_provider');
  });

  it('works for a spread of providers (provider is just a field)', async () => {
    // A representative spread across the fleet (openai-family clone, deepgram,
    // elevenlabs, gemini). Paced to stay under the gateway's per-IP WS-upgrade
    // rate limit — a 429 on the upgrade is a load-shedding artifact, not a
    // protocol failure, so we retry a probe that 429s after a short backoff.
    const providers: GatewayRealtimeProvider[] = ['deepgram', 'elevenlabs', 'gemini', 'azure'];
    for (const p of providers) {
      let r = await probe(p);
      // Retry once if we got rate-limited (bare 429) rather than a coded error.
      if (!r.code && (r.bare ?? '').includes('429')) {
        await new Promise((res) => setTimeout(res, 2000));
        r = await probe(p);
      }
      expect(r.code, `provider=${p} code=${r.code} msg=${r.message} bare=${r.bare}`).toBe('missing_api_key');
      expect(r.message).toContain(p);
      // Pace the next probe to respect the per-IP limit.
      await new Promise((res) => setTimeout(res, 1200));
    }
  });

  it('exposes all 12 providers', () => {
    expect(REALTIME_PROVIDERS).toHaveLength(12);
  });
});
