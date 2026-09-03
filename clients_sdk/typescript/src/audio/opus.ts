/**
 * D8 — browser opus codec over the WebCodecs API (optional, used only when the session negotiated
 * `opus` transport AND this runtime supports it).
 *
 * One opus packet per WS binary frame, both directions (matching the gateway). Encoding/decoding run
 * on the MAIN thread (an AudioWorkletProcessor cannot host WebCodecs); the capture worklet already
 * posts 20ms Int16 frames to the main thread, where we encode each → send; inbound binary frames are
 * decoded → fed to the player. Opus's own PLC complements the player's underrun concealment.
 *
 * WebCodecs is Chrome/Edge (and recent Firefox); Safari + jsdom lack it. So this is gated behind
 * {@link supportsWebCodecsOpus} and the gateway's `ready` echo, and the constructors are injectable
 * so the encode/decode seam is unit-tested with fakes (the real round-trip is a browser e2e gate).
 */

/* eslint-disable @typescript-eslint/no-explicit-any */

/** The subset of WebCodecs we use, structurally typed so we don't depend on lib.dom WebCodecs typings. */
export interface WebCodecsCtors {
  AudioEncoder: any;
  AudioDecoder: any;
  AudioData: any;
  EncodedAudioChunk: any;
}

/** Read the WebCodecs constructors from the global scope (browser). Returns null when unavailable. */
function globalWebCodecs(): WebCodecsCtors | null {
  const g = globalThis as any;
  if (
    typeof g.AudioEncoder === 'function' &&
    typeof g.AudioDecoder === 'function' &&
    typeof g.AudioData === 'function' &&
    typeof g.EncodedAudioChunk === 'function'
  ) {
    return {
      AudioEncoder: g.AudioEncoder,
      AudioDecoder: g.AudioDecoder,
      AudioData: g.AudioData,
      EncodedAudioChunk: g.EncodedAudioChunk,
    };
  }
  return null;
}

/**
 * True when this runtime can BOTH encode and decode opus via WebCodecs. The SDK only sets
 * `audio_in_codec`/`audio_out_codec = opus` when this is true, so it never asks the gateway for a
 * codec it can't honor client-side. (A best-effort synchronous capability check; the deeper
 * `AudioEncoder.isConfigSupported` is async and not required for the opus/48k mono baseline.)
 */
export function supportsWebCodecsOpus(): boolean {
  return globalWebCodecs() !== null;
}

/** Default opus bitrate (bits/s) — matches the gateway's 24 kbps wideband-voice default. */
export const DEFAULT_OPUS_BITRATE = 24_000;

/** Options for {@link OpusEncoder}. */
export interface OpusEncoderOptions {
  /** PCM sample rate (must be opus-valid: 8/12/16/24/48 kHz). */
  sampleRate: number;
  /** Target bitrate (default {@link DEFAULT_OPUS_BITRATE}). */
  bitrate?: number;
  /** Called with each encoded opus packet (one per submitted frame). */
  onPacket: (packet: ArrayBuffer) => void;
  /** Called on a non-fatal encoder error. */
  onError?: (error: unknown) => void;
  /** Injected WebCodecs constructors (defaults to globals); supplied by tests. */
  ctors?: WebCodecsCtors;
}

/**
 * Streaming opus encoder: feed mono Int16 PCM frames (e.g. 20ms), receive opus packets via
 * `onPacket`. Encoding is async in WebCodecs (output arrives on the encoder's callback), so packets
 * may be delivered slightly after {@link encode} returns — `onPacket` is the single sink.
 */
export class OpusEncoder {
  private encoder: any;
  private readonly sampleRate: number;
  private readonly ctors: WebCodecsCtors;
  private timestampUs = 0;

  constructor(options: OpusEncoderOptions) {
    const ctors = options.ctors ?? globalWebCodecs();
    if (!ctors) throw new Error('WebCodecs opus encoder unavailable in this runtime');
    this.ctors = ctors;
    this.sampleRate = options.sampleRate;
    this.encoder = new ctors.AudioEncoder({
      output: (chunk: any) => {
        const buf = new ArrayBuffer(chunk.byteLength);
        chunk.copyTo(buf);
        options.onPacket(buf);
      },
      error: (e: unknown) => options.onError?.(e),
    });
    this.encoder.configure({
      codec: 'opus',
      sampleRate: options.sampleRate,
      numberOfChannels: 1,
      bitrate: options.bitrate ?? DEFAULT_OPUS_BITRATE,
    });
  }

  /** Encode one mono Int16 PCM frame. The packet is delivered via `onPacket`. */
  encode(frame: Int16Array): void {
    const audioData = new this.ctors.AudioData({
      format: 's16',
      sampleRate: this.sampleRate,
      numberOfFrames: frame.length,
      numberOfChannels: 1,
      timestamp: this.timestampUs,
      data: frame,
    });
    this.timestampUs += Math.round((frame.length / this.sampleRate) * 1_000_000);
    this.encoder.encode(audioData);
    audioData.close?.();
  }

  /** Flush any pending output (await before close to not drop trailing packets). */
  async flush(): Promise<void> {
    await this.encoder.flush?.();
  }

  /** Release the encoder. */
  close(): void {
    try {
      this.encoder.close?.();
    } catch {
      /* already closed */
    }
  }
}

/** Options for {@link OpusDecoder}. */
export interface OpusDecoderOptions {
  /** Output PCM sample rate (opus-valid; matches the gateway's `client_playback_rate`). */
  sampleRate: number;
  /** Called with each decoded mono Int16 PCM frame. */
  onPcm: (pcm: Int16Array) => void;
  /** Called on a non-fatal decoder error. */
  onError?: (error: unknown) => void;
  /** Injected WebCodecs constructors (defaults to globals); supplied by tests. */
  ctors?: WebCodecsCtors;
}

/**
 * Streaming opus decoder: feed opus packets (one per inbound binary frame), receive mono Int16 PCM
 * via `onPcm`. Like the encoder, WebCodecs delivers output asynchronously on the decoder callback.
 */
export class OpusDecoder {
  private decoder: any;
  private readonly ctors: WebCodecsCtors;
  private timestampUs = 0;

  constructor(options: OpusDecoderOptions) {
    const ctors = options.ctors ?? globalWebCodecs();
    if (!ctors) throw new Error('WebCodecs opus decoder unavailable in this runtime');
    this.ctors = ctors;
    this.decoder = new ctors.AudioDecoder({
      output: (audioData: any) => {
        const frames = audioData.numberOfFrames as number;
        const out = new Int16Array(frames);
        audioData.copyTo(out, { planeIndex: 0, format: 's16' });
        options.onPcm(out);
        audioData.close?.();
      },
      error: (e: unknown) => options.onError?.(e),
    });
    this.decoder.configure({ codec: 'opus', sampleRate: options.sampleRate, numberOfChannels: 1 });
  }

  /** Decode one opus packet. The PCM is delivered via `onPcm`. */
  decode(packet: ArrayBuffer): void {
    const chunk = new this.ctors.EncodedAudioChunk({
      type: 'key',
      timestamp: this.timestampUs,
      data: packet,
    });
    this.timestampUs += 20_000; // ~20ms/frame; opus is timestamp-tolerant for playout
    this.decoder.decode(chunk);
  }

  /** Flush any pending output. */
  async flush(): Promise<void> {
    await this.decoder.flush?.();
  }

  /** Release the decoder. */
  close(): void {
    try {
      this.decoder.close?.();
    } catch {
      /* already closed */
    }
  }
}
