/**
 * D9: adaptive jitter buffer feeding the D2 scheduled-playout player.
 *
 * The Tier-1 player (player.ts) already schedules each PCM chunk against a
 * monotonic AudioContext clock, which absorbs the *rate-mismatch* class of jitter
 * (and the gateway resamples downlink to `client_playback_rate` with continuous
 * filter state, so the player does ZERO downlink resampling — see audio-knobs).
 * What the scheduled player CANNOT absorb is NETWORK ARRIVAL VARIANCE: when chunks
 * arrive bursty/late, the player underruns (playhead pins to currentTime, leaving
 * an audible gap) on every late arrival.
 *
 * This buffer sits BETWEEN the websocket `onAudio` callback and `player.play()`.
 * It is default-on but *bypasses itself on a clean network* (zero added latency):
 * it only starts holding chunks once it has actually MEASURED arrival jitter above
 * a small threshold, then releases them on a fixed cadence so the player sees an
 * even arrival stream. The target hold depth is sized from the OBSERVED inter-
 * arrival variance (NetEq-style), clamped to a small ceiling, so a clean link
 * keeps ~0ms added latency and a jittery link gets just enough cushion.
 *
 * IMPORTANT — no double-resample / no double-buffer:
 *  - It NEVER touches sample data: chunks pass through as opaque ArrayBuffers, so
 *    the gateway's client_playback_rate resampling is the only resample stage.
 *  - It does NOT re-implement the playout clock; it only RE-TIMES delivery into
 *    the existing scheduled player, so the two do not double-buffer the same
 *    variance (the player handles rate, this handles arrival variance).
 *
 * Loss tolerance: the gateway gives no per-frame delivery guarantee for
 * DroppableAudio, so this buffer simply forwards whatever it has on each release
 * tick and never blocks waiting for a missing frame.
 */

export type JitterBufferOptions = {
  /**
   * Below this measured inter-arrival jitter (ms), STAY in passthrough (release
   * each chunk immediately) so a clean network adds zero latency. Default 8ms.
   */
  jitterActivateMs?: number;
  /** Minimum buffered depth once active, in ms of audio. Default 40ms. */
  minDepthMs?: number;
  /** Hard ceiling on buffered depth, in ms of audio (bounds added latency). Default 120ms. */
  maxDepthMs?: number;
  /** Release cadence (ms) once active — how often a held chunk is handed to the player. */
  releaseIntervalMs?: number;
  /** EWMA smoothing for the jitter estimate (0..1, higher = smoother). Default 0.85. */
  smoothing?: number;
  /** Injectable clock for tests (defaults to performance.now). */
  now?: () => number;
  /** Injectable timer (defaults to setInterval); must return a clearable handle. */
  setIntervalImpl?: (cb: () => void, ms: number) => ReturnType<typeof setInterval>;
  clearIntervalImpl?: (h: ReturnType<typeof setInterval>) => void;
};

/** A queued chunk plus the audio duration it represents (for depth accounting). */
type Held = { data: ArrayBuffer; durationMs: number };

export class JitterBuffer {
  private readonly jitterActivateMs: number;
  private readonly minDepthMs: number;
  private readonly maxDepthMs: number;
  private readonly releaseIntervalMs: number;
  private readonly smoothing: number;
  private readonly now: () => number;
  private readonly setIntervalImpl: (cb: () => void, ms: number) => ReturnType<typeof setInterval>;
  private readonly clearIntervalImpl: (h: ReturnType<typeof setInterval>) => void;

  /** Bytes/sec of the inbound PCM (16-bit mono @ sinkRate) for depth<->ms math. */
  private bytesPerMs: number;

  private sink: (data: ArrayBuffer) => void;

  private queue: Held[] = [];
  private heldMs = 0;

  /** Whether we are currently re-timing (true) or passing through (false). */
  private active = false;
  /**
   * True once the cushion has filled to the target at least once; we then drain
   * one chunk per release tick until empty (NetEq "prime then drain"). Cleared
   * when the queue empties so a refill must re-prime before draining again — this
   * is what gives the player slack to ride out the NEXT late arrival.
   */
  private primed = false;
  private timer: ReturnType<typeof setInterval> | null = null;

  // --- arrival-jitter estimation ---
  private lastArrival: number | null = null;
  /** Expected inter-arrival gap (ms): the chunk duration we keep seeing. */
  private expectedGapMs = 0;
  /** EWMA of |actual gap - expected gap| — the jitter estimate. */
  private jitterMs = 0;

  /**
   * @param sampleRate the PCM sample rate of the inbound chunks (the player sink
   *        rate, since the gateway already resampled to client_playback_rate).
   * @param channels   channel count (default 1).
   * @param sink       where released chunks go (the player's play()).
   * @param options    tuning + injectables.
   */
  constructor(
    sampleRate: number,
    channels: number,
    sink: (data: ArrayBuffer) => void,
    options: JitterBufferOptions = {}
  ) {
    const rate = sampleRate > 0 ? sampleRate : 24000;
    const ch = channels > 0 ? channels : 1;
    // 16-bit PCM => 2 bytes/sample/channel.
    this.bytesPerMs = (rate * ch * 2) / 1000;

    this.sink = sink;
    this.jitterActivateMs = options.jitterActivateMs ?? 8;
    this.minDepthMs = options.minDepthMs ?? 40;
    this.maxDepthMs = options.maxDepthMs ?? 120;
    this.releaseIntervalMs = options.releaseIntervalMs ?? 20;
    this.smoothing = options.smoothing ?? 0.85;
    this.now = options.now ?? (() => performance.now());
    this.setIntervalImpl =
      options.setIntervalImpl ??
      ((cb, ms) => {
        const h = setInterval(cb, ms);
        // Don't keep the event loop / page alive on the release timer alone
        // (Node Timeout has .unref(); browser returns a bare number — guard it).
        const t = h as unknown as { unref?: () => void };
        if (typeof t.unref === 'function') t.unref();
        return h;
      });
    this.clearIntervalImpl = options.clearIntervalImpl ?? ((h) => clearInterval(h));
  }

  /** ms of audio a chunk represents, from its byte length. */
  private durationOf(byteLength: number): number {
    return this.bytesPerMs > 0 ? byteLength / this.bytesPerMs : 0;
  }

  /**
   * Feed one inbound chunk. On a clean link this forwards immediately (passthrough);
   * once measured jitter crosses the activation threshold it switches to re-timed
   * release so the player sees an even stream.
   */
  push(data: ArrayBuffer): void {
    // Empty/keepalive frames carry no audio: never queue them, never let them
    // perturb the arrival-jitter estimate. Forward straight through (the player
    // already no-ops a <2-byte chunk).
    if (!data || data.byteLength < 2) {
      this.sink(data);
      return;
    }

    const durationMs = this.durationOf(data.byteLength);
    this.observeArrival(durationMs);

    if (!this.active) {
      // Clean network: hand straight to the player — zero added latency.
      this.sink(data);
      return;
    }

    // Re-timing: hold it; the release tick drains on cadence. Guard the ceiling so
    // a sustained burst can never grow unbounded latency (shed oldest, like the
    // gateway's DroppableAudio discipline — the player is loss-tolerant).
    this.queue.push({ data, durationMs });
    this.heldMs += durationMs;
    while (this.heldMs > this.maxDepthMs && this.queue.length > 1) {
      const dropped = this.queue.shift();
      if (dropped) this.heldMs -= dropped.durationMs;
    }
  }

  /**
   * Update the inter-arrival jitter estimate and flip active on/off. `expectedGap`
   * tracks the chunk duration (the cadence the gateway emits at); jitter is the
   * EWMA deviation of the real gap from that expectation.
   */
  private observeArrival(durationMs: number): void {
    const t = this.now();
    // Track the expected cadence as the (smoothed) chunk duration we keep seeing.
    this.expectedGapMs =
      this.expectedGapMs === 0
        ? durationMs
        : this.smoothing * this.expectedGapMs + (1 - this.smoothing) * durationMs;

    if (this.lastArrival !== null) {
      const gap = t - this.lastArrival;
      const deviation = Math.abs(gap - this.expectedGapMs);
      this.jitterMs = this.smoothing * this.jitterMs + (1 - this.smoothing) * deviation;
    }
    this.lastArrival = t;

    if (!this.active) {
      if (this.jitterMs >= this.jitterActivateMs) {
        this.activate();
      }
    } else {
      // Hysteresis: only fall back to passthrough once jitter is comfortably under
      // half the activation threshold AND the queue has drained, so we don't flap.
      if (this.jitterMs < this.jitterActivateMs * 0.5 && this.queue.length === 0) {
        this.deactivate();
      }
    }
  }

  /** Target hold depth (ms): cushion ~2x the measured jitter, clamped. */
  private targetDepthMs(): number {
    const sized = this.minDepthMs + 2 * this.jitterMs;
    return Math.min(this.maxDepthMs, Math.max(this.minDepthMs, sized));
  }

  private activate(): void {
    if (this.active) return;
    this.active = true;
    if (this.timer === null) {
      this.timer = this.setIntervalImpl(() => this.release(), this.releaseIntervalMs);
    }
  }

  private deactivate(): void {
    if (!this.active) return;
    this.active = false;
    this.stopTimer();
    this.flush();
  }

  /**
   * Release tick: once we have built up to the target cushion, hand exactly one
   * chunk per tick to the player (the player re-schedules it on its own clock).
   * Never blocks on a missing frame; just waits for the cushion to fill.
   */
  private release(): void {
    if (this.queue.length === 0) {
      // Drained: require a re-prime (refill to target) before the next drain so
      // the player keeps a cushion against the following late arrival.
      this.primed = false;
      return;
    }
    // Prime: wait until the cushion has filled to target (or the chunk-count guard
    // trips, so a tiny expectedGap can't stall forever). Once primed, drain one
    // chunk per tick until empty.
    if (!this.primed) {
      if (this.heldMs >= this.targetDepthMs() || this.queue.length >= this.maxChunksGuard()) {
        this.primed = true;
      } else {
        return;
      }
    }
    const item = this.queue.shift();
    if (item) {
      this.heldMs -= item.durationMs;
      this.sink(item.data);
    }
    if (this.queue.length === 0) this.primed = false;
  }

  /** Absolute chunk-count guard so a tiny expectedGap can't stall release forever. */
  private maxChunksGuard(): number {
    const gap = this.expectedGapMs > 0 ? this.expectedGapMs : this.releaseIntervalMs;
    return Math.max(2, Math.ceil(this.maxDepthMs / gap));
  }

  /** Immediately hand everything queued to the player (drain in arrival order). */
  private flush(): void {
    while (this.queue.length > 0) {
      const item = this.queue.shift();
      if (item) {
        this.heldMs -= item.durationMs;
        this.sink(item.data);
      }
    }
    this.heldMs = 0;
  }

  private stopTimer(): void {
    if (this.timer !== null) {
      this.clearIntervalImpl(this.timer);
      this.timer = null;
    }
  }

  /**
   * Barge-in / stop: drop everything WITHOUT forwarding (the player is also being
   * stopped, so flushing here would re-queue audio the user just cut). Resets the
   * jitter estimate so the next utterance starts fresh in passthrough.
   */
  reset(): void {
    this.stopTimer();
    this.queue = [];
    this.heldMs = 0;
    this.active = false;
    this.primed = false;
    this.lastArrival = null;
    this.expectedGapMs = 0;
    this.jitterMs = 0;
  }

  /** Tear down (on disconnect): stop the timer; do not forward residual audio. */
  close(): void {
    this.reset();
  }

  // --- introspection (tests / metrics) ---
  /** True while re-timing; false while passing through (clean network). */
  get isActive(): boolean {
    return this.active;
  }
  /** Current measured inter-arrival jitter estimate (ms). */
  get jitterEstimateMs(): number {
    return this.jitterMs;
  }
  /** Currently buffered audio depth (ms). */
  get bufferedMs(): number {
    return this.heldMs;
  }
}
