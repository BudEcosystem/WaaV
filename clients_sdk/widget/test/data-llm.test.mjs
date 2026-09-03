/**
 * data-llm-* flagship path: a single HTML tag with the four beginner attributes
 * (data-llm-base-url, data-llm-model, data-stt-provider, data-tts-voice) must
 * produce a correct, fully-nested `/ws` config envelope — conversation_config
 * (the LLM loop that makes the bot talk back) plus a TTS config implied by the
 * bare voice — and the reasoning / barge-in / latency-filler dials must nest
 * correctly under conversation_config and stt_config.turn_detection.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { widget, makeWidgetElement } from './harness.mjs';

const { parseConfigFromAttributes, mergeConfig, buildConfigMessage } = widget;

/** Parse data-* attrs -> WidgetConfig -> wire envelope, like the live widget. */
function envelopeFromAttrs(attrs) {
  const el = makeWidgetElement(attrs);
  const cfg = mergeConfig(parseConfigFromAttributes(el));
  return buildConfigMessage(cfg);
}

test('minimal 4-attribute flagship tag yields a full talking-loop envelope', () => {
  const msg = envelopeFromAttrs({
    llmBaseUrl: 'http://127.0.0.1:11434/v1',
    llmModel: 'qwen2.5:7b-instruct',
    sttProvider: 'deepgram',
    ttsVoice: 'aura-asteria-en',
  });

  assert.equal(msg.type, 'config');
  assert.equal(msg.audio, true);

  // conversation_config is what drives STT -> LLM -> TTS (the bot talks back).
  assert.ok(msg.conversation_config, 'conversation_config must be present');
  assert.equal(msg.conversation_config.base_url, 'http://127.0.0.1:11434/v1');
  assert.equal(msg.conversation_config.model, 'qwen2.5:7b-instruct');

  // A bare data-tts-voice implies a TTS config so the bot can actually speak.
  assert.ok(msg.tts_config, 'a bare data-tts-voice must imply a tts_config');
  assert.equal(msg.tts_config.voice_id, 'aura-asteria-en');
  assert.equal(msg.tts_config.provider, 'deepgram', 'tts provider defaults to deepgram');

  // STT must be present and use the requested provider.
  assert.ok(msg.stt_config, 'stt_config must be present');
  assert.equal(msg.stt_config.provider, 'deepgram');
});

test('NO top-level non-keys leak onto the wire', () => {
  const msg = envelopeFromAttrs({
    llmBaseUrl: 'http://127.0.0.1:11434/v1',
    llmModel: 'm',
    sttProvider: 'deepgram',
    ttsVoice: 'v',
    turnDetection: 'true',
  });
  // These were the historical SDK bugs: top-level audio_features / realtime_config
  // / a top-level turn_detection are silently dropped by the gateway.
  assert.equal(msg.audio_features, undefined);
  assert.equal(msg.realtime_config, undefined);
  assert.equal(msg.turn_detection, undefined, 'turn_detection must NOT be top-level');
});

test('turn_detection nests under stt_config (not top-level)', () => {
  const msg = envelopeFromAttrs({
    llmBaseUrl: 'http://127.0.0.1:11434/v1',
    llmModel: 'm',
    sttProvider: 'deepgram',
    ttsVoice: 'v',
    turnDetection: 'true',
    turnDetectionThreshold: '0.6',
    turnDetectionEager: 'true',
  });
  const td = msg.stt_config.turn_detection;
  assert.ok(td, 'stt_config.turn_detection must be present');
  assert.equal(td.enabled, true);
  assert.equal(td.threshold, 0.6);
  assert.equal(td.eager, true);
  // Only enabled/threshold/eager are wire keys; no silence_ms / prefix_padding_ms.
  assert.deepEqual(Object.keys(td).sort(), ['eager', 'enabled', 'threshold']);
});

test('reasoning + barge-in + latency-filler dials nest under conversation_config', () => {
  const msg = envelopeFromAttrs({
    llmBaseUrl: 'http://127.0.0.1:11434/v1',
    llmModel: 'qwen2.5:7b-instruct',
    sttProvider: 'deepgram',
    ttsVoice: 'v',
    // realtime-reasoning two-tier
    llmReasoningModel: 'qwq:32b',
    llmReasoningEffort: 'medium',
    llmReasoningRoute: 'auto',
    llmReasoningBudgetMs: '15000',
    // barge-in
    llmEagerEot: 'true',
    llmBargeInMinWords: '3',
    llmMuteStrategy: 'until_first_bot_complete',
    llmAllowInterruption: 'false',
    // latency filler
    llmLatencyFiller: 'aggressive',
    llmLatencyFillerAfterMs: '700',
    llmLatencyFillerPhrases: 'let me think, one sec',
  });
  const c = msg.conversation_config;
  // two-tier
  assert.equal(c.reasoning_model, 'qwq:32b');
  assert.equal(c.reasoning_effort, 'medium');
  assert.equal(c.reasoning_route, 'auto');
  assert.equal(c.reasoning_budget_ms, 15000);
  // barge-in
  assert.equal(c.eager_eot, true);
  assert.equal(c.barge_in_min_words, 3);
  assert.equal(c.mute_strategy, 'until_first_bot_complete');
  assert.equal(c.allow_interruption, false);
  // latency filler
  assert.equal(c.latency_filler, 'aggressive');
  assert.equal(c.latency_filler_after_ms, 700);
  assert.deepEqual(c.latency_filler_phrases, ['let me think', 'one sec']);
});

test('eager EoT pairs with stt_config.turn_detection.eager when both set', () => {
  const msg = envelopeFromAttrs({
    llmBaseUrl: 'http://127.0.0.1:11434/v1',
    llmModel: 'm',
    sttProvider: 'deepgram',
    ttsVoice: 'v',
    llmEagerEot: 'true',
    turnDetection: 'true',
    turnDetectionEager: 'true',
  });
  assert.equal(msg.conversation_config.eager_eot, true);
  assert.equal(msg.stt_config.turn_detection.eager, true);
});

test('no data-llm-* => no conversation_config (raw STT/TTS only)', () => {
  const msg = envelopeFromAttrs({ sttProvider: 'deepgram', ttsVoice: 'v' });
  assert.equal(msg.conversation_config, undefined);
  assert.ok(msg.stt_config);
  assert.ok(msg.tts_config);
});

test('snake_case keys only on conversation_config (no camelCase leak)', () => {
  const msg = envelopeFromAttrs({
    llmBaseUrl: 'http://127.0.0.1:11434/v1',
    llmModel: 'm',
    sttProvider: 'deepgram',
    ttsVoice: 'v',
    llmReasoningModel: 'r',
    llmBargeInMinWords: '2',
  });
  for (const key of Object.keys(msg.conversation_config)) {
    assert.ok(
      /^[a-z][a-z0-9_]*$/.test(key),
      `conversation_config key "${key}" must be snake_case`
    );
  }
});
