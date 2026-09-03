/**
 * LIVE test (credential-free): the widget's REAL `/ws` serializer
 * (buildConfigMessage) against a snapshot gateway on a spare port.
 *
 * Asserts:
 *   1. The widget-built `config` envelope (audio:false, ollama-pointed
 *      conversation_config, STT, bare TTS voice) is ACCEPTED — `ready` arrives
 *      with protocol_version === "1.0" and NO error frame.
 *   2. A deliberately misnested key (turn_detection at the top level) triggers
 *      the gateway `config_warning`, which the widget surfacing path parses.
 *
 * Usage (from the widget dir, gateway already up on $GW_PORT):
 *   GW_PORT=3086 OLLAMA_MODEL=qwen2.5:7b-instruct node test/live_gateway.mjs
 */
import { JSDOM } from 'jsdom';

const GW_PORT = process.env.GW_PORT || '3086';
const OLLAMA = process.env.OLLAMA_BASE_URL || 'http://127.0.0.1:11434/v1';
const MODEL = process.env.OLLAMA_MODEL || 'qwen2.5:7b-instruct';
const WS_URL = `ws://127.0.0.1:${GW_PORT}/ws`;

// DOM globals so the bundle imports (it auto-registers the custom element).
// NOTE: deliberately do NOT install jsdom's Event/CustomEvent — Node's built-in
// WebSocket (undici) does `instanceof Event` against the NATIVE Event class, and
// shadowing it with jsdom's breaks the live socket. The serializer path used
// here never dispatches DOM events, so the native classes are sufficient.
const dom = new JSDOM('<!DOCTYPE html><body></body>', { url: 'http://localhost/' });
for (const k of ['window', 'document', 'HTMLElement', 'customElements']) {
  globalThis[k] = dom.window[k];
}
const { buildConfigMessage } = await import('../dist/bud-widget.esm.js');

function fail(msg) {
  console.error('LIVE FAIL:', msg);
  process.exit(1);
}

/** Open a /ws session, send `envelope`, collect frames until `ready` (+ a beat). */
function runSession(envelope, { waitMs = 2500 } = {}) {
  return new Promise((resolve, reject) => {
    const frames = [];
    let readyMsg = null;
    let errorMsg = null;
    const ws = new WebSocket(WS_URL);
    const done = () => {
      try {
        ws.close();
      } catch {}
      resolve({ frames, readyMsg, errorMsg });
    };
    const timer = setTimeout(done, waitMs);
    ws.addEventListener('open', () => ws.send(JSON.stringify(envelope)));
    ws.addEventListener('message', (ev) => {
      if (typeof ev.data !== 'string') return; // ignore binary audio
      let m;
      try {
        m = JSON.parse(ev.data);
      } catch {
        return;
      }
      frames.push(m);
      if (m.type === 'ready') readyMsg = m;
      if (m.type === 'error') errorMsg = m;
    });
    ws.addEventListener('error', (e) => {
      clearTimeout(timer);
      reject(new Error('WS error: ' + (e?.message || e)));
    });
    ws.addEventListener('close', () => {
      clearTimeout(timer);
    });
  });
}

(async () => {
  // --- 1) Happy path: widget-built envelope with an ollama conversation loop. ---
  const cfg = {
    gatewayUrl: WS_URL,
    stt: { provider: 'deepgram', language: 'en-US', sampleRate: 16000, channels: 1 },
    tts: { provider: 'deepgram', voiceId: 'aura-asteria-en' },
    conversation: {
      baseUrl: OLLAMA,
      model: MODEL,
      systemPrompt: 'You are a terse assistant.',
      reasoningEffort: 'low',
      latencyFiller: 'auto',
      eagerEot: true,
      bargeInMinWords: 2,
      allowInterruption: true,
    },
    features: { punctuation: true },
    audioFeatures: { turnDetection: { enabled: true, threshold: 0.5, eager: true } },
  };
  const envelope = buildConfigMessage(cfg);
  envelope.audio = false; // credential-free: no audio backend needed.

  console.log('SENT envelope (widget buildConfigMessage):');
  console.log(JSON.stringify(envelope, null, 2));

  const r1 = await runSession(envelope);
  if (!r1.readyMsg) fail('no ready frame received for the valid envelope');
  if (r1.readyMsg.protocol_version !== '1.0')
    fail(`protocol_version != "1.0" (got ${JSON.stringify(r1.readyMsg.protocol_version)})`);
  if (r1.errorMsg) fail('gateway returned an error frame for a valid config: ' + JSON.stringify(r1.errorMsg));
  // The valid envelope nests everything correctly => NO unknown-key warning.
  const warn1 = r1.frames.find((f) => f.type === 'config_warning');
  if (warn1) fail('valid envelope unexpectedly produced a config_warning: ' + JSON.stringify(warn1));
  console.log(
    `OK #1: ready stream_id=${r1.readyMsg.stream_id} protocol_version=${r1.readyMsg.protocol_version}; ` +
      `conversation_config ACCEPTED (no error, no config_warning).`
  );

  // --- 2) Misnested key => gateway config_warning the widget would surface. ---
  const bad = JSON.parse(JSON.stringify(envelope));
  delete bad.stt_config.turn_detection;
  bad.turn_detection = { enabled: true }; // WRONG nesting (belongs under stt_config)
  bad.stt_config.features = { diarizationn: true }; // typo'd feature key
  const r2 = await runSession(bad);
  if (!r2.readyMsg) fail('no ready frame for the misnested envelope (should still start)');
  const warn2 = r2.frames.find((f) => f.type === 'config_warning');
  if (!warn2) fail('misnested key did NOT produce a config_warning');
  if (warn2.code !== 'unknown_config_keys')
    fail(`unexpected config_warning code: ${warn2.code}`);
  if (!warn2.message || typeof warn2.message !== 'string')
    fail('config_warning missing human message');
  const ignored = warn2.detail?.ignored_keys || [];
  console.log(
    `OK #2: config_warning code=${warn2.code}; ignored_keys=${JSON.stringify(ignored)}`
  );
  console.log(`        message: ${warn2.message}`);
  // Sanity: the advisory names the misnested/typo'd paths.
  const namesTd = ignored.some((k) => k.includes('turn_detection')) || warn2.message.includes('turn_detection');
  const namesTypo = ignored.some((k) => k.includes('diarizationn')) || warn2.message.includes('diarizationn');
  if (!namesTd) fail('config_warning did not name the misnested turn_detection');
  if (!namesTypo) fail('config_warning did not name the typo diarizationn');

  console.log('\nLIVE PASS: widget envelope accepted live + config_warning surfaced.');
  process.exit(0);
})().catch((e) => fail(e?.stack || String(e)));
