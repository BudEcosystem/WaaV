/**
 * P4 abstract voice selection: `<bud-widget data-voice-*>` describes the voice you
 * WANT (gender/locale/accent/age/style/name-hint) and the gateway resolves it to a
 * concrete provider voice_id server-side. The attributes must reach the `/ws`
 * config envelope as a `tts_config.voice_descriptor` object (snake_case). A raw
 * `data-tts-voice-id` always wins. Plus: the widened emotion/delivery value space
 * serializes verbatim onto tts_config.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { widget, makeWidgetElement } from './harness.mjs';

const { parseConfigFromAttributes, mergeConfig, buildConfigMessage } = widget;

function envelopeFromAttrs(attrs) {
  const el = makeWidgetElement(attrs);
  const cfg = mergeConfig(parseConfigFromAttributes(el));
  return buildConfigMessage(cfg);
}

test('data-voice-* yields a tts_config.voice_descriptor object (snake_case)', () => {
  const msg = envelopeFromAttrs({
    voiceGender: 'female',
    voiceLocale: 'en-US',
    voiceAge: 'young',
    voiceStyle: 'warm',
    voiceNameHint: 'asteria',
  });
  assert.equal(msg.type, 'config');
  assert.ok(msg.tts_config, 'a tts_config is created from data-voice-* alone');
  const vd = msg.tts_config.voice_descriptor;
  assert.ok(vd, 'voice_descriptor object present');
  assert.equal(vd.gender, 'female');
  assert.equal(vd.locale, 'en-US');
  assert.equal(vd.age, 'young');
  assert.equal(vd.style, 'warm');
  // camelCase name-hint maps to snake_case name_hint.
  assert.equal(vd.name_hint, 'asteria');
});

test('no data-voice-* and no tts => no voice_descriptor on the wire', () => {
  const msg = envelopeFromAttrs({ alias: 'support-bot' });
  // tts defaults exist via mergeConfig, but no descriptor was set.
  assert.equal(msg.tts_config?.voice_descriptor, undefined);
});

test('emits only the set descriptor fields', () => {
  const msg = envelopeFromAttrs({ voiceGender: 'male', voiceAccent: 'british' });
  assert.deepEqual(msg.tts_config.voice_descriptor, { gender: 'male', accent: 'british' });
});

test('a raw data-tts-voice-id is still sent alongside the descriptor (gateway picks voice_id)', () => {
  const msg = envelopeFromAttrs({ ttsVoiceId: 'aura-asteria-en', voiceGender: 'female' });
  assert.equal(msg.tts_config.voice_id, 'aura-asteria-en');
  // The descriptor is still carried; the gateway ignores it when voice_id is set.
  assert.equal(msg.tts_config.voice_descriptor.gender, 'female');
});

test('widened P4 emotion + scenario delivery style serialize verbatim', () => {
  const msg = envelopeFromAttrs({
    ttsProvider: 'microsoft-azure',
    ttsEmotion: 'amazed',
    ttsDeliveryStyle: 'newscast',
  });
  assert.equal(msg.tts_config.emotion, 'amazed');
  assert.equal(msg.tts_config.delivery_style, 'newscast');
});
