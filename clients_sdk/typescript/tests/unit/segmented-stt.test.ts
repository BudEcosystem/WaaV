/**
 * Segmented speech-to-text wire contract (gateway docs/segmented-stt/customer-contract-reference.md).
 *
 * Covers the SDK side of the contract:
 *   - `stt_config.transcription_mode` (auto | streaming | segmented) on the config wire;
 *   - `ready.stt`, the open object saying what speech-to-text the session got;
 *   - the new `vad_event` and `stt_warning` messages;
 *   - the additive `code` / `recoverable` / `details` fields on `error`.
 *
 * Session tests drive a WebSocketSession through a mock WebSocket, so each frame goes through the
 * real wire parser (deserializeMessage) before the session dispatches it.
 */
import { describe, it, expect } from 'vitest';
import { serializeMessage, deserializeMessage } from '../../src/ws/messages.js';
import type { SDKConfigMessage } from '../../src/ws/messages.js';
import type {
  ErrorMessage,
  ReadyMessage,
  SttWarningMessage,
  VadEventMessage,
} from '../../src/types/messages.js';
import { WebSocketSession } from '../../src/ws/session.js';
import type {
  ConfigWarningEvent,
  ReadyEvent,
  SessionErrorEvent,
  SessionVadEvent,
  SttWarningEvent,
} from '../../src/ws/events.js';

function wireOf(msg: SDKConfigMessage): Record<string, any> {
  return JSON.parse(serializeMessage(msg));
}

/** The `ready.stt` example from the contract reference, verbatim. */
const READY_STT = {
  provider: 'elevenlabs',
  model: 'scribe_v2',
  model_source: 'deployment',
  transcription_mode: 'segmented',
  requested_mode: 'auto',
  requested_mode_source: 'default',
  interim_results: 'per_segment',
  endpointing: 'gateway',
  speech_events: 'detector',
  barge_in_ms: 500,
  detector: 'silero',
  confidence_source: 'none',
  latency_class: 'slow',
  final_latency_typical_ms: null,
  final_latency_slow_ms: 2010,
  final_latency_slow_percentile: 99,
  latency_basis: 'seed',
  final_deadline_ms: 6000,
  lifecycle: 'ga',
  capability_source: 'exact',
  streaming_alternatives: ['scribe_v2_realtime'],
  map_version: '2026-10-01.1',
  notices: [{ code: 'stt_language_unset', message: 'Set language: detection on short segments is unreliable.' }],
};

// ============================================================================
// Config → wire
// ============================================================================

describe('stt_config.transcription_mode on the wire', () => {
  it.each(['auto', 'streaming', 'segmented'] as const)('serializes transcriptionMode=%s', (mode) => {
    const wire = wireOf({
      type: 'config',
      stt: { provider: 'elevenlabs', language: 'en', model: 'scribe_v2', transcriptionMode: mode },
    });
    expect(wire.stt_config.transcription_mode).toBe(mode);
    // snake_case only on the wire; the camelCase SDK name never leaks.
    expect(wire.stt_config.transcriptionMode).toBeUndefined();
  });

  it('omits transcription_mode when unset (absent = auto, no new wire surface)', () => {
    const wire = wireOf({ type: 'config', stt: { provider: 'deepgram', language: 'en-US', model: 'nova-3' } });
    expect('transcription_mode' in wire.stt_config).toBe(false);
  });
});

// ============================================================================
// Wire → message parsing
// ============================================================================

describe('ready.stt parsing', () => {
  it('keeps the whole stt object (known keys, nulls and notices)', () => {
    const msg = deserializeMessage(
      JSON.stringify({ type: 'ready', protocol_version: '1.0', stream_id: 's-1', stt: READY_STT })
    ) as ReadyMessage;
    expect(msg.stt).toEqual(READY_STT);
    expect(msg.stt?.transcription_mode).toBe('segmented');
    expect(msg.stt?.final_latency_typical_ms).toBeNull();
    expect(msg.stt?.notices?.[0]?.code).toBe('stt_language_unset');
  });

  it('keeps keys the SDK does not know yet (the object is open)', () => {
    const msg = deserializeMessage(
      JSON.stringify({ type: 'ready', stream_id: 's-1', stt: { provider: 'openai', future_field: { a: 1 } } })
    ) as ReadyMessage;
    expect(msg.stt?.provider).toBe('openai');
    expect(msg.stt?.future_field).toEqual({ a: 1 });
  });

  it('has no stt when the rollout does not cover the session', () => {
    const msg = deserializeMessage(JSON.stringify({ type: 'ready', stream_id: 's-1' })) as ReadyMessage;
    expect(msg.stt).toBeUndefined();
    expect('stt' in msg).toBe(false);
  });
});

describe('vad_event parsing', () => {
  it('parses a turn_closed event exactly as the gateway sends it', () => {
    const msg = deserializeMessage(
      JSON.stringify({ type: 'vad_event', event: 'turn_closed', turn_id: 7, had_transcript: false, reason: 'no_speech' })
    ) as VadEventMessage;
    expect(msg).toEqual({
      type: 'vad_event',
      event: 'turn_closed',
      turn_id: 7,
      had_transcript: false,
      reason: 'no_speech',
    });
  });

  it('parses the optional audio_ms / sustained_ms / discarded fields', () => {
    const start = deserializeMessage(
      JSON.stringify({ type: 'vad_event', event: 'turn_start', turn_id: 3, audio_ms: 1840, sustained_ms: 500 })
    ) as VadEventMessage;
    expect(start.audio_ms).toBe(1840);
    expect(start.sustained_ms).toBe(500);

    const end = deserializeMessage(
      JSON.stringify({ type: 'vad_event', event: 'speech_end', turn_id: 3, audio_ms: 2900, discarded: true })
    ) as VadEventMessage;
    expect(end.discarded).toBe(true);
    // Fields the frame did not carry stay absent.
    expect('had_transcript' in end).toBe(false);
    expect('reason' in end).toBe(false);
  });
});

describe('stt_warning parsing', () => {
  it('parses {type, code, message, detail}', () => {
    const detail = { turn_id: 4, segment_seq: 2, voiced_ms: 1300, text_offset: 12, result: 'timeout', class: 'transient', retries: 1 };
    const msg = deserializeMessage(
      JSON.stringify({ type: 'stt_warning', code: 'stt_segment_failed', message: 'A segment was lost.', detail })
    ) as SttWarningMessage;
    expect(msg).toEqual({ type: 'stt_warning', code: 'stt_segment_failed', message: 'A segment was lost.', detail });
  });

  it('parses an stt_warning without detail', () => {
    const msg = deserializeMessage(
      JSON.stringify({ type: 'stt_warning', code: 'stt_degraded', message: 'Three turns in a row were lost.' })
    ) as SttWarningMessage;
    expect(msg.code).toBe('stt_degraded');
    expect('detail' in msg).toBe(false);
  });
});

describe('error parsing (additive code / recoverable / details)', () => {
  it('parses a coded, recoverable setup refusal', () => {
    const details = {
      provider: 'openai',
      model: 'gpt-live-transcribe',
      reason: 'client_not_implemented',
      streaming_alternatives: ['gpt-4o-transcribe'],
    };
    const msg = deserializeMessage(
      JSON.stringify({
        type: 'error',
        message: 'stt_live_unsupported: no live path for openai/gpt-live-transcribe',
        code: 'stt_live_unsupported',
        recoverable: true,
        details,
      })
    ) as ErrorMessage;
    expect(msg.code).toBe('stt_live_unsupported');
    expect(msg.recoverable).toBe(true);
    expect(msg.details).toEqual(details);
    expect(msg.message).toContain('stt_live_unsupported: ');
  });

  it('keeps an uncoded error in its old shape (no invented code)', () => {
    const msg = deserializeMessage(JSON.stringify({ type: 'error', message: 'boom' })) as ErrorMessage;
    expect(msg).toEqual({ type: 'error', message: 'boom' });
    expect('code' in msg).toBe(false);
    expect('recoverable' in msg).toBe(false);
    expect('details' in msg).toBe(false);
  });
});

// ============================================================================
// Session events
// ============================================================================

/** Minimal browser-WebSocket-shaped mock that lets a test push frames in. */
class MockWebSocket {
  static OPEN = 1;
  static CONNECTING = 0;
  static CLOSING = 2;
  static CLOSED = 3;
  OPEN = MockWebSocket.OPEN;
  readyState = MockWebSocket.CONNECTING;
  binaryType = 'arraybuffer';
  onopen: (() => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  sent: unknown[] = [];

  constructor(public url: string) {
    setTimeout(() => {
      this.readyState = MockWebSocket.OPEN;
      this.onopen?.();
    }, 0);
  }
  send(data: unknown): void {
    this.sent.push(data);
  }
  close(code = 1000, reason = ''): void {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.({ code, reason });
  }
  emit(obj: unknown): void {
    this.onmessage?.({ data: JSON.stringify(obj) });
  }
}

async function connectedSession(): Promise<{ session: WebSocketSession; ws: MockWebSocket }> {
  let ws!: MockWebSocket;
  const Impl = class extends MockWebSocket {
    constructor(url: string) {
      super(url);
      ws = this;
    }
  };
  const session = new WebSocketSession({
    url: 'ws://127.0.0.1:3009/ws',
    WebSocket: Impl as unknown as typeof WebSocket,
    autoConfig: false,
    reconnect: false,
  });
  await session.connect();
  return { session, ws };
}

describe('WebSocketSession ready.stt', () => {
  it('exposes the stt object on the ready event', async () => {
    const { session, ws } = await connectedSession();
    const events: ReadyEvent[] = [];
    session.on('ready', (e) => events.push(e));

    ws.emit({ type: 'ready', protocol_version: '1.0', stream_id: 's-1', stt: READY_STT });

    expect(events).toHaveLength(1);
    expect(events[0]!.stt).toEqual(READY_STT);
    expect(events[0]!.stt?.interim_results).toBe('per_segment');
    expect(events[0]!.raw.stt).toEqual(READY_STT);
    await session.disconnect();
  });

  it('leaves stt unset when the gateway sends none', async () => {
    const { session, ws } = await connectedSession();
    const events: ReadyEvent[] = [];
    session.on('ready', (e) => events.push(e));

    ws.emit({ type: 'ready', protocol_version: '1.0', stream_id: 's-1' });

    expect(events[0]!.stt).toBeUndefined();
    await session.disconnect();
  });
});

describe('WebSocketSession vadEvent', () => {
  it('emits a typed vadEvent with camelCase fields and the raw frame', async () => {
    const { session, ws } = await connectedSession();
    const events: SessionVadEvent[] = [];
    const errors: SessionErrorEvent[] = [];
    session.on('vadEvent', (e) => events.push(e));
    session.on('error', (e) => errors.push(e));

    ws.emit({ type: 'vad_event', event: 'turn_start', turn_id: 2, audio_ms: 900, sustained_ms: 500 });
    ws.emit({ type: 'vad_event', event: 'speech_end', turn_id: 2, audio_ms: 2100, discarded: false });
    ws.emit({ type: 'vad_event', event: 'turn_closed', turn_id: 2, had_transcript: false, reason: 'transcription_failed' });

    expect(events.map((e) => e.event)).toEqual(['turn_start', 'speech_end', 'turn_closed']);
    expect(events[0]).toMatchObject({ turnId: 2, audioMs: 900, sustainedMs: 500 });
    expect(events[1]).toMatchObject({ turnId: 2, audioMs: 2100, discarded: false });
    expect(events[2]).toMatchObject({ turnId: 2, hadTranscript: false, reason: 'transcription_failed' });
    expect(events[2]!.raw).toEqual({
      type: 'vad_event',
      event: 'turn_closed',
      turn_id: 2,
      had_transcript: false,
      reason: 'transcription_failed',
    });
    // Absent optional fields are not set on the event.
    expect('hadTranscript' in events[0]!).toBe(false);
    expect(errors).toHaveLength(0);
    await session.disconnect();
  });
});

describe('WebSocketSession sttWarning', () => {
  it('emits a typed sttWarning and NEVER an error (it is not a disconnect)', async () => {
    const { session, ws } = await connectedSession();
    const warnings: SttWarningEvent[] = [];
    const configWarnings: ConfigWarningEvent[] = [];
    const errors: SessionErrorEvent[] = [];
    session.on('sttWarning', (e) => warnings.push(e));
    session.on('configWarning', (e) => configWarnings.push(e));
    session.on('error', (e) => errors.push(e));

    ws.emit({
      type: 'stt_warning',
      code: 'stt_segment_failed',
      message: 'A segment was lost after its retry.',
      detail: { turn_id: 5, segment_seq: 1 },
    });
    ws.emit({ type: 'stt_warning', code: 'stt_rate_limited', message: 'Uploads are being limited.' });

    expect(warnings).toHaveLength(2);
    expect(warnings[0]).toMatchObject({
      code: 'stt_segment_failed',
      message: 'A segment was lost after its retry.',
      detail: { turn_id: 5, segment_seq: 1 },
    });
    expect(warnings[0]!.raw.type).toBe('stt_warning');
    expect('detail' in warnings[1]!).toBe(false);
    expect(errors).toHaveLength(0);
    // A mid-call stt_warning is not a config advisory either.
    expect(configWarnings).toHaveLength(0);
    await session.disconnect();
  });

  it('counts stt warnings in the session metrics', async () => {
    const { session, ws } = await connectedSession();
    // @ts-expect-error read the private collector the session increments
    const metrics = session.metrics as { getCounter(name: string): number };
    const before = metrics.getCounter('ws.sttWarnings');
    ws.emit({ type: 'stt_warning', code: 'stt_audio_dropped', message: 'Audio was dropped.', detail: { bytes: 3200 } });
    expect(metrics.getCounter('ws.sttWarnings')).toBe(before + 1);
    await session.disconnect();
  });
});

describe('WebSocketSession coded error', () => {
  it('carries code / recoverable / details on the error event', async () => {
    const { session, ws } = await connectedSession();
    const errors: SessionErrorEvent[] = [];
    session.on('error', (e) => errors.push(e));

    ws.emit({
      type: 'error',
      message: 'stt_live_unsupported: no live path',
      code: 'stt_live_unsupported',
      recoverable: true,
      details: { provider: 'openai', model: 'gpt-live-transcribe', reason: 'client_not_implemented' },
    });

    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatchObject({
      code: 'stt_live_unsupported',
      recoverable: true,
      details: { provider: 'openai', model: 'gpt-live-transcribe', reason: 'client_not_implemented' },
    });
    await session.disconnect();
  });

  it('an uncoded error falls back to code UNKNOWN, not recoverable', async () => {
    const { session, ws } = await connectedSession();
    const errors: SessionErrorEvent[] = [];
    session.on('error', (e) => errors.push(e));

    ws.emit({ type: 'error', message: 'Provider failed' });

    expect(errors).toHaveLength(1);
    expect(errors[0]!.code).toBe('UNKNOWN');
    expect(errors[0]!.recoverable).toBe(false);
    expect(errors[0]!.details).toBeUndefined();
    await session.disconnect();
  });
});
