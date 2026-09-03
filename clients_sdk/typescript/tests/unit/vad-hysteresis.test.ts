/**
 * D3b VAD hysteresis tests.
 *
 * The old VAD used ONE energy threshold for both speech-start and speech-end, so
 * a noisy room hovering at that level mis-segmented (rapid start/stop flapping).
 * D3b uses a higher `startThresholdDb` to ENTER speech and a lower
 * `stopThresholdDb` to LEAVE it, plus a `minVolume` floor. These tests set
 * smoothingFactor=0 so the smoothed energy equals the instantaneous RMS and the
 * threshold logic is deterministic per frame.
 *
 * A constant-amplitude A frame has RMS=A and dB = 20*log10(A):
 *   A=0.1 -> -20dB ; A=0.0316 -> -30dB ; A=0.01 -> -40dB ; A=0.001 -> -60dB.
 */
import { describe, it, expect } from 'vitest';
import { VAD } from '../../src/audio/vad.js';

/** Build a constant-amplitude frame of the requested dBFS. */
function frameAtDb(db: number, n = 480): Float32Array {
  const amp = Math.pow(10, db / 20);
  const f = new Float32Array(n);
  f.fill(amp);
  return f;
}

describe('D3b VAD hysteresis — config derivation', () => {
  it('derives start/stop rails from a single energyThresholdDb with the default gap', () => {
    const vad = new VAD({ energyThresholdDb: -35, hysteresisDb: 6 });
    const h = vad.getHysteresis();
    expect(h.startDb).toBe(-35);
    expect(h.stopDb).toBe(-41);
    expect(vad.getThreshold()).toBe(-35); // getThreshold returns the start rail
  });

  it('accepts explicit start/stop rails and clamps an inverted pair', () => {
    const vad = new VAD({ startThresholdDb: -25, stopThresholdDb: -40 });
    expect(vad.getHysteresis()).toEqual({ startDb: -25, stopDb: -40 });
    const inverted = new VAD({ startThresholdDb: -40, stopThresholdDb: -20 });
    // stop must not exceed start -> clamped to start.
    expect(inverted.getHysteresis()).toEqual({ startDb: -40, stopDb: -40 });
  });

  it('setThreshold re-derives both rails keeping the anti-flap gap', () => {
    const vad = new VAD({ hysteresisDb: 8 });
    vad.setThreshold(-30);
    expect(vad.getHysteresis()).toEqual({ startDb: -30, stopDb: -38 });
  });
});

describe('D3b VAD hysteresis — anti-flap behaviour', () => {
  it('a level BETWEEN the rails does not START speech from silence', () => {
    const vad = new VAD({
      startThresholdDb: -25,
      stopThresholdDb: -35,
      smoothingFactor: 0,
      minSpeechDurationMs: 0, // confirm instantly so timing isn't a factor
    });
    // -30dB is below the START rail (-25) -> stays silence.
    for (let i = 0; i < 10; i++) vad.processFrame(frameAtDb(-30));
    expect(vad.getState()).toBe('silence');
    expect(vad.isSpeaking()).toBe(false);
  });

  it('a level above START enters speech; then a level between the rails STAYS in speech', () => {
    const vad = new VAD({
      startThresholdDb: -25,
      stopThresholdDb: -35,
      smoothingFactor: 0,
      minSpeechDurationMs: 0,
      minSilenceDurationMs: 0,
    });
    // Loud frame (-20dB > start -25) -> enters speech (min duration 0).
    vad.processFrame(frameAtDb(-20));
    vad.processFrame(frameAtDb(-20));
    expect(vad.getState()).toBe('speech');

    // Now drop to -30dB: BELOW start (-25) but ABOVE stop (-35). With a single
    // threshold this would flap to silence; with hysteresis it STAYS speech.
    for (let i = 0; i < 10; i++) vad.processFrame(frameAtDb(-30));
    expect(vad.getState()).toBe('speech');
  });

  it('only a level below the STOP rail ends speech', () => {
    const vad = new VAD({
      startThresholdDb: -25,
      stopThresholdDb: -35,
      smoothingFactor: 0,
      minSpeechDurationMs: 0,
      minSilenceDurationMs: 0,
    });
    vad.processFrame(frameAtDb(-20));
    vad.processFrame(frameAtDb(-20));
    expect(vad.getState()).toBe('speech');
    // -40dB is below the STOP rail (-35) -> ends speech.
    vad.processFrame(frameAtDb(-40));
    vad.processFrame(frameAtDb(-40));
    expect(vad.getState()).toBe('silence');
  });

  it('minVolume floor hard-gates a sub-floor frame to never-speech', () => {
    const vad = new VAD({
      startThresholdDb: -60, // very permissive dB rail
      stopThresholdDb: -70,
      minVolume: 0.05, // but require RMS >= 0.05 (~-26dB) to count as speech
      smoothingFactor: 0,
      minSpeechDurationMs: 0,
    });
    // -40dB -> RMS 0.01 < floor 0.05 -> never speech despite passing the dB rail.
    for (let i = 0; i < 10; i++) vad.processFrame(frameAtDb(-40));
    expect(vad.getState()).toBe('silence');
    // -20dB -> RMS 0.1 >= floor -> speech.
    vad.processFrame(frameAtDb(-20));
    vad.processFrame(frameAtDb(-20));
    expect(vad.getState()).toBe('speech');
  });

  it('back-compat: a lone energyThresholdDb still detects speech', () => {
    const vad = new VAD({ energyThresholdDb: -35, smoothingFactor: 0, minSpeechDurationMs: 0 });
    vad.processFrame(frameAtDb(-20));
    vad.processFrame(frameAtDb(-20));
    expect(vad.isSpeaking()).toBe(true);
  });
});
