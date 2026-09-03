/**
 * D6 STT keepalive-silence tests.
 *
 * Drives the pump with an injectable clock + fake timers so the idle gate is
 * deterministic. Asserts: a zero-PCM frame is sent after an idle gap; real audio
 * resets the idle clock so keepalive NEVER overlaps genuine audio; it stays
 * silent while disconnected; and each frame is the configured size of zeroes.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { KeepaliveSilence } from '../../src/ws/keepalive.js';

class FakeSocket {
  connected = true;
  sent: ArrayBuffer[] = [];
  sendAudio = (f: ArrayBuffer): boolean => {
    if (!this.connected) return false;
    this.sent.push(f);
    return true;
  };
}

function allZero(buf: ArrayBuffer): boolean {
  const v = new Uint8Array(buf);
  for (let i = 0; i < v.length; i++) if (v[i] !== 0) return false;
  return true;
}

/** A monotonic clock kept in lockstep with vitest's fake timers. */
let clock = 0;
const now = () => clock;
/** Advance both the fake timer queue and the injected monotonic clock. */
function advance(ms: number): void {
  clock += ms;
  vi.advanceTimersByTime(ms);
}

describe('D6 keepalive-silence', () => {
  beforeEach(() => {
    clock = 0;
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it('sends a zero-PCM frame after the idle gap', () => {
    const sock = new FakeSocket();
    const ka = new KeepaliveSilence(
      { sendAudio: sock.sendAudio, isConnected: () => sock.connected, now },
      { idleMs: 5000, frameMs: 100, sampleRate: 16000 },
    );
    ka.start();
    advance(5200); // past one idle window
    expect(ka.getSentCount()).toBe(1);
    expect(sock.sent.length).toBe(1);
    expect(allZero(sock.sent[0]!)).toBe(true);
    // 100ms @ 16k mono linear16 = 1600 samples * 2 bytes = 3200 bytes.
    expect(sock.sent[0]!.byteLength).toBe(3200);
    ka.stop();
  });

  it('real audio resets the idle clock so keepalive never overlaps it', () => {
    const sock = new FakeSocket();
    const ka = new KeepaliveSilence(
      { sendAudio: sock.sendAudio, isConnected: () => sock.connected, now },
      { idleMs: 5000, frameMs: 100 },
    );
    ka.start();
    // Keep "speaking": note real audio every 2s, well under the 5s idle gap.
    for (let i = 0; i < 5; i++) {
      advance(2000);
      ka.noteRealAudio();
    }
    // Across 10s of continuous real audio, NO keepalive frame was sent.
    expect(ka.getSentCount()).toBe(0);
    ka.stop();
  });

  it('emits at most one frame per idle window (no burst)', () => {
    const sock = new FakeSocket();
    const ka = new KeepaliveSilence(
      { sendAudio: sock.sendAudio, isConnected: () => sock.connected, now },
      { idleMs: 5000, frameMs: 100 },
    );
    ka.start();
    // Advance in fine steps across ~3 idle windows so the pump can fire once per
    // window (not a burst proportional to the poll rate).
    for (let i = 0; i < 160; i++) advance(100);
    expect(ka.getSentCount()).toBeGreaterThanOrEqual(2);
    expect(ka.getSentCount()).toBeLessThanOrEqual(4);
    ka.stop();
  });

  it('does not send while disconnected', () => {
    const sock = new FakeSocket();
    sock.connected = false;
    const ka = new KeepaliveSilence(
      { sendAudio: sock.sendAudio, isConnected: () => sock.connected, now },
      { idleMs: 5000, frameMs: 100 },
    );
    ka.start();
    for (let i = 0; i < 200; i++) advance(100);
    expect(ka.getSentCount()).toBe(0);
    ka.stop();
  });

  it('stop() halts the pump', () => {
    const sock = new FakeSocket();
    const ka = new KeepaliveSilence(
      { sendAudio: sock.sendAudio, isConnected: () => sock.connected, now },
      { idleMs: 5000, frameMs: 100 },
    );
    ka.start();
    ka.stop();
    for (let i = 0; i < 200; i++) advance(100);
    expect(ka.getSentCount()).toBe(0);
  });
});
