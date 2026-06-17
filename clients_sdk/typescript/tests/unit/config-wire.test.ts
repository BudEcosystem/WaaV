/**
 * Config → wire serialization tests (P1).
 *
 * Asserts the SDK serializes the FULL 1:1 config surface into the EXACT gateway
 * wire shapes:
 *   - advanced STT/TTS features nest under `features{}` (not silently dropped)
 *   - ML turn detection nests under `stt_config.turn_detection`
 *   - the conversation/agent loop serializes into `conversation_config`
 *   - DAG into `dag_config`
 *   - the DEAD top-level `features` key is GONE
 *   - no-canonical-field knobs route through `extras`
 */
import { describe, it, expect } from 'vitest';
import { serializeMessage, createConfigMessage } from '../../src/ws/messages.js';
import type { SDKConfigMessage } from '../../src/ws/messages.js';
import { conversationConfigToWire } from '../../src/types/conversation.js';
import type { ConversationConfig } from '../../src/types/conversation.js';

function wireOf(msg: SDKConfigMessage): Record<string, any> {
  return JSON.parse(serializeMessage(msg));
}

describe('STT config → wire (features nesting + turn_detection)', () => {
  it('nests advanced STT features under stt_config.features{} (canonical names)', () => {
    const wire = wireOf({
      type: 'config',
      stt: {
        provider: 'deepgram',
        language: 'en-US',
        sampleRate: 16000,
        channels: 1,
        punctuation: true,
        model: 'nova-3',
        interimResults: true,
        diarize: true,
        smartFormat: true,
        wordTimestamps: true,
        numerals: true,
        keyterms: ['Bud', 'WaaV'],
      },
    });
    expect(wire.stt_config.provider).toBe('deepgram');
    // Canonical SttFeatures keys, NOT top-level.
    expect(wire.stt_config.features).toEqual({
      interim_results: true,
      diarization: true, // diarize → canonical `diarization`
      smart_format: true,
      word_timestamps: true,
      numerals: true,
      keyterms: ['Bud', 'WaaV'],
    });
    // These advanced fields are NOT leaked at the top level of stt_config.
    expect(wire.stt_config.diarize).toBeUndefined();
    expect(wire.stt_config.word_timestamps).toBeUndefined();
  });

  it('nests turn detection under stt_config.turn_detection (its real wire home)', () => {
    const wire = wireOf({
      type: 'config',
      stt: { provider: 'deepgram', language: 'en-US', model: 'nova-3' },
      turnDetection: { enabled: true, threshold: 0.6, eager: true },
    });
    expect(wire.stt_config.turn_detection).toEqual({ enabled: true, threshold: 0.6, eager: true });
    // NEVER a top-level key (that was the original silent-drop bug).
    expect(wire.turn_detection).toBeUndefined();
  });

  it('omits turn_detection when not enabled', () => {
    const wire = wireOf({
      type: 'config',
      stt: { provider: 'deepgram', language: 'en-US', model: 'nova-3' },
      turnDetection: { enabled: false, threshold: 0.6 },
    });
    expect(wire.stt_config.turn_detection).toBeUndefined();
  });

  it('routes no-canonical-field customVocabulary through extras', () => {
    const wire = wireOf({
      type: 'config',
      stt: {
        provider: 'deepgram',
        language: 'en-US',
        model: 'nova-3',
        customVocabulary: ['Kubernetes', 'Anthropic'],
        extras: { foo: 'bar' },
      },
    });
    expect(wire.stt_config.extras).toEqual({ foo: 'bar', custom_vocabulary: ['Kubernetes', 'Anthropic'] });
  });

  it('accepts `punctuate` (Deepgram alias) as `punctuation`', () => {
    const wire = wireOf({
      type: 'config',
      stt: { provider: 'deepgram', language: 'en-US', model: 'nova-3', punctuate: false },
    });
    expect(wire.stt_config.punctuation).toBe(false);
  });
});

describe('TTS config → wire (features nesting + extras)', () => {
  it('nests advanced TTS features under tts_config.features{} and keeps speaking_rate top-level', () => {
    const wire = wireOf({
      type: 'config',
      tts: {
        provider: 'elevenlabs',
        model: 'eleven_turbo_v2',
        voiceId: 'rachel',
        speakingRate: 1.1,
        stability: 0.5,
        similarityBoost: 0.8,
        style: 0.3,
        useSpeakerBoost: true,
        ssml: true,
      },
    });
    expect(wire.tts_config.provider).toBe('elevenlabs');
    expect(wire.tts_config.voice_id).toBe('rachel');
    expect(wire.tts_config.speaking_rate).toBe(1.1); // top-level egress rate
    expect(wire.tts_config.features).toEqual({
      stability: 0.5,
      similarity_boost: 0.8,
      style: 0.3,
      use_speaker_boost: true,
      ssml: true,
    });
  });

  it('routes Hume-only knobs (no canonical field) through extras', () => {
    const wire = wireOf({
      type: 'config',
      tts: {
        provider: 'hume',
        model: 'octave',
        actingInstructions: 'whispered fearfully',
        instantMode: true,
      },
    });
    expect(wire.tts_config.extras).toEqual({
      acting_instructions: 'whispered fearfully',
      instant_mode: true,
    });
  });

  it('serializes the unified emotion system at the top level', () => {
    const wire = wireOf({
      type: 'config',
      tts: {
        provider: 'cartesia',
        model: 'sonic',
        emotion: 'excited',
        emotionIntensity: 'high',
        deliveryStyle: 'cheerful',
      },
    });
    expect(wire.tts_config.emotion).toBe('excited');
    expect(wire.tts_config.emotion_intensity).toBe('high');
    expect(wire.tts_config.delivery_style).toBe('cheerful');
  });
});

describe('conversation + dag config → wire', () => {
  it('serializes the conversation/agent loop into conversation_config', () => {
    const conversation: ConversationConfig = {
      baseUrl: 'http://localhost:11434/v1',
      model: 'llama3.2:1b',
      systemPrompt: 'You are helpful.',
      reasoningEffort: 'minimal',
      reasoningModel: 'deepseek-r1',
      latencyFiller: 'auto',
      muteStrategy: 'until_first_bot_complete',
      bargeInMinWords: 3,
      eagerEot: true,
    };
    const wire = wireOf({ type: 'config', conversation });
    expect(wire.conversation_config).toEqual({
      base_url: 'http://localhost:11434/v1',
      model: 'llama3.2:1b',
      system_prompt: 'You are helpful.',
      reasoning_effort: 'minimal',
      reasoning_model: 'deepseek-r1',
      latency_filler: 'auto',
      mute_strategy: 'until_first_bot_complete',
      barge_in_min_words: 3,
      eager_eot: true,
    });
  });

  it('conversationConfigToWire emits ONLY base_url+model when nothing else is set', () => {
    expect(conversationConfigToWire({ baseUrl: 'u', model: 'm' })).toEqual({ base_url: 'u', model: 'm' });
  });

  it('never serializes an undefined optional field', () => {
    const w = conversationConfigToWire({ baseUrl: 'u', model: 'm', temperature: 0 });
    // temperature:0 IS sent (0 !== undefined); a missing field is absent.
    expect(w.temperature).toBe(0);
    expect('max_tokens' in w).toBe(false);
  });

  it('serializes DAG config into dag_config', () => {
    const wire = wireOf({
      type: 'config',
      dag: { template: 'support-bot', enableMetrics: true, timeoutMs: 5000 },
    });
    expect(wire.dag_config).toEqual({ template: 'support-bot', enable_metrics: true, timeout_ms: 5000 });
  });
});

describe('the DEAD top-level `features` key is gone', () => {
  it('does NOT emit a top-level features key even when legacy FeatureFlags are passed', () => {
    const msg = createConfigMessage(
      { provider: 'deepgram', language: 'en-US', model: 'nova-3' },
      undefined,
      undefined,
      // legacy FeatureFlags
      { vad: true, noiseCancellation: true, speakerDiarization: true, interimResults: true,
        punctuation: true, profanityFilter: false, smartFormat: true, wordTimestamps: false,
        echoCancellation: true, fillerWords: false }
    );
    const wire = wireOf(msg);
    // The gateway never recognized a top-level `features` key — it must be absent.
    expect(wire.features).toBeUndefined();
    // Instead the recognised flags fold into stt_config.features{}.
    expect(wire.stt_config.features.diarization).toBe(true);
    expect(wire.stt_config.features.smart_format).toBe(true);
  });
});

describe('streamId + audio envelope', () => {
  it('serializes stream_id and audio via the extra options bag', () => {
    const msg = createConfigMessage(undefined, undefined, undefined, undefined, {
      streamId: 'sess-1',
      audio: false,
    });
    const wire = wireOf(msg);
    expect(wire.stream_id).toBe('sess-1');
    expect(wire.audio).toBe(false);
  });
});
