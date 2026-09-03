// Live P1 validation: bud.agent(...) + config_warning surfacing, against a
// SNAPSHOT gateway on a spare port (never the user's :3009) using the BUILT SDK
// (dist) and a local ollama LLM.
//
// Usage:
//   WAAV_GATEWAY_URL=ws://127.0.0.1:3085 OLLAMA_MODEL=deepseek-r1:1.5b \
//   node tests/live/live-agent.mjs
//
// Asserts:
//  (1) bud.agent({...}) reaches the gateway built-in conversation loop and the
//      session goes READY with protocol_version '1.0' (the full conversation_config
//      + nested turn_detection wire shape is accepted, not silently dropped).
//  (2) a gateway config_warning surfaces as a typed `configWarning` event:
//        - reasoning_model_on_voice_path (model is a reasoning model), and/or
//        - reasoning_tier_without_model (reasoning sub-knob without reasoning_model).
//  (3) a BOGUS config key surfaces as an `unknown_config_keys` config_warning
//      (the P0 advisory), proving misnested/typo keys degrade LOUDLY.
import { WebSocket } from 'ws';
import { BudClient } from '../../dist/index.mjs';
import { WebSocketSession, serializeMessage } from '../../dist/index.mjs';

const URL = process.env.WAAV_GATEWAY_URL ?? 'ws://127.0.0.1:3085';
const WS_URL = URL.endsWith('/ws') ? URL : `${URL.replace(/\/$/, '')}/ws`;
const HTTP = URL.replace(/^ws/, 'http').replace(/\/ws$/, '');
const OLLAMA = process.env.OLLAMA_URL ?? 'http://127.0.0.1:11434/v1';
const MODEL = process.env.OLLAMA_MODEL ?? 'deepseek-r1:1.5b';

let failures = 0;
function ok(name, cond, detail) {
  const status = cond ? 'PASS' : 'FAIL';
  if (!cond) failures++;
  console.log(`[${status}] ${name}${detail ? ' :: ' + detail : ''}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------------------
// (1)+(2) bud.agent() — full conversation_config + reasoning warning + ready.
// ---------------------------------------------------------------------------
{
  const bud = new BudClient({ baseUrl: HTTP, WebSocket });
  const agent = bud.agent({
    stt: { provider: 'deepgram', language: 'en-US', apiKey: 'placeholder-not-a-real-key' },
    tts: { provider: 'deepgram', voiceId: 'aura-asteria-en', apiKey: 'placeholder-not-a-real-key' },
    llm: {
      baseUrl: OLLAMA,
      model: MODEL, // a reasoning model on the spoken path → reasoning_model_on_voice_path
      reasoningRoute: 'always', // reasoning sub-knob without reasoning_model → reasoning_tier_without_model
      latencyFiller: 'auto',
      muteStrategy: 'until_first_bot_complete',
      bargeInMinWords: 3,
    },
    turn: { enabled: true, threshold: 0.6, eagerEot: true },
    reconnect: false,
  });

  const warnings = [];
  const errors = [];
  let ready = null;
  agent.on('configWarning', (w) => warnings.push(w));
  agent.on('error', (e) => errors.push(e));
  agent.on('ready', (r) => { ready = r; });

  try {
    const r = await agent.connect();
    ready = r;
    ok('bud.agent() connects + READY', !!r.streamId, `streamId=${r.streamId}`);
    ok('ready protocol_version is 1.0', r.protocolVersion === '1.0', `pv=${r.protocolVersion}`);
  } catch (err) {
    ok('bud.agent() connects + READY', false, String((err && err.message) || err));
  }

  // Give the gateway a beat to emit the config-time advisories.
  await sleep(1200);

  const codes = warnings.map((w) => w.code);
  console.log('    config_warning codes seen:', JSON.stringify(codes));
  ok(
    'reasoning config_warning surfaced (voice-path and/or tier-without-model)',
    codes.includes('reasoning_model_on_voice_path') || codes.includes('reasoning_tier_without_model'),
    codes.join(',') || 'none'
  );
  // A reasoning config_warning carries a human message + (sometimes) detail.
  const rw = warnings.find((w) => w.code === 'reasoning_model_on_voice_path' || w.code === 'reasoning_tier_without_model');
  if (rw) ok('reasoning config_warning has a human message', typeof rw.message === 'string' && rw.message.length > 0, rw.message.slice(0, 80));

  ok('no PROTOCOL_VERSION_MISMATCH', !errors.some((e) => e.code === 'PROTOCOL_VERSION_MISMATCH'), errors.map((e) => e.code).join(','));

  await agent.disconnect();
}

// ---------------------------------------------------------------------------
// (3) Bogus config key → unknown_config_keys config_warning (P0 advisory).
//     Use a raw session so we can inject a key the typed builder would not.
// ---------------------------------------------------------------------------
{
  const session = new WebSocketSession({ url: WS_URL, WebSocket, autoConfig: false, reconnect: false, connectionTimeout: 8000 });
  const warnings = [];
  let ready = null;
  session.on('configWarning', (w) => warnings.push(w));
  session.on('ready', (r) => { ready = r; });

  try {
    await session.connect();
    // audio:false config-only session + a bogus top-level key + a misnested key.
    const raw = JSON.stringify({
      type: 'config',
      audio: false,
      totally_unknown_key: 123,
      reasoning_effort: 'minimal', // misnested: belongs under conversation_config
    });
    session.send(JSON.parse(raw));
    await session.waitForReady(8000).catch(() => {});
    await sleep(900);
  } catch (err) {
    ok('raw config send', false, String((err && err.message) || err));
  }

  const warn = warnings.find((w) => w.code === 'unknown_config_keys');
  ok('bogus key → unknown_config_keys config_warning', !!warn, warnings.map((w) => w.code).join(',') || 'none');
  if (warn) {
    ok('unknown_config_keys message names the bad key', /totally_unknown_key/.test(warn.message), warn.message.slice(0, 120));
    const ignored = warn.detail && (warn.detail.ignored_keys || warn.detail.ignoredKeys);
    ok('unknown_config_keys detail.ignored_keys present', Array.isArray(ignored) && ignored.length > 0, JSON.stringify(warn.detail));
    ok('config_warning did NOT close the session (non-fatal)', session.isConnected(), `connected=${session.isConnected()}`);
  }

  await session.disconnect();
}

console.log(`\nRESULT: ${failures === 0 ? 'ALL PASS' : failures + ' FAILURE(S)'}`);
process.exit(failures === 0 ? 0 : 1);
