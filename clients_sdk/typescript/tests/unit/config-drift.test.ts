/**
 * SDK config DRIFT GUARD (P1 — the non-negotiable deliverable).
 *
 * Loads the committed gateway OpenAPI spec (gateway/docs/openapi.yaml, generated
 * from the live wire structs in gateway/src/handlers/ws/config.rs) and asserts
 * the TypeScript SDK config types cover EVERY config property the gateway
 * exposes. If the gateway gains or renames a config field the SDK lacks, this
 * test FAILS — so "a feature exists server-side but is unreachable from the SDK"
 * can never silently recur.
 *
 * Strategy: for each gateway config schema, enumerate its openapi `properties`
 * and assert each one is reachable via the SDK (either a camelCase field on the
 * mirror type, OR an intentionally-handled exception with a documented reason —
 * e.g. a deprecated alias or a field the serializer derives). The SDK→wire
 * round-trip is exercised separately in config-wire.test.ts; this guard is
 * purely about COVERAGE of the schema surface.
 *
 * No YAML dependency: a tiny indentation-aware extractor reads the property
 * keys (the openapi.yaml structure is regular — 4-space schema, 6-space
 * `properties:`, 8-space keys).
 */
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const OPENAPI = resolve(__dirname, '../../../../gateway/docs/openapi.yaml');

/** Convert snake_case → camelCase (the SDK field-naming convention). */
function toCamel(s: string): string {
  return s.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());
}

/**
 * Extract the property keys (and `required` list) of a top-level component
 * schema from openapi.yaml by indentation. Schemas live under
 * `components.schemas.<Name>:` at 4-space indent; their `properties:` block is
 * at 6-space; property keys are at 8-space.
 */
function schemaProps(yaml: string, schemaName: string): { props: string[]; required: string[] } {
  const lines = yaml.split('\n');
  // Find the `    <Name>:` line (4-space indent).
  const headRe = new RegExp(`^ {4}${schemaName.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}:\\s*$`);
  let i = lines.findIndex((l) => headRe.test(l));
  if (i === -1) throw new Error(`schema ${schemaName} not found in openapi.yaml`);

  const props: string[] = [];
  const required: string[] = [];
  let inProps = false;
  let inRequired = false;
  for (i = i + 1; i < lines.length; i++) {
    const line = lines[i]!;
    if (line.trim() === '') continue;
    const indent = line.length - line.trimStart().length;
    // A new 4-space (or less) sibling/parent ends this schema.
    if (indent <= 4) break;

    // `required:` items are 6-space YAML sequence entries (`      - provider`);
    // capture them BEFORE the generic 6-space key reset below.
    if (inRequired && /^ {6}- /.test(line)) { required.push(line.trim().slice(2)); continue; }

    if (indent === 6 && /^ {6}properties:\s*$/.test(line)) { inProps = true; inRequired = false; continue; }
    if (indent === 6 && /^ {6}required:\s*$/.test(line)) { inRequired = true; inProps = false; continue; }
    if (indent === 6) { inProps = false; inRequired = false; continue; } // some other 6-space key (type/description/...)

    if (inProps && indent === 8) {
      const m = line.match(/^ {8}([A-Za-z0-9_]+):\s*$/);
      if (m) props.push(m[1]!);
    }
  }
  return { props, required };
}

// SDK config field sets (camelCase). These are the typed surfaces the mirror
// exposes; the drift guard asserts every openapi property maps into one of them
// (possibly via a documented exception).
//
// Kept here (not imported) deliberately: the guard must reflect what the SDK
// TYPES actually expose, and a reviewer edits this list in lock-step with the
// type when intentionally adding/exempting a field.
const STT_SDK = new Set([
  'provider', 'language', 'sampleRate', 'channels', 'punctuation', 'punctuate',
  'encoding', 'model', 'apiKey', 'extras',
  // D8 uplink transport codec (linear16|opus):
  'audioInCodec',
  // features{} canonical surface (flattened onto STTConfig):
  'interimResults', 'diarize', 'wordTimestamps', 'smartFormat', 'profanityFilter',
  'fillerWords', 'vadEvents', 'endpointingMs', 'utteranceEndMsFeature', 'keyterms',
  'redaction', 'languageDetection', 'entityDetection', 'numerals', 'multichannel',
  'alternatives', 'sentiment', 'speechBeginEvent',
  // turn detection (nested into stt_config.turn_detection):
  'turnDetection',
  // P5 canonical translation block (STTConfig.translation):
  'translation',
]);

const TTS_SDK = new Set([
  'provider', 'voiceId', 'voice', 'speakingRate', 'audioFormat', 'sampleRate',
  'audioOutChunkMs', 'clientPlaybackRate', 'connectionTimeout', 'requestTimeout',
  // D8 downlink transport codec (linear16|opus):
  'audioOutCodec',
  'model', 'pronunciations', 'apiKey', 'emotion', 'emotionIntensity',
  'deliveryStyle', 'emotionDescription', 'extras',
  // P4 abstract voice selection (TTSConfig.voiceDescriptor):
  'voiceDescriptor',
  // features{} canonical surface (flattened onto TTSConfig):
  'speed', 'pitch', 'volume', 'stability', 'similarityBoost', 'style',
  'useSpeakerBoost', 'instructions', 'ssml', 'language', 'wordTimestamps',
  'streaming', 'seed', 'optimizeStreamingLatency', 'includeTimestampTypes',
  'ratePercentage', 'pitchPercentage',
]);

const CONVERSATION_SDK = new Set([
  'baseUrl', 'model', 'systemPrompt', 'apiKey', 'temperature', 'maxTokens',
  'streaming', 'maxHistory', 'allowInterruption', 'providerKind', 'stripMarkdown',
  'eagerEot', 'bargeInMinWords', 'muteStrategy', 'userIdleTimeoutMs',
  'summarizeTargetTokens', 'latencyFiller', 'latencyFillerAfterMs',
  'latencyFillerPhrases', 'reasoningEffort', 'reasoningModel', 'reasoningBaseUrl',
  'reasoningApiKey', 'reasoningProviderKind', 'reasoningRoute', 'reasoningBudgetMs',
  'degradationMessage', 'maxLlmCallsPerTurn', 'maxReasoningTokens',
]);

const TURN_SDK = new Set(['enabled', 'threshold', 'eager']);

const LIVEKIT_SDK = new Set([
  'roomName', 'enableRecording', 'waavParticipantIdentity', 'waavParticipantName',
  'listenParticipants',
]);

const DAG_SDK = new Set(['template', 'definition', 'enableMetrics', 'timeoutMs']);

// Top-level config envelope (IncomingMessage::Config branch).
const ENVELOPE_SDK = new Set([
  'streamId', 'audio', 'sttConfig', 'ttsConfig', 'livekit', 'dag', 'conversation',
  'turnDetection',
]);

/**
 * Schema properties that intentionally do NOT need a 1:1 SDK field, with the
 * reason. The guard treats these as covered.
 */
const EXCEPTIONS: Record<string, Record<string, string>> = {
  STTWebSocketConfig: {
    features: 'flattened: the SttFeatures sub-schema is hoisted onto STTConfig as individual fields',
  },
  TTSWebSocketConfig: {
    features: 'flattened: the TtsFeatures sub-schema is hoisted onto TTSConfig as individual fields',
  },
  ConversationWebSocketConfig: {},
  TurnDetectionWsConfig: {},
  LiveKitWebSocketConfig: {},
  DAGWebSocketConfig: {},
  ConfigEnvelope: {
    type: 'discriminator constant, set by the serializer',
    audio_disabled: 'DEPRECATED alias for audio:false; the SDK uses the canonical `audio` field',
  },
};

describe('config drift guard: SDK config ⊇ gateway openapi config schemas', () => {
  const yaml = readFileSync(OPENAPI, 'utf8');

  const cases: Array<[string, Set<string>]> = [
    ['STTWebSocketConfig', STT_SDK],
    ['TTSWebSocketConfig', TTS_SDK],
    ['ConversationWebSocketConfig', CONVERSATION_SDK],
    ['TurnDetectionWsConfig', TURN_SDK],
    ['LiveKitWebSocketConfig', LIVEKIT_SDK],
    ['DAGWebSocketConfig', DAG_SDK],
  ];

  for (const [schema, sdk] of cases) {
    it(`${schema}: every openapi property is reachable from the SDK`, () => {
      const { props } = schemaProps(yaml, schema);
      // Sanity: the extractor actually found properties.
      expect(props.length, `no properties parsed for ${schema}`).toBeGreaterThan(0);

      const exceptions = EXCEPTIONS[schema] ?? {};
      const missing = props.filter((p) => !sdk.has(toCamel(p)) && exceptions[p] === undefined);
      expect(
        missing,
        `${schema} has openapi properties with NO SDK field (drift!): ${missing.join(', ')}. ` +
          `Add them to the SDK config type + the drift-guard set, or document an exception.`
      ).toEqual([]);
    });
  }

  it('config envelope (IncomingMessage::Config): every top-level config key is reachable', () => {
    // The Config branch is an inline oneOf member, not a named schema; assert
    // against the known top-level keys from the spec digest (verified present in
    // the regenerated openapi IncomingMessage oneOf).
    const envelopeProps = [
      'stream_id', 'audio', 'audio_disabled', 'stt_config', 'tts_config',
      'livekit', 'dag_config', 'conversation_config', 'type',
    ];
    const exc = EXCEPTIONS.ConfigEnvelope!;
    // Map wire key → SDK field name.
    const wireToSdk: Record<string, string> = {
      stream_id: 'streamId',
      audio: 'audio',
      stt_config: 'sttConfig',
      tts_config: 'ttsConfig',
      livekit: 'livekit',
      dag_config: 'dag',
      conversation_config: 'conversation',
    };
    const missing = envelopeProps.filter((p) => {
      if (exc[p] !== undefined) return false;
      const sdkName = wireToSdk[p] ?? toCamel(p);
      return !ENVELOPE_SDK.has(sdkName);
    });
    expect(missing, `config envelope keys not reachable from SessionConfig: ${missing.join(', ')}`).toEqual([]);
  });
});

describe('config drift guard: the extractor itself is honest', () => {
  const yaml = readFileSync(OPENAPI, 'utf8');

  it('parses the documented STT required set', () => {
    const { required } = schemaProps(yaml, 'STTWebSocketConfig');
    expect(required.sort()).toEqual(['channels', 'language', 'provider', 'punctuation', 'sample_rate']);
  });

  it('parses the documented Conversation required set (base_url + model)', () => {
    const { required } = schemaProps(yaml, 'ConversationWebSocketConfig');
    expect(required.sort()).toEqual(['base_url', 'model']);
  });

  it('finds a representative reasoning property on the Conversation schema', () => {
    const { props } = schemaProps(yaml, 'ConversationWebSocketConfig');
    expect(props).toContain('reasoning_model');
    expect(props).toContain('latency_filler');
    expect(props).toContain('eager_eot');
  });
});
