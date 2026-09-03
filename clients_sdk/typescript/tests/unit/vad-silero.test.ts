/**
 * D3b opt-in ONNX Silero VAD scaffold tests.
 *
 * Silero is strictly opt-in and dependency-free by default (onnxruntime-web is
 * NOT bundled). These tests verify the seam: it is unavailable without a model
 * (so the caller falls back to energy), it uses the correct 512@16k frame size,
 * it drives a provided session factory, and it periodically resets the recurrent
 * state. A fake session stands in for onnxruntime-web.
 */
import { describe, it, expect, vi } from 'vitest';
import { SileroVAD, type SileroSession } from '../../src/audio/vad-silero.js';

/** A fake ORT session that returns a fixed speech probability + echoes state. */
function fakeSession(prob: number): SileroSession & { runs: number } {
  const s = {
    runs: 0,
    async run(feeds: Record<string, { data: Float32Array | BigInt64Array; dims: number[] }>) {
      s.runs++;
      const state = feeds.state!.data as Float32Array;
      return {
        output: { data: new Float32Array([prob]) },
        stateN: { data: Float32Array.from(state) },
      };
    },
  };
  return s;
}

describe('D3b SileroVAD availability', () => {
  it('is unavailable without a modelUrl (energy fallback)', async () => {
    const vad = new SileroVAD();
    expect(await vad.init()).toBe(false);
    expect(vad.isAvailable()).toBe(false);
    expect(await vad.process(new Float32Array(512), 0)).toBeNull();
  });

  it('uses the correct frame size (512 @16k, 256 @8k)', () => {
    expect(new SileroVAD({ sampleRate: 16000 }).getFrameSamples()).toBe(512);
    expect(new SileroVAD({ sampleRate: 8000 }).getFrameSamples()).toBe(256);
  });
});

describe('D3b SileroVAD with an injected session', () => {
  it('runs the model and returns the speech probability', async () => {
    const sess = fakeSession(0.9);
    const vad = new SileroVAD({ modelUrl: 'x.onnx', createSession: async () => sess, threshold: 0.5 });
    expect(await vad.init()).toBe(true);
    const p = await vad.process(new Float32Array(512), 0);
    expect(p).toBeCloseTo(0.9, 5); // Float32 round-trip loses precision
    expect(vad.isSpeech(p!)).toBe(true);
    expect(vad.isSpeech(0.1)).toBe(false);
  });

  it('rejects a wrong-sized frame', async () => {
    const sess = fakeSession(0.9);
    const vad = new SileroVAD({ modelUrl: 'x.onnx', createSession: async () => sess });
    await vad.init();
    expect(await vad.process(new Float32Array(320), 0)).toBeNull(); // not 512
  });

  it('resets the recurrent state on the configured interval', async () => {
    const sess = fakeSession(0.5);
    const resetSpy = vi.spyOn(SileroVAD.prototype, 'resetState');
    const vad = new SileroVAD({ modelUrl: 'x.onnx', createSession: async () => sess, resetIntervalMs: 5000 });
    await vad.init();
    await vad.process(new Float32Array(512), 0);
    await vad.process(new Float32Array(512), 1000); // < interval -> no reset
    const before = resetSpy.mock.calls.length;
    await vad.process(new Float32Array(512), 6000); // >= interval -> reset
    expect(resetSpy.mock.calls.length).toBeGreaterThan(before);
    resetSpy.mockRestore();
  });

  it('disables itself if the session throws at runtime (energy fallback)', async () => {
    const sess: SileroSession = { async run() { throw new Error('ort boom'); } };
    const vad = new SileroVAD({ modelUrl: 'x.onnx', createSession: async () => sess });
    await vad.init();
    expect(await vad.process(new Float32Array(512), 0)).toBeNull();
    expect(vad.isAvailable()).toBe(false);
  });
});
