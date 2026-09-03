/**
 * D10 mic-silence watchdog + getUserMedia-constraints tests.
 *
 * Mocks the Web Audio + getUserMedia surface so the AudioRecorder can run in
 * Node. Asserts: getUserMedia requests echoCancellation/noiseSuppression/
 * autoGainControl=true by default (and honours overrides); and the mic-silence
 * watchdog fires `micSilent` after the configured below-threshold window and
 * `micActive` on recovery.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { AudioRecorder } from '../../src/audio/recorder.js';

/** A controllable mock AnalyserNode whose frequency data the test sets. */
class MockAnalyser {
  fftSize = 256;
  frequencyBinCount = 128;
  private level = 0; // 0..255 average byte the watchdog reads
  connect = vi.fn();
  disconnect = vi.fn();
  setLevel(byteAvg: number) {
    this.level = byteAvg;
  }
  getByteFrequencyData(arr: Uint8Array): void {
    arr.fill(this.level);
  }
}

const lastAnalyser = { node: null as MockAnalyser | null };

class MockSource {
  connect = vi.fn();
  disconnect = vi.fn();
}
class MockGain {
  gain = { value: 1 };
  connect = vi.fn();
  disconnect = vi.fn();
}
class MockAudioContext {
  state = 'running';
  sampleRate = 48000;
  destination = {};
  createMediaStreamSource = () => new MockSource();
  createAnalyser = () => {
    const a = new MockAnalyser();
    lastAnalyser.node = a;
    return a;
  };
  createScriptProcessor = () => ({ connect: vi.fn(), disconnect: vi.fn(), onaudioprocess: null });
  createGain = () => new MockGain();
  async resume() {}
  async close() {}
}

/** Capture the constraints getUserMedia was called with. */
const lastConstraints = { value: null as MediaStreamConstraints | null };

function installWebAudioMocks() {
  lastAnalyser.node = null;
  lastConstraints.value = null;
  vi.stubGlobal('AudioContext', MockAudioContext);
  // No AudioWorkletNode -> recorder uses the ScriptProcessor fallback path.
  vi.stubGlobal('AudioWorkletNode', undefined);
  // `navigator` is a getter-only global in modern Node; stubGlobal handles it.
  vi.stubGlobal('navigator', {
    mediaDevices: {
      getUserMedia: vi.fn(async (c: MediaStreamConstraints) => {
        lastConstraints.value = c;
        return {
          getAudioTracks: () => [{ onended: null }],
          getTracks: () => [{ stop: vi.fn() }],
        } as unknown as MediaStream;
      }),
    },
  });
}

describe('D10 getUserMedia constraints', () => {
  beforeEach(installWebAudioMocks);
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('requests AEC/NS/AGC = true by default', async () => {
    const rec = new AudioRecorder();
    await rec.start();
    const audio = lastConstraints.value!.audio as MediaTrackConstraints;
    expect(audio.echoCancellation).toBe(true);
    expect(audio.noiseSuppression).toBe(true);
    expect(audio.autoGainControl).toBe(true);
    await rec.dispose();
  });

  it('honours explicit AEC/NS/AGC overrides', async () => {
    const rec = new AudioRecorder({ echoCancellation: false, noiseSuppression: false, autoGainControl: false });
    await rec.start();
    const audio = lastConstraints.value!.audio as MediaTrackConstraints;
    expect(audio.echoCancellation).toBe(false);
    expect(audio.noiseSuppression).toBe(false);
    expect(audio.autoGainControl).toBe(false);
    await rec.dispose();
  });
});

describe('D10 mic-silence watchdog', () => {
  let clock = 0;
  beforeEach(() => {
    installWebAudioMocks();
    clock = 0;
    vi.useFakeTimers();
    // The watchdog reads performance.now(); back it with the fake clock.
    vi.spyOn(performance, 'now').mockImplementation(() => clock);
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  // Advance in 50ms slices so the injected `performance.now` clock moves in
  // lockstep with the 100ms level-check interval (each poll sees the elapsed
  // time, not a single end-of-window jump).
  function advance(ms: number) {
    const step = 50;
    for (let done = 0; done < ms; done += step) {
      const slice = Math.min(step, ms - done);
      clock += slice;
      vi.advanceTimersByTime(slice);
    }
  }

  it('fires micSilent after the below-threshold window, then micActive on recovery', async () => {
    const onMicSilent = vi.fn();
    const onMicActive = vi.fn();
    const rec = new AudioRecorder({ micSilenceThreshold: 0.01, micSilenceMs: 500 });
    rec.setHandlers({ onMicSilent, onMicActive });
    await rec.start();

    // Analyser reports silence (byte avg 0 -> level 0).
    lastAnalyser.node!.setLevel(0);
    advance(600); // > 500ms of silence (level-check polls every 100ms)
    expect(onMicSilent).toHaveBeenCalledTimes(1);
    expect(onMicSilent.mock.calls[0]![0]).toBeGreaterThanOrEqual(500);

    // Audio returns (byte avg well above threshold*255).
    lastAnalyser.node!.setLevel(200);
    advance(200);
    expect(onMicActive).toHaveBeenCalledTimes(1);

    await rec.dispose();
  });

  it('does not fire micSilent while audio is present', async () => {
    const onMicSilent = vi.fn();
    const rec = new AudioRecorder({ micSilenceThreshold: 0.01, micSilenceMs: 500 });
    rec.setHandlers({ onMicSilent });
    await rec.start();

    lastAnalyser.node!.setLevel(150); // above threshold
    advance(2000);
    expect(onMicSilent).not.toHaveBeenCalled();
    await rec.dispose();
  });

  it('fires micSilent only once per silence episode', async () => {
    const onMicSilent = vi.fn();
    const rec = new AudioRecorder({ micSilenceThreshold: 0.01, micSilenceMs: 300 });
    rec.setHandlers({ onMicSilent });
    await rec.start();
    lastAnalyser.node!.setLevel(0);
    advance(2000); // stays silent the whole time
    expect(onMicSilent).toHaveBeenCalledTimes(1);
    await rec.dispose();
  });
});
