/**
 * AudioWorklet capture processor (D3) — runs on the audio render thread.
 *
 * Replaces the deprecated, main-thread ScriptProcessorNode capture. It:
 *  - receives 128-sample render quanta at the AudioContext's native rate,
 *  - resamples to the target STT rate (default 16k) with linear interpolation
 *    (same math as the SDK's main-thread AudioProcessor.resample, so the two
 *    capture paths are byte-compatible),
 *  - accumulates exactly 20ms frames (320 samples @16k),
 *  - converts Float32 -> Int16 IN the worklet, and
 *  - postMessages the Int16 frame to the main thread as a TRANSFERABLE
 *    ArrayBuffer (zero-copy). Buffers are taken from a small pool that the main
 *    thread returns after sending, so process() does NO per-frame allocation
 *    (the SOTA "minimize allocations in process()" rule) once warmed.
 *
 * This file has its own global scope (AudioWorkletGlobalScope) and CANNOT import
 * from the main bundle. It is built as a standalone module (see rollup config:
 * `dist/capture.worklet.js`) and loaded via `audioWorklet.addModule(url)`.
 *
 * Messages:
 *   main -> worklet: { type:'config', frameMs?, targetSampleRate? }
 *                    { type:'recycle', buffer: ArrayBuffer }   // return a pool buffer
 *   worklet -> main: { type:'frame', buffer: ArrayBuffer, samples: number } (transfer)
 */

// --- Minimal AudioWorklet ambient declarations (this scope is not the DOM) ---
/* eslint-disable @typescript-eslint/no-explicit-any */
declare const sampleRate: number;
declare function registerProcessor(name: string, ctor: unknown): void;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
  constructor();
  process(
    inputs: Float32Array[][],
    outputs: Float32Array[][],
    parameters: Record<string, Float32Array>
  ): boolean;
}

interface CaptureConfigMessage {
  type: 'config';
  /** Output frame length in ms (default 20). */
  frameMs?: number;
  /** Target sample rate for the emitted Int16 frames (default 16000). */
  targetSampleRate?: number;
}
interface RecycleMessage {
  type: 'recycle';
  buffer: ArrayBuffer;
}
type InboundMessage = CaptureConfigMessage | RecycleMessage;

class CaptureProcessor extends AudioWorkletProcessor {
  /** Target output rate (Hz). */
  private targetRate = 16000;
  /** Output frame length in samples at the target rate. */
  private frameSamples = 320; // 20ms @ 16k
  /** Native-rate ring of pending input samples awaiting resample. */
  private pending: Float32Array = new Float32Array(0);
  /** Fractional read position into `pending` (carried across quanta). */
  private readPos = 0;
  /** Accumulated, resampled output samples for the current frame. */
  private outAccum: Float32Array;
  private outLen = 0;
  /** Pool of reusable Int16 ArrayBuffers returned by the main thread. */
  private pool: ArrayBuffer[] = [];

  constructor() {
    super();
    this.outAccum = new Float32Array(this.frameSamples);
    this.port.onmessage = (e: MessageEvent<InboundMessage>) => this.onMessage(e.data);
  }

  private onMessage(msg: InboundMessage): void {
    if (msg.type === 'config') {
      if (typeof msg.targetSampleRate === 'number' && msg.targetSampleRate > 0) {
        this.targetRate = msg.targetSampleRate;
      }
      const frameMs = typeof msg.frameMs === 'number' && msg.frameMs > 0 ? msg.frameMs : 20;
      this.frameSamples = Math.max(1, Math.round((this.targetRate * frameMs) / 1000));
      this.outAccum = new Float32Array(this.frameSamples);
      this.outLen = 0;
    } else if (msg.type === 'recycle' && msg.buffer instanceof ArrayBuffer) {
      // Cap the pool so a stalled main thread can't make it grow unbounded.
      if (this.pool.length < 8) this.pool.push(msg.buffer);
    }
  }

  /** Get a 2*frameSamples-byte ArrayBuffer (Int16) from the pool or allocate. */
  private takeBuffer(): ArrayBuffer {
    const need = this.frameSamples * 2;
    let buf = this.pool.pop();
    while (buf && buf.byteLength !== need) {
      buf = this.pool.pop(); // discard stale-sized buffers (rate/frame changed)
    }
    return buf ?? new ArrayBuffer(need);
  }

  process(inputs: Float32Array[][]): boolean {
    const input = inputs[0];
    const channel = input && input[0];
    // No input this quantum (e.g. track muted): keep the processor alive.
    if (!channel || channel.length === 0) return true;

    // Append this quantum to the native-rate pending buffer.
    this.appendPending(channel);

    const ratio = sampleRate / this.targetRate; // native samples per output sample
    // Drain as many output samples as the pending buffer can supply.
    // Linear interpolation between pending[floor] and pending[floor+1].
    while (this.readPos + 1 < this.pending.length) {
      const idx = Math.floor(this.readPos);
      const frac = this.readPos - idx;
      const s0 = this.pending[idx]!;
      const s1 = this.pending[idx + 1]!;
      this.outAccum[this.outLen++] = s0 * (1 - frac) + s1 * frac;
      this.readPos += ratio;

      if (this.outLen >= this.frameSamples) {
        this.emitFrame();
      }
    }

    // Compact the pending buffer: drop the fully-consumed prefix, keep the tail
    // (plus the 1 sample the next interpolation needs). This bounds memory and
    // preserves cross-quantum continuity (no boundary click).
    this.compactPending();
    return true;
  }

  private appendPending(quantum: Float32Array): void {
    if (this.pending.length === 0) {
      this.pending = quantum.slice();
      return;
    }
    const merged = new Float32Array(this.pending.length + quantum.length);
    merged.set(this.pending, 0);
    merged.set(quantum, this.pending.length);
    this.pending = merged;
  }

  private compactPending(): void {
    const consumed = Math.floor(this.readPos);
    if (consumed <= 0) return;
    // Keep one sample before readPos so interpolation continuity holds.
    const keepFrom = Math.max(0, consumed);
    this.pending = this.pending.slice(keepFrom);
    this.readPos -= keepFrom;
  }

  private emitFrame(): void {
    const buffer = this.takeBuffer();
    const view = new Int16Array(buffer);
    const n = this.frameSamples;
    for (let i = 0; i < n; i++) {
      // Clamp then scale (matches AudioProcessor.float32ToInt16 exactly).
      let s = this.outAccum[i]!;
      if (s > 1) s = 1;
      else if (s < -1) s = -1;
      view[i] = s < 0 ? s * 32768 : s * 32767;
    }
    this.outLen = 0;
    // Transfer the buffer to the main thread (zero-copy). The main thread sends
    // it back via a 'recycle' message to keep process() allocation-free.
    this.port.postMessage({ type: 'frame', buffer, samples: n }, [buffer]);
  }
}

registerProcessor('waav-capture', CaptureProcessor);

// Marked as a module so bundlers treat it independently; no runtime export.
export {};
