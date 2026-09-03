/**
 * D8 WebCodecs opus codec seam tests.
 *
 * WebCodecs is absent in jsdom/node, so we inject FAKE AudioEncoder/AudioDecoder/AudioData/
 * EncodedAudioChunk constructors (exactly the runtime seam the real browser provides) and assert the
 * encoder/decoder drive them correctly: opus config, one packet per frame, decode → PCM, and an
 * encode→decode round-trip. The real browser opus round-trip is a documented manual e2e gate.
 */
import { describe, it, expect, vi } from 'vitest';
import {
  supportsWebCodecsOpus,
  OpusEncoder,
  OpusDecoder,
  DEFAULT_OPUS_BITRATE,
  type WebCodecsCtors,
} from '../../src/audio/opus.js';

/** A minimal in-memory WebCodecs fake: the encoder "compresses" to ~1/10th, the decoder synthesizes
 *  20ms of PCM at the configured rate. Records configure() calls for assertions. */
function makeFakeWebCodecs() {
  const encoderConfigs: any[] = [];
  const decoderConfigs: any[] = [];

  class FakeAudioData {
    format: string;
    sampleRate: number;
    numberOfFrames: number;
    numberOfChannels: number;
    timestamp: number;
    data: Int16Array;
    constructor(init: any) {
      this.format = init.format;
      this.sampleRate = init.sampleRate;
      this.numberOfFrames = init.numberOfFrames;
      this.numberOfChannels = init.numberOfChannels;
      this.timestamp = init.timestamp;
      this.data = init.data;
    }
    copyTo(dest: Int16Array): void {
      dest.set(this.data.subarray(0, dest.length));
    }
    close(): void {}
  }

  class FakeEncodedAudioChunk {
    type: string;
    timestamp: number;
    data: ArrayBuffer;
    byteLength: number;
    constructor(init: any) {
      this.type = init.type;
      this.timestamp = init.timestamp;
      this.data = init.data;
      this.byteLength = (init.data as ArrayBuffer).byteLength;
    }
    copyTo(buf: ArrayBuffer): void {
      new Uint8Array(buf).set(new Uint8Array(this.data));
    }
  }

  class FakeAudioEncoder {
    private output: (c: any) => void;
    private error: (e: unknown) => void;
    constructor(init: any) {
      this.output = init.output;
      this.error = init.error;
    }
    configure(c: any): void {
      encoderConfigs.push(c);
    }
    encode(audioData: FakeAudioData): void {
      // Simulate compression: ~1/10th the PCM byte size, non-empty.
      const n = Math.max(1, Math.floor(audioData.numberOfFrames / 10));
      const data = new Uint8Array(n).fill(7).buffer;
      this.output(new FakeEncodedAudioChunk({ type: 'key', timestamp: audioData.timestamp, data }));
    }
    async flush(): Promise<void> {}
    close(): void {}
  }

  class FakeAudioDecoder {
    private output: (d: any) => void;
    private error: (e: unknown) => void;
    private rate = 48000;
    constructor(init: any) {
      this.output = init.output;
      this.error = init.error;
    }
    configure(c: any): void {
      decoderConfigs.push(c);
      this.rate = c.sampleRate;
    }
    decode(): void {
      const frames = Math.round(this.rate * 0.02); // 20ms
      const pcm = new Int16Array(frames).fill(123);
      this.output(new FakeAudioData({ numberOfFrames: frames, data: pcm } as any));
    }
    async flush(): Promise<void> {}
    close(): void {}
  }

  const ctors: WebCodecsCtors = {
    AudioEncoder: FakeAudioEncoder,
    AudioDecoder: FakeAudioDecoder,
    AudioData: FakeAudioData,
    EncodedAudioChunk: FakeEncodedAudioChunk,
  };
  return { ctors, encoderConfigs, decoderConfigs };
}

describe('D8 supportsWebCodecsOpus', () => {
  it('is false when WebCodecs is absent (jsdom/node)', () => {
    expect(supportsWebCodecsOpus()).toBe(false);
  });
});

describe('D8 OpusEncoder', () => {
  it('configures opus mono at the requested rate/bitrate and emits one packet per frame', () => {
    const { ctors, encoderConfigs } = makeFakeWebCodecs();
    const packets: ArrayBuffer[] = [];
    const enc = new OpusEncoder({ sampleRate: 48000, onPacket: (p) => packets.push(p), ctors });

    expect(encoderConfigs).toHaveLength(1);
    expect(encoderConfigs[0]).toMatchObject({
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 1,
      bitrate: DEFAULT_OPUS_BITRATE,
    });

    const frame = new Int16Array(960); // 20ms @48k
    enc.encode(frame);
    enc.encode(frame);
    expect(packets).toHaveLength(2);
    for (const p of packets) {
      expect(p.byteLength).toBeGreaterThan(0);
      expect(p.byteLength).toBeLessThan(frame.length * 2); // compressed vs PCM
    }
    enc.close();
  });

  it('throws when no WebCodecs ctors are available', () => {
    expect(() => new OpusEncoder({ sampleRate: 48000, onPacket: () => {} })).toThrow(/unavailable/);
  });
});

describe('D8 OpusDecoder', () => {
  it('configures opus mono and decodes a packet to PCM', () => {
    const { ctors, decoderConfigs } = makeFakeWebCodecs();
    const pcms: Int16Array[] = [];
    const dec = new OpusDecoder({ sampleRate: 48000, onPcm: (p) => pcms.push(p), ctors });
    expect(decoderConfigs[0]).toMatchObject({ codec: 'opus', sampleRate: 48000, numberOfChannels: 1 });

    dec.decode(new Uint8Array([1, 2, 3]).buffer);
    expect(pcms).toHaveLength(1);
    expect(pcms[0]!.length).toBe(Math.round(48000 * 0.02));
    dec.close();
  });
});

describe('D8 encode→decode round-trip through the seam', () => {
  it('an encoder packet feeds a decoder and yields PCM', () => {
    const { ctors } = makeFakeWebCodecs();
    const out: Int16Array[] = [];
    const dec = new OpusDecoder({ sampleRate: 48000, onPcm: (p) => out.push(p), ctors });
    const enc = new OpusEncoder({
      sampleRate: 48000,
      onPacket: (pkt) => dec.decode(pkt), // wire encoder → decoder
      ctors,
    });
    enc.encode(new Int16Array(960));
    expect(out).toHaveLength(1);
    expect(out[0]!.length).toBeGreaterThan(0);
  });
});
