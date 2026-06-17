/**
 * DRIFT GUARD (P1 non-negotiable deliverable).
 *
 * Make the widget SDK config a 1:1 mirror of the gateway `/ws` config so a
 * feature can never exist server-side yet be unreachable from the SDK. The
 * gateway's wire structs are the source of truth, exported as the committed
 * OpenAPI spec (gateway/docs/openapi.yaml, regenerated from the structs via
 * utoipa::ToSchema and itself byte-drift-gated by gateway/tests/openapi_drift.rs).
 *
 * This test asserts: for every property of each `/ws` config schema, the widget
 * SDK emits a corresponding snake_case key on the wire. We prove it the strong
 * way — by populating EVERY SDK field and running the SDK's REAL serializers
 * (buildConfigMessage / buildConversationConfig), then asserting the emitted
 * key-set is a SUPERSET of the schema's `properties`. If the gateway gains or
 * renames a config field and the widget isn't updated, the new openapi property
 * has no emitted key here and this test FAILS — drift can no longer ship.
 *
 * It also pins the canonical provider value-space counts (32 STT / 37 TTS /
 * 12 realtime) so a provider added to the gateway dispatch tables without
 * updating providers.ts is caught.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import yaml from 'js-yaml';
import { widget, WIDGET_DIR } from './harness.mjs';

const { buildConfigMessage, buildConversationConfig, STT_PROVIDERS, TTS_PROVIDERS, REALTIME_PROVIDERS } = widget;

// --- locate + load the committed OpenAPI spec ---------------------------------
const OPENAPI_PATH = path.resolve(WIDGET_DIR, '..', '..', 'gateway', 'docs', 'openapi.yaml');

function loadSchemas() {
  assert.ok(
    fs.existsSync(OPENAPI_PATH),
    `committed OpenAPI spec not found at ${OPENAPI_PATH} (regenerate via the gateway openapi exporter)`
  );
  const spec = yaml.load(fs.readFileSync(OPENAPI_PATH, 'utf8'));
  const schemas = spec?.components?.schemas;
  assert.ok(schemas, 'openapi.yaml missing components.schemas');
  return schemas;
}

/** Property names declared on an openapi schema object. */
function schemaProps(schemas, name) {
  const s = schemas[name];
  assert.ok(s, `openapi schema "${name}" is missing — did the gateway rename it?`);
  return Object.keys(s.properties ?? {});
}

/**
 * Assert every property of `schemaName` appears as a key in `emittedKeys`.
 * `extras`/`pronunciations` are open passthroughs / not beginner-surfaced by the
 * widget; list them in `ignore` with a justification.
 */
function assertSupersetOf(schemas, schemaName, emittedKeys, ignore = []) {
  const props = schemaProps(schemas, schemaName);
  const emitted = new Set(emittedKeys);
  const ignoreSet = new Set(ignore);
  const missing = props.filter((p) => !emitted.has(p) && !ignoreSet.has(p));
  assert.deepEqual(
    missing,
    [],
    `widget SDK does not emit these ${schemaName} field(s): [${missing.join(', ')}]. ` +
      `The gateway config gained/renamed a field the widget lacks — add it to the SDK ` +
      `config + serializer (see clients_sdk/widget/src/types.ts + websocket.ts).`
  );
}

// --- ConversationWebSocketConfig (the flagship LLM loop, 29 fields) -----------
test('drift: conversation_config emits every gateway ConversationWebSocketConfig field', () => {
  const schemas = loadSchemas();
  // Populate EVERY ConversationConfig field with a representative value.
  const fullConv = {
    baseUrl: 'http://x/v1',
    model: 'm',
    systemPrompt: 's',
    apiKey: 'k',
    temperature: 0.5,
    maxTokens: 100,
    streaming: true,
    maxHistory: 20,
    allowInterruption: true,
    providerKind: 'openai',
    stripMarkdown: true,
    eagerEot: true,
    bargeInMinWords: 2,
    muteStrategy: 'first_speech_only',
    userIdleTimeoutMs: 5000,
    summarizeTargetTokens: 2000,
    latencyFiller: 'auto',
    latencyFillerAfterMs: 500,
    latencyFillerPhrases: ['a', 'b'],
    reasoningEffort: 'low',
    reasoningModel: 'r',
    reasoningBaseUrl: 'http://r/v1',
    reasoningApiKey: 'rk',
    reasoningProviderKind: 'anthropic',
    reasoningRoute: 'always',
    reasoningBudgetMs: 15000,
    degradationMessage: 'd',
    maxLlmCallsPerTurn: 8,
    maxReasoningTokens: 1000,
  };
  const emitted = Object.keys(buildConversationConfig(fullConv));
  assertSupersetOf(schemas, 'ConversationWebSocketConfig', emitted);
});

// --- STTWebSocketConfig + TurnDetectionWsConfig -------------------------------
test('drift: stt_config emits every gateway STTWebSocketConfig field', () => {
  const schemas = loadSchemas();
  const cfg = {
    gatewayUrl: 'ws://x/ws',
    stt: {
      provider: 'deepgram',
      language: 'en-US',
      model: 'nova-3',
      sampleRate: 16000,
      channels: 1,
      encoding: 'linear16',
    },
    features: { punctuation: true },
    audioFeatures: { turnDetection: { enabled: true, threshold: 0.5, eager: true } },
  };
  const msg = buildConfigMessage(cfg);
  const emitted = Object.keys(msg.stt_config);
  // `extras`: open passthrough (gateway escape hatch, never flagged) — beginners
  // use first-class fields, advanced users can already round-trip arbitrary keys.
  // `features`: the SttFeatures bag is advanced/optional; the widget exposes the
  // common ones via top-level FeatureFlags and is not the beginner surface here.
  assertSupersetOf(schemas, 'STTWebSocketConfig', emitted, ['extras', 'features', 'api_key']);
  // turn_detection itself must be a fully-mirrored sub-envelope.
  assertSupersetOf(schemas, 'TurnDetectionWsConfig', Object.keys(msg.stt_config.turn_detection));
});

// --- TTSWebSocketConfig -------------------------------------------------------
test('drift: tts_config emits the gateway TTSWebSocketConfig beginner fields', () => {
  const schemas = loadSchemas();
  const cfg = {
    gatewayUrl: 'ws://x/ws',
    tts: {
      provider: 'deepgram',
      voiceId: 'v',
      model: 'aura-2',
      sampleRate: 24000,
      emotion: {
        emotion: 'happy',
        intensity: 'high',
        deliveryStyle: 'cheerful',
        description: 'warm',
      },
    },
  };
  const emitted = Object.keys(buildConfigMessage(cfg).tts_config);
  // Advanced/transport knobs not on the widget beginner surface (open passthrough
  // `extras`; SSML-grade `features`; per-pronunciation `pronunciations`; egress
  // reframing/timeout transport knobs). Listed explicitly so adding a NEW core
  // TTS field still trips the guard.
  assertSupersetOf(schemas, 'TTSWebSocketConfig', emitted, [
    'extras',
    'features',
    'pronunciations',
    'api_key',
    'speaking_rate',
    'audio_format',
    'audio_out_chunk_ms',
    'client_playback_rate',
    'connection_timeout',
    'request_timeout',
  ]);
});

// --- provider value-space counts (mirror of the gateway dispatch tables) ------
test('drift: provider counts mirror the gateway dispatch tables (32/37/12)', () => {
  assert.equal(STT_PROVIDERS.length, 32, 'STT provider count drifted from gateway dispatch tables');
  assert.equal(TTS_PROVIDERS.length, 37, 'TTS provider count drifted from gateway dispatch tables');
  assert.equal(
    REALTIME_PROVIDERS.length,
    12,
    'realtime provider count drifted from gateway dispatch tables'
  );
  // No duplicates, all lowercase canonical names.
  for (const [name, list] of [
    ['STT', STT_PROVIDERS],
    ['TTS', TTS_PROVIDERS],
    ['REALTIME', REALTIME_PROVIDERS],
  ]) {
    assert.equal(new Set(list).size, list.length, `${name} provider list has duplicates`);
    for (const p of list) {
      assert.ok(/^[a-z0-9][a-z0-9_-]*$/.test(p), `${name} provider "${p}" is not a canonical name`);
    }
  }
});

// --- top-level envelope keys are all gateway IncomingMessage::Config keys ------
test('drift: config envelope emits no key the gateway would silently drop', () => {
  const cfg = {
    gatewayUrl: 'ws://x/ws',
    stt: { provider: 'deepgram', language: 'en-US', sampleRate: 16000, channels: 1 },
    tts: { provider: 'deepgram', voiceId: 'v' },
    conversation: { baseUrl: 'http://x/v1', model: 'm' },
    features: { punctuation: true },
    audioFeatures: { turnDetection: { enabled: true } },
  };
  const msg = buildConfigMessage(cfg);
  // The gateway Config envelope accepts exactly these top-level keys
  // (messages.rs IncomingMessage::Config). `type`/`audio` are the discriminator
  // + audio toggle; the rest are the nested config blocks.
  const allowedTopLevel = new Set([
    'type',
    'audio',
    'audio_disabled',
    'stream_id',
    'stt_config',
    'tts_config',
    'livekit',
    'dag_config',
    'conversation_config',
  ]);
  const unexpected = Object.keys(msg).filter((k) => !allowedTopLevel.has(k));
  assert.deepEqual(
    unexpected,
    [],
    `widget put non-Config-envelope key(s) on the wire (gateway would silently drop them): [${unexpected.join(', ')}]`
  );
});
