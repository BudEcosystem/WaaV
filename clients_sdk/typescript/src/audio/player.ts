/**
 * PCM Audio Player (D2: scheduled-chunk playout + true barge-in)
 *
 * Plays streamed PCM using the Web Audio API with a PLAYOUT CLOCK rather than
 * the old concatenate-and-start scheme. Each incoming ~20ms PCM chunk becomes a
 * tiny AudioBuffer that is scheduled at an explicit `nextStartTime`:
 *
 *     playhead = max(ctx.currentTime, nextStartTime)
 *     source.start(playhead); nextStartTime = playhead + chunk.duration
 *
 * This eliminates the underrun gaps the old player produced (back-to-back
 * `source.start()` with no `when` raced the audio clock) AND, by keeping every
 * scheduled source, lets barge-in `source.stop()` EVERY future-scheduled node so
 * the audio the user is *currently hearing* is cut within one chunk — the old
 * `clearBuffer()`/`stop()` only emptied a JS array and never stopped a node that
 * was already scheduled, so a barge-in did not interrupt the bot.
 *
 * The player is LOSS-TOLERANT: the gateway gives no per-frame delivery guarantee
 * for downlink audio (DroppableAudio), so a chunk whose scheduled start has
 * already slipped far into the past is DROPPED rather than scheduled late — we
 * never accumulate playout latency to "catch up".
 *
 * Gateway leverage: pair this with `tts_config.client_playback_rate` = the
 * AudioContext sink rate (the gateway resamples downlink with continuous filter
 * state, so this player does ZERO downlink resampling) and
 * `tts_config.audio_out_chunk_ms = 20` (egress arrives pre-framed at 20ms so a
 * `clear` truncates within one server chunk). The session sets both by default.
 */

import { AudioProcessor } from './processor.js';
import { JitterBuffer, type JitterBufferConfig } from './jitter-buffer.js';

/**
 * Player configuration
 */
export interface PlayerConfig {
  /**
   * Sample rate of the incoming PCM AND the AudioContext sink (default: 24000).
   * Because the gateway resamples downlink to `client_playback_rate`, the SDK
   * sends this value as `client_playback_rate` so the incoming PCM already
   * matches the sink rate and no client resampling is needed.
   */
  sampleRate?: number;
  /** Number of channels (default: 1) */
  channels?: number;
  /** Buffer size in samples (default: 4096) — retained for back-compat (unused by scheduled playout). */
  bufferSize?: number;
  /**
   * Maximum amount of audio (seconds) allowed to be scheduled ahead of the
   * playout clock. Beyond this we still schedule (audio is contiguous) but it
   * bounds how much we pre-roll; mainly a safety rail (default: 2.0).
   */
  maxBufferDuration?: number;
  /** Gain in dB (default: 0) */
  gainDb?: number;
  /**
   * Grace window (seconds) for a slipped chunk. If a chunk would have to start
   * more than this far in the past (i.e. we underran and fell behind), it is
   * DROPPED instead of scheduled late, so latency never accumulates (default
   * 0.04 = two 20ms chunks).
   */
  lateFrameGraceSec?: number;
  /**
   * Adaptive jitter buffer + PLC (D9). When enabled, incoming chunks first pass
   * through a {@link JitterBuffer} that engages ONLY under measured network
   * arrival jitter — reordering, concealing late/lost frames, and time-stretching
   * to drain — before reaching the scheduled playout below. On a clean network
   * it passes through with ZERO added latency, so it is safe to leave on. `false`
   * (default) disables it; `true` enables with defaults; an object tunes it. It
   * does NOT resample (client_playback_rate already aligns the rate).
   */
  jitter?: boolean | Omit<JitterBufferConfig, 'sampleRate'>;
}

/**
 * Player state
 */
export type PlayerState = 'idle' | 'playing' | 'paused' | 'stopped';

/**
 * Player event handlers
 */
export interface PlayerEventHandlers {
  /** Called when playback starts */
  onPlay?: () => void;
  /** Called when playback pauses */
  onPause?: () => void;
  /** Called when playback stops */
  onStop?: () => void;
  /** Called when buffer is empty (underrun) */
  onBufferEmpty?: () => void;
  /** Called when buffer is full */
  onBufferFull?: () => void;
  /** Called on playback error */
  onError?: (error: Error) => void;
  /** Called with playback progress */
  onProgress?: (currentTime: number, bufferedTime: number) => void;
  /**
   * Called when a local barge-in (interrupt) cuts playback, so the caller can
   * send the gateway `clear` op (the player has no socket of its own). Invoked
   * by {@link interrupt}; NOT by a plain {@link stop} (which is a local-only
   * teardown). Wire this to `session.clear()`.
   */
  onClear?: () => void;
}

/**
 * PCM Audio Player using the Web Audio API with a scheduled playout clock.
 */
export class PCMPlayer {
  private config: Required<PlayerConfig>;
  private audioContext: AudioContext | null = null;
  private gainNode: GainNode | null = null;
  private state: PlayerState = 'idle';
  private handlers: PlayerEventHandlers = {};
  /** Sources scheduled at-or-after the playout clock; killed on barge-in. */
  private scheduledSources: Set<AudioBufferSourceNode> = new Set();
  /** Absolute AudioContext time at which the next chunk should start. */
  private nextStartTime = 0;
  /** Total samples actually scheduled for playback (for progress reporting). */
  private totalSamplesScheduled = 0;
  /** Samples dropped as late (diagnostics). */
  private droppedLateSamples = 0;
  /** D9: optional adaptive jitter buffer in front of scheduled playout. */
  private jitterBuffer: JitterBuffer | null = null;
  /**
   * D10: when true, this playback is a must-play prompt (e.g. a legal
   * disclaimer) and a barge-in {@link interrupt}/{@link clearBuffer} is IGNORED
   * so it plays to completion. Set via {@link setUninterruptible}.
   */
  private uninterruptible = false;

  constructor(config: PlayerConfig = {}) {
    this.config = {
      sampleRate: config.sampleRate ?? 24000,
      channels: config.channels ?? 1,
      bufferSize: config.bufferSize ?? 4096,
      maxBufferDuration: config.maxBufferDuration ?? 2.0,
      gainDb: config.gainDb ?? 0,
      lateFrameGraceSec: config.lateFrameGraceSec ?? 0.04,
      jitter: config.jitter ?? false,
    };

    // D9: build the jitter buffer when enabled. Its `release` feeds the same
    // scheduled-playout path used directly on a clean network; the buffer only
    // interposes ordering/PLC/drain once it MEASURES arrival jitter.
    if (config.jitter !== undefined && config.jitter !== false) {
      this.jitterBuffer = new JitterBuffer(
        {
          sampleRate: this.config.sampleRate,
          ...(typeof config.jitter === 'object' ? config.jitter : {}),
        },
        { release: (frame) => { void this.schedule(frame); } }
      );
    }
  }

  /**
   * Initialize audio context
   */
  async initialize(): Promise<void> {
    if (this.audioContext) return;

    try {
      this.audioContext = new AudioContext({ sampleRate: this.config.sampleRate });
      this.gainNode = this.audioContext.createGain();
      this.gainNode.gain.value = AudioProcessor.dbToLinear(this.config.gainDb);
      this.gainNode.connect(this.audioContext.destination);
    } catch (err) {
      const error = err instanceof Error ? err : new Error(String(err));
      this.handlers.onError?.(error);
      throw error;
    }
  }

  /**
   * Set event handlers
   */
  setHandlers(handlers: PlayerEventHandlers): void {
    this.handlers = { ...this.handlers, ...handlers };
  }

  /**
   * Get current state
   */
  getState(): PlayerState {
    return this.state;
  }

  /**
   * Get audio context sample rate (the sink rate — what to send as
   * `client_playback_rate` so the gateway pre-resamples downlink).
   */
  getSampleRate(): number {
    return this.audioContext?.sampleRate ?? this.config.sampleRate;
  }

  /**
   * Set gain in dB
   */
  setGain(gainDb: number): void {
    this.config.gainDb = gainDb;
    if (this.gainNode) {
      this.gainNode.gain.value = AudioProcessor.dbToLinear(gainDb);
    }
  }

  /**
   * D10: mark the current/next playback as a must-play prompt. While true, a
   * barge-in {@link interrupt}/{@link clearBuffer} is IGNORED so the prompt
   * (e.g. a legal disclaimer the user must hear) is never cut off. Clear it
   * (`false`) once the prompt finishes to restore normal barge-in. A plain
   * {@link stop} still tears down (it is an explicit local teardown, not a
   * barge-in).
   */
  setUninterruptible(value: boolean): void {
    this.uninterruptible = value;
  }

  /** D10: whether the current playback is flagged must-play (uninterruptible). */
  isUninterruptible(): boolean {
    return this.uninterruptible;
  }

  /**
   * Get buffered duration in seconds — how far the scheduled playout clock is
   * ahead of the audio context's current time.
   */
  getBufferedDuration(): number {
    if (!this.audioContext) return 0;
    return Math.max(0, this.nextStartTime - this.audioContext.currentTime);
  }

  /**
   * Check if buffer has space (we are not pre-rolled beyond maxBufferDuration).
   */
  hasBufferSpace(): boolean {
    return this.getBufferedDuration() < this.config.maxBufferDuration;
  }

  /** Number of sources currently scheduled (future + playing). */
  getScheduledCount(): number {
    return this.scheduledSources.size;
  }

  /** Samples dropped because they arrived too late to schedule (diagnostics). */
  getDroppedLateSamples(): number {
    return this.droppedLateSamples;
  }

  /**
   * D9 jitter-buffer diagnostics: whether it is currently engaged (vs zero-
   * latency passthrough), the measured arrival jitter (ms), how many frames it
   * concealed (PLC), and how many samples it dropped to drain. All zero/false
   * when the jitter buffer is disabled.
   */
  getJitterStats(): { enabled: boolean; engaged: boolean; jitterMs: number; concealedFrames: number; droppedSamples: number } {
    if (!this.jitterBuffer) {
      return { enabled: false, engaged: false, jitterMs: 0, concealedFrames: 0, droppedSamples: 0 };
    }
    return {
      enabled: true,
      engaged: this.jitterBuffer.isEngaged(),
      jitterMs: this.jitterBuffer.getJitterMs(),
      concealedFrames: this.jitterBuffer.getConcealedFrames(),
      droppedSamples: this.jitterBuffer.getDroppedSamples(),
    };
  }

  /**
   * Add PCM data to the playout (Int16 format).
   * @param pcmData Int16 PCM data
   * @param seq Optional source sequence number (D9 jitter-buffer reordering).
   */
  addPCM(pcmData: ArrayBuffer | Int16Array, seq?: number): void {
    const int16 = pcmData instanceof Int16Array ? pcmData : new Int16Array(pcmData);
    const float32 = AudioProcessor.int16ToFloat32(int16);
    this.addFloat32(float32, seq);
  }

  /**
   * Add Float32 audio to the playout. With the D9 jitter buffer enabled the
   * chunk first passes through it (engages only under measured arrival jitter;
   * zero-latency passthrough otherwise); without it the chunk is scheduled
   * immediately on the playout clock. Each call is treated as one contiguous
   * chunk.
   * @param audioData Float32 audio data
   * @param seq Optional source sequence number (D9 jitter-buffer reordering).
   */
  addFloat32(audioData: Float32Array, seq?: number): void {
    if (audioData.length === 0) return;
    if (this.state === 'paused' || this.state === 'stopped') {
      // Don't schedule while paused/stopped; drop (loss-tolerant).
      return;
    }
    if (this.jitterBuffer) {
      // D9: the buffer's release() routes back into schedule() below.
      this.jitterBuffer.push(audioData, seq);
      return;
    }
    // Fire-and-forget; initialization is awaited inside.
    void this.schedule(audioData);
  }

  /**
   * Schedule one chunk at the playout clock. Drops the chunk if it has slipped
   * too far behind (loss-tolerant: never schedule late and accumulate latency).
   */
  private async schedule(audioData: Float32Array): Promise<void> {
    if (!this.audioContext || !this.gainNode) {
      await this.initialize();
    }
    const ctx = this.audioContext!;
    const gain = this.gainNode!;

    try {
      if (ctx.state === 'suspended') {
        await ctx.resume();
      }
    } catch {
      // Best-effort; some browsers require a user gesture. Scheduling below will
      // still queue and play once the context resumes.
    }

    // A late-arriving async resume could have raced a stop()/pause(); bail.
    if (this.state === 'paused' || this.state === 'stopped') return;

    const now = ctx.currentTime;
    const duration = audioData.length / this.config.sampleRate;

    // Playout clock: never start before `now`; if we've drained, restart at now.
    let startAt = Math.max(now, this.nextStartTime);

    // Loss tolerance: if the queue underran and `nextStartTime` fell far behind
    // `now`, `startAt` already clamps to `now` (no late audio). If a chunk is
    // *individually* older than the grace window, drop it.
    if (this.nextStartTime > 0 && now - this.nextStartTime > this.config.lateFrameGraceSec) {
      // We fell behind — resync the clock to now and drop nothing further; the
      // clamp above keeps audio contiguous from `now`.
      this.droppedLateSamples += Math.round((now - this.nextStartTime) * this.config.sampleRate);
      startAt = now;
    }

    const audioBuffer = ctx.createBuffer(this.config.channels, audioData.length, this.config.sampleRate);
    audioBuffer.getChannelData(0).set(audioData);

    const source = ctx.createBufferSource();
    source.buffer = audioBuffer;
    source.connect(gain);

    this.scheduledSources.add(source);
    source.onended = () => {
      this.scheduledSources.delete(source);
      if (this.scheduledSources.size === 0 && this.state === 'playing') {
        // Drained — go idle and notify underrun/empty.
        this.state = 'idle';
        this.handlers.onBufferEmpty?.();
      }
    };

    try {
      source.start(startAt);
    } catch (err) {
      this.scheduledSources.delete(source);
      this.handlers.onError?.(err instanceof Error ? err : new Error(String(err)));
      return;
    }

    this.nextStartTime = startAt + duration;
    this.totalSamplesScheduled += audioData.length;

    if (this.state !== 'playing') {
      this.state = 'playing';
      this.handlers.onPlay?.();
    }

    if (!this.hasBufferSpace()) {
      this.handlers.onBufferFull?.();
    }

    const currentTime = now;
    this.handlers.onProgress?.(currentTime, this.nextStartTime);
  }

  /**
   * Start/resume playback. With scheduled playout there is no pending JS queue
   * to flush; resuming simply un-pauses the context so future chunks schedule.
   */
  async play(): Promise<void> {
    if (this.state === 'playing') return;

    if (!this.audioContext) {
      await this.initialize();
    }
    if (this.audioContext!.state === 'suspended') {
      await this.audioContext!.resume();
    }
    if (this.state === 'paused') {
      // Resume the playout clock from now; chunks dropped while paused are gone.
      this.nextStartTime = this.audioContext!.currentTime;
      this.state = 'idle';
    }
  }

  /**
   * Pause playback: stop everything currently scheduled (Web Audio cannot pause
   * an individual scheduled source) and mark paused so new chunks are dropped.
   */
  pause(): void {
    if (this.state !== 'playing' && this.state !== 'idle') return;
    this.jitterBuffer?.reset();
    this.stopAllScheduled();
    this.state = 'paused';
    this.handlers.onPause?.();
  }

  /**
   * Stop playback and clear all scheduled audio. Local teardown only — does NOT
   * fire `onClear` (use {@link interrupt} for a barge-in that also tells the
   * gateway to stop generating).
   */
  stop(): void {
    this.jitterBuffer?.reset();
    this.stopAllScheduled();
    this.state = 'stopped';
    this.nextStartTime = 0;
    this.totalSamplesScheduled = 0;
    this.handlers.onStop?.();
  }

  /**
   * Barge-in / interrupt: cut the audio the user is currently hearing AND signal
   * the gateway to stop generating. This is the TRUE barge-in the old player
   * lacked — it `source.stop()`s every scheduled node (not just the JS queue),
   * so the cut is audible within one ~20ms chunk, then fires `onClear` so the
   * caller can send the gateway `clear` op. Playback can resume on the next
   * incoming chunk.
   */
  interrupt(): void {
    // D10: a must-play prompt is not barge-over-able — ignore the cut entirely
    // (do not stop sources, do not fire onClear) so it plays to completion.
    if (this.uninterruptible) return;
    // D9: drop any jitter-buffered audio too, so barge-in cuts EVERYTHING
    // pending (not just what's already scheduled on the playout clock).
    this.jitterBuffer?.reset();
    this.stopAllScheduled();
    this.nextStartTime = 0;
    // Return to idle (not 'stopped') so a subsequent bot turn plays normally.
    this.state = 'idle';
    this.handlers.onClear?.();
  }

  /**
   * Clear all scheduled audio without changing state intent. Like a barge-in
   * cut, this stops every scheduled source (so currently-audible audio is cut),
   * but does NOT fire `onClear`. Kept for back-compat with callers that only
   * want to flush local playout.
   */
  clearBuffer(): void {
    // D10: respect the must-play flag — a barge-in clear is ignored while an
    // uninterruptible prompt is playing.
    if (this.uninterruptible) return;
    this.jitterBuffer?.reset();
    this.stopAllScheduled();
    this.nextStartTime = 0;
    if (this.state === 'playing') this.state = 'idle';
  }

  /**
   * Stop and discard every scheduled source, guarding against the double-stop
   * throw on an AudioBufferSourceNode that has already ended/stopped.
   */
  private stopAllScheduled(): void {
    for (const source of this.scheduledSources) {
      source.onended = null;
      try {
        source.stop();
      } catch {
        // Already stopped/ended — Web Audio throws on a second stop(); ignore.
      }
      try {
        source.disconnect();
      } catch {
        // Ignore.
      }
    }
    this.scheduledSources.clear();
  }

  /**
   * Get current playback time in seconds (the audio context clock).
   */
  getCurrentTime(): number {
    return this.audioContext?.currentTime ?? 0;
  }

  /**
   * Dispose resources
   */
  async dispose(): Promise<void> {
    this.stop();

    if (this.gainNode) {
      this.gainNode.disconnect();
      this.gainNode = null;
    }

    if (this.audioContext) {
      await this.audioContext.close();
      this.audioContext = null;
    }

    this.state = 'idle';
  }
}

/**
 * Create a PCM player instance
 */
export function createPCMPlayer(sampleRate = 24000, config?: Omit<PlayerConfig, 'sampleRate'>): PCMPlayer {
  return new PCMPlayer({ ...config, sampleRate });
}
