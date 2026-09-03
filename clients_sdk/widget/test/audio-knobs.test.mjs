/**
 * D2 gateway-leverage audio knobs: the widget config envelope must carry the two
 * TTSWebSocketConfig knobs that let the GATEWAY do the downlink audio work so the
 * browser player doesn't:
 *
 *   - tts_config.audio_out_chunk_ms = 20  -> the gateway re-frames PCM egress into
 *     ~20ms chunks, so a barge-in `clear` truncates within ONE server chunk and the
 *     chunks arrive pre-sized for the player's scheduled-chunk playout clock.
 *   - tts_config.client_playback_rate = <sink rate> -> the gateway resamples downlink
 *     PCM to EXACTLY the widget's AudioContext rate, so the player does ZERO downlink
 *     resampling (the widget's AudioPlayer is created at tts.sampleRate || 24000).
 *
 * These keys are real gateway fields (gateway config.rs:540/549, openapi.yaml) and
 * are deliberately allow-listed in the drift guard's TTS ignore set, so this test is
 * what actually pins them onto the wire.
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

test('tts_config carries audio_out_chunk_ms=20 for server-side 20ms egress reframing', () => {
  const msg = envelopeFromAttrs({ ttsProvider: 'deepgram', ttsVoice: 'aura-asteria-en' });
  assert.ok(msg.tts_config, 'a tts_config is built');
  assert.equal(msg.tts_config.audio_out_chunk_ms, 20);
});

test('client_playback_rate equals the sink rate (default 24000) so the player never resamples', () => {
  const msg = envelopeFromAttrs({ ttsProvider: 'deepgram', ttsVoice: 'v' });
  // Default sink rate = tts.sampleRate || 24000 (the widget's AudioContext rate).
  assert.equal(msg.tts_config.client_playback_rate, 24000);
  // And it must equal the sample_rate the gateway uses for synthesis.
  assert.equal(msg.tts_config.sample_rate, msg.tts_config.client_playback_rate);
});

test('client_playback_rate tracks an explicit tts sample rate (still == sink == sample_rate)', () => {
  const el = makeWidgetElement({ ttsProvider: 'deepgram', ttsVoice: 'v' });
  // data-* parsing has no tts-sample-rate attribute; set it on the parsed config
  // directly to prove the serializer keys all three off ONE sink rate.
  const cfg = mergeConfig(parseConfigFromAttributes(el));
  cfg.tts.sampleRate = 48000;
  const msg = buildConfigMessage(cfg);
  assert.equal(msg.tts_config.sample_rate, 48000);
  assert.equal(msg.tts_config.client_playback_rate, 48000);
  assert.equal(msg.tts_config.audio_out_chunk_ms, 20);
});

test('audio knobs are snake_case integer values the gateway accepts (not strings)', () => {
  const msg = envelopeFromAttrs({ ttsProvider: 'cartesia', ttsVoice: 'v' });
  assert.equal(typeof msg.tts_config.audio_out_chunk_ms, 'number');
  assert.equal(typeof msg.tts_config.client_playback_rate, 'number');
  assert.ok(Number.isInteger(msg.tts_config.audio_out_chunk_ms));
  assert.ok(Number.isInteger(msg.tts_config.client_playback_rate));
});
