/**
 * P3 proxy/alias: `<bud-widget data-alias="support-bot">` is a complete agent —
 * the single attribute must reach the `/ws` config envelope as a top-level
 * `alias`, which the gateway resolves to a full {stt,tts,llm,dag} bundle (and can
 * re-point server-side with zero client change). An explicit data-* attribute
 * still overrides the alias default (client-wins, mirrored gateway-side).
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

test('data-alias alone yields a config envelope with a top-level alias', () => {
  const msg = envelopeFromAttrs({ alias: 'support-bot' });
  assert.equal(msg.type, 'config');
  assert.equal(msg.alias, 'support-bot');
});

test('no data-alias => no alias key on the wire', () => {
  const msg = envelopeFromAttrs({ ttsVoice: 'aura-asteria-en', sttProvider: 'deepgram' });
  assert.equal(msg.alias, undefined);
});

test('data-alias composes with an explicit stt provider (override reaches the wire)', () => {
  const msg = envelopeFromAttrs({ alias: 'support-bot', sttProvider: 'openai' });
  assert.equal(msg.alias, 'support-bot');
  assert.ok(msg.stt_config, 'explicit stt provider still serialized alongside the alias');
  assert.equal(msg.stt_config.provider, 'openai');
});
