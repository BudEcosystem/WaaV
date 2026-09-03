/**
 * Adaptive jitter buffer + PLC (D9), feeding the D2 scheduled player.
 *
 * The D2 player already plays steadily-arriving chunks gap-free with a playout
 * clock, so on a CLEAN network this buffer adds NOTHING — it passes each chunk
 * straight through (zero added latency). It only engages when it MEASURES
 * arrival jitter, at which point it:
 *
 *  - holds a SMALL adaptive buffer (target sized from observed inter-arrival
 *    variance, clamped ~40..120ms) so a late frame has slack to arrive;
 *  - REORDERS out-of-order frames by sequence number (when the source provides
 *    one) or by arrival order;
 *  - CONCEALS a missing/late frame with packet-loss concealment: a short
 *    extrapolation of the last frame, faded toward silence (repeat-with-fade) so
 *    a gap is masked instead of clicking;
 *  - TIME-STRETCHES without pitch change to drain when over-buffered — it drops a
 *    short run of samples at a zero-crossing (so no click), shrinking latency
 *    back toward target instead of letting it pile up.
 *
 * It does NOT resample: `tts_config.client_playback_rate` already aligns the
 * gateway egress to the player's sink rate, so every frame here is already at
 * the right rate (double-resampling would add aliasing + latency).
 *
 * The buffer is pure logic over an injected `release` sink + clock + timer hooks
 * (no Web Audio, no DOM), so it is unit-testable with fake timers. The player
 * owns one and routes `addFloat32` through it when enabled.
 */

/** Tunables for the adaptive jitter buffer. */
export interface JitterBufferConfig {
  /** Sample rate of the frames flowing through (Hz). Must match the sink rate. */
  sampleRate: number;
  /**
   * Target initial/baseline buffer depth in ms once engaged (default 40). Grows
   * toward `maxDelayMs` as measured jitter rises; never below this when engaged.
   */
  targetDelayMs?: number;
  /** Hard cap on buffer depth in ms (default 120) — bounds added latency. */
  maxDelayMs?: number;
  /**
   * Inter-arrival jitter (ms, smoothed mean-abs-deviation) above which the
   * buffer ENGAGES from passthrough (default 8). Below it (after a settle
   * period) it returns to zero-latency passthrough. The hysteresis avoids
   * flapping on a single late packet.
   */
  engageJitterMs?: number;
  /**
   * Nominal frame duration in ms (default 20 — the gateway's audio_out_chunk_ms).
   * Used to size buffer depth in whole frames and to clock concealed releases.
   */
  frameMs?: number;
  /**
   * Max consecutive concealed (PLC) frames before giving up and emitting silence
   * /stopping (default 5 = ~100ms). Beyond this the loss is too long to mask.
   */
  maxConcealFrames?: number;
  /** EWMA smoothing factor for the jitter estimate (default 0.1). */
  jitterSmoothing?: number;
}

export type ResolvedJitterBufferConfig = Required<JitterBufferConfig>;

const DEFAULTS: Omit<ResolvedJitterBufferConfig, 'sampleRate'> = {
  targetDelayMs: 40,
  maxDelayMs: 120,
  engageJitterMs: 8,
  frameMs: 20,
  maxConcealFrames: 5,
  jitterSmoothing: 0.1,
};

/** Hooks the jitter buffer needs from its host (the player). */
export interface JitterBufferHooks {
  /** Hand one ready frame to the player for scheduled playout. */
  release: (frame: Float32Array) => void;
  /** Monotonic clock in ms (injectable for tests). Defaults to performance.now(). */
  now?: () => number;
  /** Schedule a delayed callback (injectable). Defaults to setTimeout. */
  setTimer?: (cb: () => void, ms: number) => ReturnType<typeof setTimeout>;
  /** Cancel a scheduled callback (injectable). Defaults to clearTimeout. */
  clearTimer?: (id: ReturnType<typeof setTimeout>) => void;
}

/** One buffered frame with its ordering key + arrival time. */
interface QueuedFrame {
  data: Float32Array;
  /** Source sequence number, or a synthesized monotonic index. */
  seq: number;
  /** Whether `seq` came from the source (true) or was synthesized (false). */
  hasSeq: boolean;
  arrival: number;
}

/** A monotonic clock that never goes backwards. */
function defaultNow(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}

/**
 * Adaptive jitter buffer with PLC + time-stretch drain. Engages only under
 * measured arrival jitter; clean networks pass through with zero added latency.
 */
export class JitterBuffer {
  private readonly config: ResolvedJitterBufferConfig;
  private readonly hooks: Required<Pick<JitterBufferHooks, 'release'>> & JitterBufferHooks;
  private readonly now: () => number;
  private readonly setTimer: (cb: () => void, ms: number) => ReturnType<typeof setTimeout>;
  private readonly clearTimer: (id: ReturnType<typeof setTimeout>) => void;

  /** Whether buffering is currently engaged (vs zero-latency passthrough). */
  private engaged = false;
  /** Ordered pending frames (ascending seq). */
  private queue: QueuedFrame[] = [];
  private releaseTimer: ReturnType<typeof setTimeout> | null = null;

  // Arrival-jitter estimation.
  private lastArrival: number | null = null;
  private smoothedIntervalMs: number;
  private jitterMs = 0;
  /** Synthesized sequence counter for sources without sequence numbers. */
  private nextSyntheticSeq = 0;
  /** seq of the next frame we expect to play (for gap/reorder detection). */
  private playSeq: number | null = null;
  /** Last frame's tail, kept for concealment extrapolation. */
  private lastReleased: Float32Array | null = null;
  private concealRun = 0;

  // Diagnostics.
  private concealedFrames = 0;
  private droppedSamples = 0;
  private engageCount = 0;

  constructor(config: JitterBufferConfig, hooks: JitterBufferHooks) {
    this.config = { ...DEFAULTS, ...config };
    this.hooks = hooks as Required<Pick<JitterBufferHooks, 'release'>> & JitterBufferHooks;
    this.now = hooks.now ?? defaultNow;
    this.setTimer = hooks.setTimer ?? ((cb, ms) => setTimeout(cb, ms));
    this.clearTimer = hooks.clearTimer ?? ((id) => clearTimeout(id));
    this.smoothedIntervalMs = this.config.frameMs;
  }

  /** Whether the buffer is currently engaged (diagnostics/tests). */
  isEngaged(): boolean {
    return this.engaged;
  }

  /** Current measured arrival jitter in ms (smoothed MAD; diagnostics/tests). */
  getJitterMs(): number {
    return this.jitterMs;
  }

  /** Frames currently held in the reorder queue (diagnostics/tests). */
  getDepth(): number {
    return this.queue.length;
  }

  /** Total frames concealed via PLC (diagnostics/tests). */
  getConcealedFrames(): number {
    return this.concealedFrames;
  }

  /** Total samples dropped by time-stretch drain (diagnostics/tests). */
  getDroppedSamples(): number {
    return this.droppedSamples;
  }

  /** How many times the buffer has engaged from passthrough (diagnostics/tests). */
  getEngageCount(): number {
    return this.engageCount;
  }

  /** Target buffer depth in whole frames given the current jitter estimate. */
  private targetFrames(): number {
    // Buffer enough to cover ~2x the measured jitter, clamped to [target,max].
    const jitterDepthMs = Math.min(this.config.maxDelayMs, Math.max(this.config.targetDelayMs, this.jitterMs * 2));
    return Math.max(1, Math.round(jitterDepthMs / this.config.frameMs));
  }

  /**
   * Push one arriving frame. Updates the jitter estimate, decides engage vs
   * passthrough, and either releases immediately (passthrough) or enqueues for
   * ordered, jitter-absorbing release.
   *
   * @param data  The frame's samples (already at the sink rate — NOT resampled).
   * @param seq   Optional source sequence number for reordering/gap detection.
   */
  push(data: Float32Array, seq?: number): void {
    if (data.length === 0) return;
    const arrival = this.now();
    this.updateJitter(arrival);

    const hasSeq = seq !== undefined;
    const frame: QueuedFrame = {
      data,
      seq: hasSeq ? seq : this.nextSyntheticSeq++,
      hasSeq,
      arrival,
    };

    // Engage/disengage decision (hysteresis): jitter above the threshold turns
    // buffering on; a calm period with an empty queue turns it back off.
    if (!this.engaged && this.jitterMs >= this.config.engageJitterMs) {
      this.engage();
    }

    if (!this.engaged) {
      // Passthrough: zero added latency on a clean network.
      this.playSeq = frame.seq + 1;
      this.emit(frame.data);
      return;
    }

    this.insertOrdered(frame);
    this.ensureReleaseTimer();
  }

  /** Smoothly estimate inter-arrival interval + jitter (mean abs deviation). */
  private updateJitter(arrival: number): void {
    if (this.lastArrival !== null) {
      const interval = arrival - this.lastArrival;
      const a = this.config.jitterSmoothing;
      // Deviation of this interval from the smoothed nominal interval.
      const deviation = Math.abs(interval - this.smoothedIntervalMs);
      this.jitterMs = (1 - a) * this.jitterMs + a * deviation;
      this.smoothedIntervalMs = (1 - a) * this.smoothedIntervalMs + a * interval;
    }
    this.lastArrival = arrival;
  }

  private engage(): void {
    this.engaged = true;
    this.engageCount++;
  }

  /** Insert a frame keeping the queue ascending by seq; drop stale duplicates. */
  private insertOrdered(frame: QueuedFrame): void {
    // Already-played frame arriving late → drop (too old to be useful).
    if (this.playSeq !== null && frame.hasSeq && frame.seq < this.playSeq) {
      return;
    }
    let i = this.queue.length;
    while (i > 0 && this.queue[i - 1]!.seq > frame.seq) i--;
    // Exact duplicate seq → keep the first, ignore the rest.
    if (i > 0 && this.queue[i - 1]!.seq === frame.seq) return;
    this.queue.splice(i, 0, frame);
  }

  /**
   * Arm the release clock if not already armed. Once engaged we release one
   * frame per frame-interval, holding back until the queue has reached the
   * adaptive target depth so late frames have time to arrive.
   */
  private ensureReleaseTimer(): void {
    if (this.releaseTimer !== null) return;
    // Wait until we've accumulated the target depth before first release, so the
    // buffer actually absorbs jitter (don't drain it instantly).
    if (this.queue.length < this.targetFrames()) return;
    this.scheduleNextRelease(0);
  }

  private scheduleNextRelease(delayMs: number): void {
    this.releaseTimer = this.setTimer(() => {
      this.releaseTimer = null;
      this.tick();
    }, delayMs);
  }

  /**
   * One release tick: emit the next in-order frame (or conceal a gap), apply
   * time-stretch drain when over-buffered, and reschedule while frames remain.
   */
  private tick(): void {
    if (!this.engaged) return;

    const depth = this.queue.length;
    const target = this.targetFrames();

    // Over-buffered (latency creeping past target+slack): drain by time-stretch
    // — drop a short run from the NEXT frame at a zero crossing (no pitch shift).
    let timeStretch = false;
    if (depth > target + 1) {
      timeStretch = true;
    }

    const head = this.queue[0];
    if (head && (this.playSeq === null || head.seq === this.playSeq || !head.hasSeq)) {
      // In-order frame available.
      this.queue.shift();
      this.playSeq = head.seq + 1;
      this.concealRun = 0;
      let out = head.data;
      if (timeStretch) {
        out = this.dropAtZeroCrossing(out);
      }
      this.emit(out);
    } else if (head && this.concealRun < this.config.maxConcealFrames) {
      // Next expected frame is missing/late but later frames exist → conceal one
      // frame-time of audio to bridge the gap, then re-check (the late frame may
      // arrive; if not, we advance playSeq so we don't stall forever).
      this.concealOne();
      if (this.playSeq !== null) this.playSeq++;
    } else if (!head) {
      // Underran: nothing queued. If we recently played, conceal briefly to ride
      // out the gap; otherwise go idle (let the buffer refill / disengage).
      if (this.lastReleased && this.concealRun < this.config.maxConcealFrames) {
        this.concealOne();
      } else {
        this.maybeDisengage();
        return; // don't reschedule; refill or a push() will re-arm.
      }
    } else {
      // Conceal budget exhausted with a gap: skip the gap, resync to the head.
      this.playSeq = head.seq;
      return this.tick();
    }

    // Keep draining at the frame cadence while frames remain behind the playhead
    // (a gap we're concealing still has queued frames waiting after it). When the
    // queue empties, stop the clock — a push() re-arms it, or we disengage.
    if (this.queue.length > 0) {
      this.scheduleNextRelease(this.config.frameMs);
    } else {
      this.maybeDisengage();
    }
  }

  /** Emit a frame to the player and remember its tail for concealment. */
  private emit(frame: Float32Array): void {
    this.lastReleased = frame;
    this.hooks.release(frame);
  }

  /**
   * Packet-loss concealment: synthesize one frame by extrapolating the last
   * frame faded toward silence (repeat-with-fade). Cheap, click-free, and good
   * enough to mask a single missing 20ms frame.
   */
  private concealOne(): void {
    this.concealRun++;
    this.concealedFrames++;
    const frameSamples = Math.max(1, Math.round((this.config.sampleRate * this.config.frameMs) / 1000));
    const src = this.lastReleased;
    const out = new Float32Array(frameSamples);
    if (src && src.length > 0) {
      // Copy the tail of the last frame, applying a linear fade so repeated
      // concealment decays to silence instead of buzzing on a held value.
      const startFade = 1 - this.concealRun / (this.config.maxConcealFrames + 1);
      const endFade = 1 - (this.concealRun + 1) / (this.config.maxConcealFrames + 1);
      for (let i = 0; i < frameSamples; i++) {
        const s = src[i % src.length]!;
        const t = i / frameSamples;
        const gain = Math.max(0, startFade + (endFade - startFade) * t);
        out[i] = s * gain;
      }
    }
    this.emit(out);
  }

  /**
   * Time-stretch drain: drop a short run (~10% of the frame) at the nearest
   * zero-crossing so the splice is click-free and the pitch is unchanged. Used
   * to shrink latency when over-buffered.
   */
  private dropAtZeroCrossing(frame: Float32Array): Float32Array {
    const dropLen = Math.max(1, Math.floor(frame.length * 0.1));
    if (frame.length <= dropLen + 2) return frame;
    // Find a zero crossing near the middle to splice across.
    const mid = Math.floor(frame.length / 2);
    let cut = mid;
    for (let off = 0; off < mid; off++) {
      const i = mid + off;
      if (i > 0 && i < frame.length && sign(frame[i - 1]!) !== sign(frame[i]!)) { cut = i; break; }
      const j = mid - off;
      if (j > 0 && j < frame.length && sign(frame[j - 1]!) !== sign(frame[j]!)) { cut = j; break; }
    }
    const keep = Math.min(dropLen, frame.length - cut);
    const out = new Float32Array(frame.length - keep);
    out.set(frame.subarray(0, cut), 0);
    out.set(frame.subarray(cut + keep), cut);
    this.droppedSamples += keep;
    return out;
  }

  /** Disengage back to zero-latency passthrough once the link is calm + drained. */
  private maybeDisengage(): void {
    if (this.queue.length === 0 && this.jitterMs < this.config.engageJitterMs * 0.5) {
      this.engaged = false;
      this.concealRun = 0;
    }
  }

  /**
   * Flush everything immediately to the player (e.g. on stream end) and reset to
   * passthrough. Cancels the release clock.
   */
  flush(): void {
    if (this.releaseTimer !== null) {
      this.clearTimer(this.releaseTimer);
      this.releaseTimer = null;
    }
    for (const f of this.queue) {
      this.emit(f.data);
    }
    this.queue = [];
    this.concealRun = 0;
  }

  /** Drop all buffered audio + reset (e.g. on barge-in / stop). */
  reset(): void {
    if (this.releaseTimer !== null) {
      this.clearTimer(this.releaseTimer);
      this.releaseTimer = null;
    }
    this.queue = [];
    this.engaged = false;
    this.playSeq = null;
    this.lastReleased = null;
    this.concealRun = 0;
    this.lastArrival = null;
    this.jitterMs = 0;
    this.smoothedIntervalMs = this.config.frameMs;
    this.nextSyntheticSeq = 0;
  }
}

function sign(x: number): number {
  return x < 0 ? -1 : 1;
}
