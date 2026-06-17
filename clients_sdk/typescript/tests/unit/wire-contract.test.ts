/**
 * Wire-contract round-trip tests (P0).
 *
 * These exercise the SDK's OWN serializer/deserializer against the EXACT
 * gateway wire shapes (handlers/ws/messages.rs), so they fail if the SDK ever
 * drifts from the gateway contract again. They specifically guard the bugs
 * fixed in P0:
 *   - STT transcript was read from `text` (always empty) instead of `transcript`
 *   - ready envelope dropped stream_id / protocol_version / livekit_url
 *   - JSON ping / stop / flush / interrupt were sent (gateway has no such ops)
 */
import { describe, it, expect } from 'vitest';
import {
  deserializeMessage,
  serializeMessage,
  createClearMessage,
  createAudioEndMessage,
  createSendMessageMessage,
} from '../../src/ws/messages.js';
import { PROTOCOL_VERSION } from '../../src/types/messages.js';
import type { STTResultMessage, ReadyMessage, TTSAudioMessage } from '../../src/types/messages.js';

describe('STT result wire mapping (transcript, NOT text)', () => {
  it('surfaces the transcript from the exact gateway wire {transcript:"hello world"}', () => {
    // EXACT gateway wire for OutgoingMessage::STTResult.
    const wire = JSON.stringify({
      type: 'stt_result',
      transcript: 'hello world',
      is_final: true,
      is_speech_final: true,
      confidence: 0.97,
    });

    const msg = deserializeMessage(wire) as STTResultMessage;

    expect(msg.type).toBe('stt_result');
    // THE BUG: this used to be '' because the SDK read wire.text.
    expect(msg.transcript).toBe('hello world');
    expect(msg.transcript).not.toBe('');
    expect(msg.is_final).toBe(true);
    expect(msg.is_speech_final).toBe(true);
    expect(msg.confidence).toBeCloseTo(0.97);
  });

  it('surfaces segment_transcript when present', () => {
    const wire = JSON.stringify({
      type: 'stt_result',
      transcript: '',
      is_final: true,
      is_speech_final: true,
      confidence: 0.9,
      segment_transcript: 'the full accumulated segment',
    });
    const msg = deserializeMessage(wire) as STTResultMessage;
    expect(msg.segment_transcript).toBe('the full accumulated segment');
  });

  it('does NOT read a legacy `text` field (proves the fix, not a fallback)', () => {
    const wire = JSON.stringify({
      type: 'stt_result',
      text: 'legacy-should-be-ignored',
      transcript: 'correct-value',
      is_final: false,
      is_speech_final: false,
      confidence: 0.5,
    });
    const msg = deserializeMessage(wire) as STTResultMessage;
    expect(msg.transcript).toBe('correct-value');
  });

  it('surfaces the uniform translations[] array (P5) from the stt_result frame', () => {
    // EXACT gateway wire: OutgoingMessage::STTResult with translations.
    const wire = JSON.stringify({
      type: 'stt_result',
      transcript: 'hello world',
      is_final: true,
      is_speech_final: true,
      confidence: 0.95,
      translations: [
        { lang: 'es-ES', text: 'hola mundo', is_partial: false },
        { lang: 'de-DE', text: 'hallo welt' },
      ],
    });
    const msg = deserializeMessage(wire) as STTResultMessage;
    expect(msg.translations).toBeDefined();
    expect(msg.translations).toHaveLength(2);
    expect(msg.translations![0]).toEqual({ lang: 'es-ES', text: 'hola mundo', is_partial: false });
    expect(msg.translations![1]).toEqual({ lang: 'de-DE', text: 'hallo welt' });
  });

  it('omits translations on a frame without them (non-translation sessions)', () => {
    const wire = JSON.stringify({
      type: 'stt_result',
      transcript: 'hello world',
      is_final: true,
      is_speech_final: true,
      confidence: 0.95,
    });
    const msg = deserializeMessage(wire) as STTResultMessage;
    expect(msg.translations).toBeUndefined();
  });
});

describe('ready envelope wire mapping', () => {
  it('maps stream_id, protocol_version and livekit_url from the gateway ready frame', () => {
    const wire = JSON.stringify({
      type: 'ready',
      protocol_version: '1.0',
      stream_id: '550e8400-e29b-41d4-a716-446655440000',
      livekit_url: 'ws://localhost:7880',
      waav_participant_identity: 'waav-ai',
      waav_participant_name: 'WaaV AI',
    });

    const msg = deserializeMessage(wire) as ReadyMessage;

    expect(msg.type).toBe('ready');
    expect(msg.stream_id).toBe('550e8400-e29b-41d4-a716-446655440000');
    expect(msg.protocol_version).toBe('1.0');
    expect(msg.livekit_url).toBe('ws://localhost:7880');
    expect(msg.waav_participant_identity).toBe('waav-ai');
    expect(msg.waav_participant_name).toBe('WaaV AI');
  });

  it('SDK PROTOCOL_VERSION matches the gateway value', () => {
    expect(PROTOCOL_VERSION).toBe('1.0');
  });
});

describe('TTS audio wire mapping (snake_case)', () => {
  it('maps sample_rate / is_final from the gateway wire', () => {
    const wire = JSON.stringify({
      type: 'tts_audio',
      audio: 'AAAA',
      format: 'linear16',
      sample_rate: 24000,
      is_final: true,
      sequence: 3,
    });
    const msg = deserializeMessage(wire) as TTSAudioMessage;
    expect(msg.audio).toBe('AAAA');
    expect(msg.sample_rate).toBe(24000);
    expect(msg.is_final).toBe(true);
    expect(msg.sequence).toBe(3);
  });
});

describe('outgoing control messages (clear / audio_end / send_message)', () => {
  it('serializes clear() to {type:"clear"} (barge-in)', () => {
    expect(JSON.parse(serializeMessage(createClearMessage()))).toEqual({ type: 'clear' });
  });

  it('serializes audioEnd() to {type:"audio_end"} (finalize)', () => {
    expect(JSON.parse(serializeMessage(createAudioEndMessage()))).toEqual({ type: 'audio_end' });
  });

  it('serializes send_message with role + optional topic', () => {
    const wire = JSON.parse(
      serializeMessage(createSendMessageMessage('hi there', 'user', { topic: 'chat' }))
    );
    expect(wire).toEqual({ type: 'send_message', message: 'hi there', role: 'user', topic: 'chat' });
  });

  it('does NOT expose ping/stop/flush/interrupt factories (removed in P0)', async () => {
    const mod = (await import('../../src/ws/messages.js')) as Record<string, unknown>;
    expect(mod.createPingMessage).toBeUndefined();
    expect(mod.createStopMessage).toBeUndefined();
    expect(mod.createFlushMessage).toBeUndefined();
    expect(mod.createInterruptMessage).toBeUndefined();
  });
});

describe('config message livekit shape (waav_ prefix, snake_case)', () => {
  it('serializes livekit config with waav_participant_* and room_name', () => {
    const wire = JSON.parse(
      serializeMessage({
        type: 'config',
        livekit: {
          roomName: 'room-1',
          enableRecording: true,
          waavParticipantIdentity: 'waav-ai',
          waavParticipantName: 'WaaV AI',
          listenParticipants: ['user-1'],
        },
      })
    );
    expect(wire.type).toBe('config');
    expect(wire.livekit).toEqual({
      room_name: 'room-1',
      enable_recording: true,
      waav_participant_identity: 'waav-ai',
      waav_participant_name: 'WaaV AI',
      listen_participants: ['user-1'],
    });
    // No legacy sayna_ keys.
    expect(JSON.stringify(wire)).not.toContain('sayna');
  });
});
