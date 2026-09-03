/**
 * P4 SDK mirror (TypeScript): VoiceDescriptor on the TTS config, voices.clone
 * canonical body, widened emotion/delivery value space.
 *
 * Gateway side committed at 8d18c4e (emotion 22→44, VoiceDescriptor resolver,
 * widened voice-clone REST). This mirrors the canonical VALUE SPACE in the SDK; the
 * MAPPING stays gateway-side. Style mirrors tests/unit/alias.test.ts (P3).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { serializeMessage, createConfigMessage } from '../../src/ws/messages.js';
import { cloneVoice, cloneVoiceAndWait } from '../../src/rest/voice.js';
import { VOICE_CLONE_PROVIDERS, isTerminalCloneStatus } from '../../src/types/voice.js';
import type { Emotion, DeliveryStyle, VoiceDescriptor } from '../../src/types/config.js';

// =============================================================================
// A. VoiceDescriptor -> tts_config.voice_descriptor on the wire
// =============================================================================

describe('voice_descriptor — OUT (tts_config.voice_descriptor object)', () => {
  it('serializes a VoiceDescriptor into tts_config.voice_descriptor (snake_case)', () => {
    const vd: VoiceDescriptor = {
      gender: 'female',
      locale: 'en-US',
      age: 'young',
      style: 'warm',
      nameHint: 'asteria',
    };
    const wire = JSON.parse(
      serializeMessage(
        createConfigMessage(undefined, { provider: 'deepgram', voiceDescriptor: vd })
      )
    );
    const tts = wire.tts_config as Record<string, unknown>;
    const vdWire = tts.voice_descriptor as Record<string, unknown>;
    expect(vdWire).toBeDefined();
    expect(vdWire.gender).toBe('female');
    expect(vdWire.locale).toBe('en-US');
    expect(vdWire.age).toBe('young');
    expect(vdWire.style).toBe('warm');
    // camelCase nameHint maps to snake_case name_hint on the wire.
    expect(vdWire.name_hint).toBe('asteria');
    expect('nameHint' in vdWire).toBe(false);
  });

  it('omits voice_descriptor when not set', () => {
    const wire = JSON.parse(
      serializeMessage(createConfigMessage(undefined, { provider: 'deepgram', voiceId: 'v1' }))
    );
    const tts = wire.tts_config as Record<string, unknown>;
    expect(tts.voice_descriptor).toBeUndefined();
    // A raw voice_id still wins / is present.
    expect(tts.voice_id).toBe('v1');
  });

  it('emits only the set descriptor fields', () => {
    const wire = JSON.parse(
      serializeMessage(
        createConfigMessage(undefined, {
          provider: 'cartesia',
          voiceDescriptor: { gender: 'male', accent: 'british' },
        })
      )
    );
    const vdWire = (wire.tts_config as Record<string, unknown>).voice_descriptor as Record<
      string,
      unknown
    >;
    expect(vdWire).toEqual({ gender: 'male', accent: 'british' });
  });
});

// =============================================================================
// B. voices.clone builds the canonical POST body + cloneAndWait
// =============================================================================

describe('cloneVoice — canonical JSON body (gateway VoiceCloneRequest)', () => {
  const mockFetch = vi.fn();

  beforeEach(() => {
    mockFetch.mockReset();
    vi.stubGlobal('fetch', mockFetch);
  });

  it('POSTs JSON with audio_samples + mode (not multipart audio_N)', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () =>
        Promise.resolve({
          voice_id: 'voice_abc123',
          name: 'My Voice',
          provider: 'elevenlabs',
          status: 'ready',
          created_at: '2026-06-17T00:00:00Z',
        }),
    });

    const buf = new Uint8Array([0, 1, 2, 3]).buffer;
    const result = await cloneVoice('http://localhost:3001', {
      name: 'My Voice',
      provider: 'elevenlabs',
      audioFiles: [buf],
      audioSamples: ['https://example.com/sample.wav'],
      mode: 'instant',
      description: 'A warm voice',
    });

    expect(result.voiceId).toBe('voice_abc123');
    expect(result.status).toBe('ready');

    const [url, init] = mockFetch.mock.calls[0];
    expect(url).toBe('http://localhost:3001/voices/clone');
    expect(init.method).toBe('POST');
    expect(init.headers['Content-Type']).toBe('application/json');
    const body = JSON.parse(init.body as string);
    expect(body.provider).toBe('elevenlabs');
    expect(body.name).toBe('My Voice');
    expect(body.mode).toBe('instant');
    expect(body.description).toBe('A warm voice');
    // URL sample passes through; ArrayBuffer is base64-encoded; both under audio_samples.
    expect(body.audio_samples).toContain('https://example.com/sample.wav');
    expect(body.audio_samples).toContain(btoa(String.fromCharCode(0, 1, 2, 3)));
    // The canonical field name (NOT the old broken multipart `audio_0`).
    expect('audio_0' in body).toBe(false);
  });

  it('cloneAndWait resolves immediately for an instant ready clone', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () =>
        Promise.resolve({ voice_id: 'v1', name: 'V', provider: 'cartesia', status: 'ready' }),
    });
    const result = await cloneVoiceAndWait('http://localhost:3001', {
      name: 'V',
      provider: 'cartesia',
      audioFiles: [new Uint8Array([1]).buffer],
    });
    expect(result.voiceId).toBe('v1');
  });

  it('cloneAndWait rejects for a still-running professional clone (no poll endpoint)', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () =>
        Promise.resolve({ voice_id: 'pvc1', name: 'Pro', provider: 'elevenlabs', status: 'queued' }),
    });
    await expect(
      cloneVoiceAndWait('http://localhost:3001', {
        name: 'Pro',
        provider: 'elevenlabs',
        audioFiles: [new Uint8Array([1]).buffer],
        mode: 'professional',
      })
    ).rejects.toThrow(/queued/i);
  });
});

describe('voice-clone value space (mirror of gateway enums)', () => {
  it('widened to the gateway 7 providers', () => {
    expect([...VOICE_CLONE_PROVIDERS]).toEqual([
      'hume',
      'elevenlabs',
      'lmnt',
      'cartesia',
      'playht',
      'speechify',
      'resemble',
    ]);
  });

  it('canonical clone status terminality', () => {
    expect(isTerminalCloneStatus('ready')).toBe(true);
    expect(isTerminalCloneStatus('failed')).toBe(true);
    expect(isTerminalCloneStatus('queued')).toBe(false);
    expect(isTerminalCloneStatus('training')).toBe(false);
    expect(isTerminalCloneStatus('verifying')).toBe(false);
  });
});

// =============================================================================
// C. Widened Emotion + DeliveryStyle value space (typed help)
// =============================================================================

describe('emotion / delivery-style value space', () => {
  it('accepts the 22 new P4 emotion variants as typed values', () => {
    // Compile-time assignability + runtime presence (TS unions erase, so we assert
    // the values are accepted on the typed config wire path).
    const newEmotions: Emotion[] = [
      'amazed',
      'amused',
      'affectionate',
      'intrigued',
      'flirtatious',
      'frustrated',
      'annoyed',
      'determined',
      'reassuring',
      'sympathetic',
      'nostalgic',
      'serene',
      'terrified',
      'ecstatic',
      'skeptical',
      'relieved',
      'panicked',
      'concerned',
      'apologetic',
      'resigned',
      'wistful',
      'contemplative',
    ];
    expect(newEmotions).toHaveLength(22);
    // Each serializes onto tts_config.emotion verbatim.
    for (const emotion of newEmotions) {
      const wire = JSON.parse(
        serializeMessage(createConfigMessage(undefined, { provider: 'hume', emotion }))
      );
      expect((wire.tts_config as Record<string, unknown>).emotion).toBe(emotion);
    }
  });

  it('keeps old emotion variants + accepts a raw string (forward-compat)', () => {
    const kept: Emotion[] = ['neutral', 'happy', 'sad', 'angry', 'sarcastic', 'bored'];
    const raw: Emotion = 'some_future_emotion'; // (string & {}) forward-compat branch
    const wire = JSON.parse(
      serializeMessage(
        createConfigMessage(undefined, { provider: 'hume', emotion: raw, emotionDescription: 'warm' })
      )
    );
    expect((wire.tts_config as Record<string, unknown>).emotion).toBe('some_future_emotion');
    expect((wire.tts_config as Record<string, unknown>).emotion_description).toBe('warm');
    expect(kept).toHaveLength(6);
  });

  it('accepts the new scenario DeliveryStyles', () => {
    const scenarios: DeliveryStyle[] = [
      'newscast',
      'customer_service',
      'assistant',
      'chat',
      'advertisement_upbeat',
      'sports_commentary',
      'documentary_narration',
      'narration_professional',
      'narration_relaxed',
      'poetry_reading',
      'lyrical',
      'gentle',
    ];
    for (const deliveryStyle of scenarios) {
      const wire = JSON.parse(
        serializeMessage(createConfigMessage(undefined, { provider: 'microsoft-azure', deliveryStyle }))
      );
      expect((wire.tts_config as Record<string, unknown>).delivery_style).toBe(deliveryStyle);
    }
  });
});
