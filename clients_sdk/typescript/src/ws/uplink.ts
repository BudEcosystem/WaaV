/**
 * Uplink audio backpressure shedder (D5).
 *
 * The gateway gives no per-frame delivery guarantee for uplink audio — it sheds
 * stale DroppableAudio under load (messages.rs:458). The SDK MIRRORS that
 * discipline on its own uplink so a slow gateway (or a saturated client send
 * buffer) never accumulates uplink latency: outbound audio frames go through a
 * bounded ring, and a drain pump only hands a frame to the socket while the
 * WebSocket `bufferedAmount` is below a high-water mark. When the ring is full
 * we shed the OLDEST frame (newest audio is the most useful for live STT), and
 * when `bufferedAmount` is over the mark we stop pushing and let it drain.
 *
 * This is for the CONNECTED, backpressured case. Offline buffering (drop-oldest
 * too) is handled separately by the session's MessageQueue.
 *
 * The shedder is pure logic over injected `send`/`bufferedAmount` hooks (no
 * direct socket), so it is unit-testable with a fake socket.
 */

export interface UplinkConfig {
  /**
   * Max audio frames held in the pre-socket ring (default 64). At 20ms/frame
   * that's ~1.3s of audio; beyond it we shed oldest rather than grow latency.
   */
  maxFrames?: number;
  /**
   * High-water mark for the socket `bufferedAmount` in bytes (default 262144 =
   * 256KB ≈ 1s of 16k mono PCM). While bufferedAmount is at/above this, the
   * pump pauses (lets the socket drain) and the ring absorbs/sheds.
   */
  highWaterBytes?: number;
}

export type ResolvedUplinkConfig = Required<UplinkConfig>;

export const DEFAULT_UPLINK_CONFIG: ResolvedUplinkConfig = {
  maxFrames: 64,
  highWaterBytes: 262144,
};

export interface UplinkHooks {
  /** Send one audio frame to the socket. Returns true if accepted. */
  send: (frame: ArrayBuffer) => boolean;
  /** Current socket send-buffer backlog in bytes (WebSocket.bufferedAmount). */
  bufferedAmount: () => number;
  /** True iff the socket is currently connected/open. */
  isConnected: () => boolean;
}

/**
 * Bounded, drop-oldest uplink ring with a bufferedAmount-gated drain pump.
 */
export class UplinkAudioShedder {
  private readonly config: ResolvedUplinkConfig;
  private readonly hooks: UplinkHooks;
  private ring: ArrayBuffer[] = [];
  private droppedFrames = 0;

  constructor(hooks: UplinkHooks, config: UplinkConfig = {}) {
    this.config = { ...DEFAULT_UPLINK_CONFIG, ...config };
    this.hooks = hooks;
  }

  /** Total frames shed (oldest-dropped) under backpressure (diagnostics). */
  getDroppedFrames(): number {
    return this.droppedFrames;
  }

  /** Frames currently waiting in the ring. */
  size(): number {
    return this.ring.length;
  }

  /**
   * Offer one outbound audio frame. Fast path: if the socket is connected, the
   * ring is empty, and bufferedAmount is below the mark, send immediately (no
   * queuing). Otherwise enqueue (shedding the oldest if the ring is full) and
   * run the pump.
   */
  push(frame: ArrayBuffer): void {
    if (
      this.ring.length === 0 &&
      this.hooks.isConnected() &&
      this.hooks.bufferedAmount() < this.config.highWaterBytes
    ) {
      if (this.hooks.send(frame)) return;
      // Send failed (race with close) — fall through to enqueue.
    }
    this.enqueue(frame);
    this.pump();
  }

  /** Enqueue a frame, shedding the OLDEST when the ring is at capacity. */
  private enqueue(frame: ArrayBuffer): void {
    if (this.ring.length >= this.config.maxFrames) {
      this.ring.shift(); // drop oldest — newest audio matters most for live STT
      this.droppedFrames++;
    }
    this.ring.push(frame);
  }

  /**
   * Drain the ring to the socket while connected and bufferedAmount is under the
   * high-water mark. Call on each push and whenever the socket may have drained
   * (the session also pumps on a timer while the ring is non-empty).
   */
  pump(): void {
    if (!this.hooks.isConnected()) return;
    while (this.ring.length > 0 && this.hooks.bufferedAmount() < this.config.highWaterBytes) {
      const frame = this.ring[0]!;
      if (!this.hooks.send(frame)) break; // socket rejected — stop, retry later
      this.ring.shift();
    }
  }

  /** True while frames remain queued (the session keeps the pump timer alive). */
  hasPending(): boolean {
    return this.ring.length > 0;
  }

  /** Drop all queued frames (e.g. on disconnect/teardown). */
  clear(): void {
    this.ring = [];
  }
}
