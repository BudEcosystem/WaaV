/**
 * Audio player for PCM audio playback (D2: scheduled-chunk playout + true barge-in).
 *
 * Instead of concatenating the queue and calling `source.start()` back-to-back
 * (which races the audio clock -> gaps on underrun), this schedules every PCM
 * chunk against a monotonic playout clock:
 *
 *   playhead = max(ctx.currentTime, nextStartTime)
 *   source.start(playhead)
 *   nextStartTime = playhead + chunk.duration
 *
 * and KEEPS a handle to every still-scheduled source. On a barge-in `stop()` we
 * `source.stop()` EVERY scheduled source (cutting audio the user is already
 * hearing within ~one chunk) AND drop the JS queue. This fixes both the underrun
 * glitch and the non-interruptible bug the old concatenate-and-start path had.
 *
 * Gateway leverage (see buildConfigMessage): the widget sends
 * `tts_config.client_playback_rate` = this AudioContext sink rate, so the
 * gateway resamples downlink PCM to EXACTLY this rate with continuous filter
 * state -> the player does ZERO downlink resampling (we decode the incoming
 * Int16 at the sink rate directly). And `tts_config.audio_out_chunk_ms=20`
 * makes the gateway pre-frame egress into ~20ms chunks so a `clear` truncates
 * within one server chunk AND the chunks arrive pre-sized for this scheduler.
 */

export type PlayerOptions = {
  sampleRate?: number;
  channels?: number;
};

export class AudioPlayer {
  private audioContext: AudioContext | null = null;
  private options: PlayerOptions;
  private gainNode: GainNode | null = null;
  private onPlaybackEndCallback: (() => void) | null = null;
  // Track initialization to prevent race condition creating multiple AudioContexts
  private initPromise: Promise<void> | null = null;

  // --- scheduled-playout clock ---
  /** The AudioContext time at which the next chunk should start. */
  private nextStartTime = 0;
  /** Every source that has been start()ed but not yet ended (for barge-in stop). */
  private scheduledSources = new Set<AudioBufferSourceNode>();

  constructor(options: PlayerOptions = {}) {
    this.options = {
      sampleRate: options.sampleRate || 24000,
      channels: options.channels || 1,
    };
  }

  async initialize(): Promise<void> {
    // Prevent multiple AudioContext creation
    if (this.audioContext) return;

    this.audioContext = new AudioContext({
      sampleRate: this.options.sampleRate,
    });
    this.gainNode = this.audioContext.createGain();
    this.gainNode.connect(this.audioContext.destination);
  }

  /**
   * D6 prewarm: adopt an AudioContext that was created early (at page-load) so the
   * first TTS chunk doesn't pay the cold context-create cost on the user gesture.
   * No-op if a context already exists (initialize() then stays a no-op). The
   * adopted context is wired with a fresh gain node exactly like initialize().
   */
  adoptContext(ctx: AudioContext): void {
    if (this.audioContext || !ctx) return;
    this.audioContext = ctx;
    this.gainNode = this.audioContext.createGain();
    this.gainNode.connect(this.audioContext.destination);
  }

  async play(audioData: ArrayBuffer): Promise<void> {
    // Use a single Promise to prevent race condition on initialization
    if (!this.audioContext) {
      if (!this.initPromise) {
        this.initPromise = this.initialize();
      }
      await this.initPromise;
    }

    if (!this.audioContext || !this.gainNode) return;

    // Resume context if suspended (required for autoplay policy)
    if (this.audioContext.state === 'suspended') {
      await this.audioContext.resume();
    }

    // A zero-length chunk (e.g. an empty binary keepalive frame) carries no audio
    // and must not advance the clock or schedule a silent source.
    if (audioData.byteLength < 2) {
      return;
    }

    // The incoming PCM is already at the AudioContext sink rate (the gateway
    // resampled it to client_playback_rate), so decode straight to a buffer —
    // no client-side resample.
    const audioBuffer = this.int16ToAudioBuffer(audioData);
    this.scheduleBuffer(audioBuffer);
  }

  /**
   * Schedule one chunk against the playout clock. playhead never goes backwards:
   * if we are caught up (or underran) it pins to ctx.currentTime so the chunk
   * plays now; otherwise it queues seamlessly after the previously-scheduled tail.
   */
  private scheduleBuffer(buffer: AudioBuffer): void {
    if (!this.audioContext || !this.gainNode) return;

    const now = this.audioContext.currentTime;
    // max(currentTime, nextStartTime): pin to now on first chunk / after an
    // underrun, otherwise append seamlessly after the prior chunk's tail.
    const startAt = Math.max(now, this.nextStartTime);

    const source = this.audioContext.createBufferSource();
    source.buffer = buffer;
    source.connect(this.gainNode);

    source.onended = () => {
      // Drop our handle; once the queue fully drains, fire the end callback.
      this.scheduledSources.delete(source);
      if (this.scheduledSources.size === 0) {
        if (this.onPlaybackEndCallback) {
          this.onPlaybackEndCallback();
        }
      }
    };

    this.scheduledSources.add(source);
    source.start(startAt);
    this.nextStartTime = startAt + buffer.duration;
  }

  /**
   * True barge-in: stop EVERY still-scheduled source (cuts audio the user is
   * currently hearing) and drop the queue. Resets the playout clock so the next
   * arriving chunk starts cleanly at ctx.currentTime.
   */
  stop(): void {
    for (const source of this.scheduledSources) {
      try {
        // onended would re-fire the end callback mid-drain; detach it first.
        source.onended = null;
        source.stop();
      } catch (e) {
        // stop() throws if the source already ended / was never started — ignore.
      }
    }
    this.scheduledSources.clear();
    this.nextStartTime = 0;
  }

  setVolume(volume: number): void {
    if (this.gainNode) {
      this.gainNode.gain.value = Math.max(0, Math.min(1, volume));
    }
  }

  onPlaybackEnd(callback: () => void): void {
    this.onPlaybackEndCallback = callback;
  }

  private int16ToAudioBuffer(data: ArrayBuffer): AudioBuffer {
    if (!this.audioContext) {
      throw new Error('AudioContext not initialized');
    }

    // Guard odd byte lengths: an Int16Array view requires an even byteLength, so
    // floor to whole samples (a trailing odd byte would throw on the view).
    const numSamples = Math.floor(data.byteLength / 2);
    const int16Array = new Int16Array(data, 0, numSamples);

    const audioBuffer = this.audioContext.createBuffer(
      this.options.channels || 1,
      numSamples,
      this.options.sampleRate || 24000
    );

    const channelData = audioBuffer.getChannelData(0);
    for (let i = 0; i < numSamples; i++) {
      channelData[i] = int16Array[i] / 32768;
    }

    return audioBuffer;
  }

  /** True while any chunk is scheduled/playing. */
  get playing(): boolean {
    return this.scheduledSources.size > 0;
  }

  close(): void {
    this.stop();
    if (this.audioContext) {
      this.audioContext.close();
      this.audioContext = null;
    }
    this.gainNode = null;
    // Reset initialization Promise so a new context can be created if needed
    this.initPromise = null;
  }
}
