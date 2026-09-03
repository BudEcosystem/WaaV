/**
 * Tier-1 config-surface tests:
 *  - D2 gateway audio knobs (audio_out_chunk_ms / client_playback_rate) serialize
 *    from the camelCase TTSConfig fields into the exact wire keys, and BudTTS /
 *    BudTalk DEFAULT them when they own a player (so the scheduled player does no
 *    client resampling and barge-in truncates within one server chunk).
 *  - D1 protocol_version grading on `ready`: a MAJOR mismatch is a typed error
 *    event; a same-major (minor) drift is a softer configWarning.
 */
import { describe, it, expect } from 'vitest';
import { serializeMessage } from '../../src/ws/messages.js';
import type { SDKConfigMessage } from '../../src/ws/messages.js';
import { BudTTS } from '../../src/pipelines/tts.js';
import { BudTalk } from '../../src/pipelines/talk.js';
import { WebSocketSession } from '../../src/ws/session.js';

function wireOf(msg: SDKConfigMessage): Record<string, any> {
  return JSON.parse(serializeMessage(msg));
}

/** The session needs a global WebSocket ctor; pipelines build one in ctor. */
class NoopWebSocket {
  static OPEN = 1;
  readyState = 0;
  binaryType = 'arraybuffer';
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: (() => void) | null = null;
  constructor(public url: string) {}
  send(): void {}
  close(): void {}
}

describe('D2 gateway audio knobs → wire', () => {
  it('serializes audio_out_chunk_ms + client_playback_rate from camelCase', () => {
    const wire = wireOf({
      type: 'config',
      tts: {
        provider: 'deepgram',
        sampleRate: 24000,
        audioOutChunkMs: 20,
        clientPlaybackRate: 48000,
      },
    });
    expect(wire.tts_config.audio_out_chunk_ms).toBe(20);
    expect(wire.tts_config.client_playback_rate).toBe(48000);
  });

  it('BudTTS defaults the knobs when autoPlay (owns a player)', () => {
    const prev = (globalThis as any).WebSocket;
    (globalThis as any).WebSocket = NoopWebSocket;
    try {
      const tts = new BudTTS({
        url: 'ws://127.0.0.1:3009/ws',
        provider: 'deepgram',
        sampleRate: 24000,
        autoPlay: true,
      });
      // Reach into the session config the pipeline built to confirm defaults.
      const cfg = (tts as any).session.config as { tts: { audioOutChunkMs?: number; clientPlaybackRate?: number } };
      expect(cfg.tts.audioOutChunkMs).toBe(20);
      expect(cfg.tts.clientPlaybackRate).toBe(24000);
    } finally {
      (globalThis as any).WebSocket = prev;
    }
  });

  it('BudTTS does NOT default the knobs when autoPlay is false', () => {
    const prev = (globalThis as any).WebSocket;
    (globalThis as any).WebSocket = NoopWebSocket;
    try {
      const tts = new BudTTS({
        url: 'ws://127.0.0.1:3009/ws',
        provider: 'deepgram',
        sampleRate: 24000,
        autoPlay: false,
      });
      const cfg = (tts as any).session.config as { tts: { audioOutChunkMs?: number; clientPlaybackRate?: number } };
      expect(cfg.tts.audioOutChunkMs).toBeUndefined();
      expect(cfg.tts.clientPlaybackRate).toBeUndefined();
    } finally {
      (globalThis as any).WebSocket = prev;
    }
  });

  it('BudTalk defaults the knobs when autoPlay (owns a player)', () => {
    const prev = (globalThis as any).WebSocket;
    (globalThis as any).WebSocket = NoopWebSocket;
    try {
      const talk = new BudTalk({
        url: 'ws://127.0.0.1:3009/ws',
        tts: { provider: 'deepgram', sampleRate: 24000 },
        autoPlay: true,
      });
      const cfg = (talk as any).session.config as { tts: { audioOutChunkMs?: number; clientPlaybackRate?: number } };
      expect(cfg.tts.audioOutChunkMs).toBe(20);
      expect(cfg.tts.clientPlaybackRate).toBe(24000);
    } finally {
      (globalThis as any).WebSocket = prev;
    }
  });

  it('an explicit user knob wins over the default', () => {
    const prev = (globalThis as any).WebSocket;
    (globalThis as any).WebSocket = NoopWebSocket;
    try {
      const tts = new BudTTS({
        url: 'ws://127.0.0.1:3009/ws',
        provider: 'deepgram',
        sampleRate: 24000,
        autoPlay: true,
        audioOutChunkMs: 40, // explicit user override
      });
      const cfg = (tts as any).session.config as { tts: { audioOutChunkMs?: number } };
      // The user value flows through; default does not clobber it.
      expect(cfg.tts.audioOutChunkMs).toBe(40);
    } finally {
      (globalThis as any).WebSocket = prev;
    }
  });
});

describe('D1 protocol_version grading on ready', () => {
  function makeSession() {
    let ws: any;
    const Impl = class {
      static OPEN = 1;
      readyState = 0;
      binaryType = 'arraybuffer';
      onopen: (() => void) | null = null;
      onclose: ((e: { code: number; reason: string }) => void) | null = null;
      onerror: (() => void) | null = null;
      onmessage: ((e: { data: unknown }) => void) | null = null;
      constructor(public url: string) {
        ws = this;
        setTimeout(() => {
          this.readyState = 1;
          this.onopen?.();
        }, 0);
      }
      send(): void {}
      close(code = 1000, reason = ''): void {
        this.readyState = 3;
        this.onclose?.({ code, reason });
      }
      emit(obj: unknown): void {
        this.onmessage?.({ data: JSON.stringify(obj) });
      }
    };
    const session = new WebSocketSession({
      url: 'ws://127.0.0.1:3009/ws',
      WebSocket: Impl as unknown as typeof WebSocket,
      autoConfig: false,
      reconnect: false,
      liveness: false,
    });
    return { session, getWs: () => ws };
  }

  it('emits a typed error on a MAJOR protocol_version mismatch', async () => {
    const { session, getWs } = makeSession();
    const errors: string[] = [];
    const warnings: string[] = [];
    session.on('error', (e) => errors.push(e.code));
    session.on('configWarning', (w) => warnings.push(w.code));

    await session.connect();
    getWs().emit({ type: 'ready', protocol_version: '2.0', stream_id: 's1' });

    expect(errors).toContain('PROTOCOL_VERSION_MISMATCH');
    expect(warnings).not.toContain('PROTOCOL_VERSION_MINOR_DRIFT');
    await session.disconnect();
  });

  it('emits a configWarning (not error) on a same-major minor drift', async () => {
    const { session, getWs } = makeSession();
    const errors: string[] = [];
    const warnings: string[] = [];
    session.on('error', (e) => errors.push(e.code));
    session.on('configWarning', (w) => warnings.push(w.code));

    await session.connect();
    // SDK PROTOCOL_VERSION is "1.0"; "1.5" is a same-major minor drift.
    getWs().emit({ type: 'ready', protocol_version: '1.5', stream_id: 's1' });

    expect(warnings).toContain('PROTOCOL_VERSION_MINOR_DRIFT');
    expect(errors).not.toContain('PROTOCOL_VERSION_MISMATCH');
    await session.disconnect();
  });

  it('no event when protocol_version matches exactly', async () => {
    const { session, getWs } = makeSession();
    const errors: string[] = [];
    const warnings: string[] = [];
    session.on('error', (e) => errors.push(e.code));
    session.on('configWarning', (w) => warnings.push(w.code));

    await session.connect();
    getWs().emit({ type: 'ready', protocol_version: '1.0', stream_id: 's1' });

    expect(errors).toHaveLength(0);
    expect(warnings).toHaveLength(0);
    await session.disconnect();
  });
});
