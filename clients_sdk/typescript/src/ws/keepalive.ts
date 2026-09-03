/**
 * STT keepalive-silence (D6).
 *
 * The gateway's auto-pong keeps the client<->gateway SOCKET open during a lull,
 * but it does NOT keep the upstream PROVIDER STT stream warm: many streaming STT
 * providers close (or wind down) an idle audio stream after a few seconds of
 * silence, so the first word after a conversational pause pays a fresh
 * stream-warmup penalty (or is lost). Pipecat solves this with a periodic
 * keepalive frame (stt_service.py:45 — 100ms of zero-PCM every ~5s of no real
 * audio); the SDK mirrors that here.
 *
 * The pump is GATED so it never fights real audio:
 *  - `noteRealAudio()` is called on every genuine uplink frame and resets the
 *    idle clock, so keepalive only fires during a true silence gap.
 *  - it only emits when the socket is connected (`isConnected`), the session is
 *    an audio session (the host only constructs it when `audio !== false`), and
 *    at least `idleMs` has elapsed since the last real frame OR the last
 *    keepalive — so at most one ~100ms zero frame per `idleMs`.
 *
 * Pure timers + injected hooks (no socket/DOM), so it is unit-testable with fake
 * timers. One instance per session; started on `ready`, stopped on close.
 */

/** Tunable thresholds for the STT keepalive-silence pump. */
export interface KeepaliveConfig {
  /**
   * Idle gap with no real audio after which one keepalive frame is sent, ms
   * (default 5000). Pipecat uses ~5s; well under a typical provider idle-close.
   */
  idleMs?: number;
  /**
   * Duration of each zero-PCM keepalive frame, ms (default 100). Long enough to
   * count as a real audio packet to the upstream, short enough to be negligible.
   */
  frameMs?: number;
  /** Sample rate of the zero-PCM frame, Hz (default 16000 — STT ingress rate). */
  sampleRate?: number;
  /** Bytes per sample (default 2 — linear16). */
  bytesPerSample?: number;
}

/** Resolved keepalive config. */
export type ResolvedKeepaliveConfig = Required<KeepaliveConfig>;

export const DEFAULT_KEEPALIVE_CONFIG: ResolvedKeepaliveConfig = {
  idleMs: 5000,
  frameMs: 100,
  sampleRate: 16000,
  bytesPerSample: 2,
};

/** Hooks the keepalive pump needs from its host (the session). */
export interface KeepaliveHooks {
  /** Send one raw audio frame to the socket. Returns true if accepted. */
  sendAudio: (frame: ArrayBuffer) => boolean;
  /** True iff the socket is currently connected/open. */
  isConnected: () => boolean;
  /** Monotonic clock in ms (injectable for tests). Defaults to performance.now()/Date.now(). */
  now?: () => number;
}

/** A monotonic clock that never goes backwards (immune to wall-clock jumps). */
function defaultNow(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}

/**
 * Periodically sends a short zero-PCM frame during silence so the gateway's
 * upstream STT stream stays warm, without ever overlapping real audio.
 */
export class KeepaliveSilence {
  private readonly config: ResolvedKeepaliveConfig;
  private readonly hooks: KeepaliveHooks;
  private readonly now: () => number;
  /** The pre-built zero-PCM frame (allocated once, reused every tick). */
  private readonly silenceFrame: ArrayBuffer;
  private timer: ReturnType<typeof setInterval> | null = null;
  /** Monotonic time of the last real OR keepalive frame; the idle clock. */
  private lastActivity: number;
  private sentCount = 0;

  constructor(hooks: KeepaliveHooks, config: KeepaliveConfig = {}) {
    this.config = { ...DEFAULT_KEEPALIVE_CONFIG, ...config };
    this.hooks = hooks;
    this.now = hooks.now ?? defaultNow;
    const samples = Math.max(1, Math.round((this.config.sampleRate * this.config.frameMs) / 1000));
    // Zero-filled ArrayBuffer == digital silence (linear16 all-zero). Reused
    // every tick so the pump allocates nothing on the steady path.
    this.silenceFrame = new ArrayBuffer(samples * this.config.bytesPerSample);
    this.lastActivity = this.now();
  }

  /** Total keepalive frames sent (diagnostics/tests). */
  getSentCount(): number {
    return this.sentCount;
  }

  /** The size of one keepalive frame in bytes (diagnostics/tests). */
  frameBytes(): number {
    return this.silenceFrame.byteLength;
  }

  /**
   * Start the pump. Polls at a fraction of `idleMs` so the keepalive fires
   * within ~idleMs of the last activity (not idleMs + a full coarse period).
   * Idempotent.
   */
  start(): void {
    if (this.timer) return;
    this.lastActivity = this.now();
    const poll = Math.max(250, Math.floor(this.config.idleMs / 4));
    this.timer = setInterval(() => this.tick(), poll);
  }

  /** Stop the pump. Idempotent. Call on socket close/teardown. */
  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  /**
   * Record that a REAL audio frame was just sent. Resets the idle clock so the
   * keepalive never fires while genuine audio is flowing (the gate that keeps it
   * from fighting real audio).
   */
  noteRealAudio(): void {
    this.lastActivity = this.now();
  }

  /**
   * One pump tick: if the socket is connected and we've been idle for at least
   * `idleMs`, send exactly one zero-PCM frame and re-arm the idle clock (so the
   * next frame is at most one more `idleMs` away — never a burst).
   */
  private tick(): void {
    if (!this.hooks.isConnected()) {
      // Don't advance the clock while disconnected; a fresh `ready` restarts us.
      return;
    }
    if (this.now() - this.lastActivity < this.config.idleMs) {
      return;
    }
    const sent = this.hooks.sendAudio(this.silenceFrame.slice(0));
    if (sent) {
      this.sentCount++;
    }
    // Re-arm regardless of send success: on a failed send we still back off a
    // full idleMs rather than hammering a struggling socket every poll.
    this.lastActivity = this.now();
  }
}
