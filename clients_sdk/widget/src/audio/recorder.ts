/**
 * Audio recorder (D3: off-main-thread capture via AudioWorklet).
 *
 * Capture runs on the audio render thread in an AudioWorkletProcessor that
 * accumulates 128-sample render quanta into 20ms frames (320 samples @16k),
 * converts Float32 -> Int16 IN the worklet, and posts the Int16 frame to the
 * main thread as a TRANSFERABLE ArrayBuffer (zero-copy). This frees the main
 * thread (no React/GC/layout contention -> no jank/underrun) and gives a 20ms
 * input cadence instead of the ScriptProcessor's 256ms@16k buffer.
 *
 * The deprecated ScriptProcessorNode is kept ONLY as a legacy fallback for
 * browsers without AudioWorklet (`typeof AudioWorkletNode === 'undefined'`).
 *
 * The worklet module is registered via an inline Blob URL so no separate bundle
 * entry / fetchable asset is required (the widget ships as a single IIFE/ESM).
 *
 * VAD (RMS-based speech/silence) is computed on the main thread from each
 * received Int16 frame so behaviour is identical across both capture paths.
 *
 * D10 (getUserMedia / duplex-echo hardening): the capture constraints request
 * echoCancellation + noiseSuppression + autoGainControl = true by default (the
 * browser's own AEC/NS/AGC), each independently overridable. `voiceIsolation`
 * (iOS/Safari deep-noise-removal) is offered as an opt-in constraint. A mic that
 * goes HARD-silent — all-zero frames (OS hard-mute) or no frames at all (dead
 * device) — for ~0.5s fires an `onMicSilence` event so the app can surface "mic
 * may be muted" instead of a dead-air call. A live-but-quiet room (low nonzero
 * room tone) is NOT treated as muted, so a silent listener does not false-trigger.
 *
 * D3b (VAD hysteresis): the energy VAD uses start/stop-secs hysteresis (a short
 * sustained-loud window to START speech, a longer sustained-quiet window to STOP)
 * so a noisy room with brief dips does not mis-segment into spurious silence
 * edges (Pipecat vad_analyzer start_secs/stop_secs/min_volume).
 */

export type RecorderOptions = {
  sampleRate?: number;
  channels?: number;
  /** Browser AEC (cancel speaker bleed into the mic). Default true. */
  echoCancellation?: boolean;
  /** Browser noise suppression. Default true. */
  noiseSuppression?: boolean;
  /** Browser auto-gain control (level the mic). Default true. */
  autoGainControl?: boolean;
  /**
   * Opt-in deep voice isolation (iOS 16.4+/Safari `voiceIsolation`); only emitted
   * when explicitly set, since it is non-standard and unsupported elsewhere.
   */
  voiceIsolation?: boolean;
  // --- D3b VAD hysteresis (seconds) ---
  /** RMS above this counts a frame as loud (0..1). Default 0.01. */
  vadMinVolume?: number;
  /** Sustained-loud duration before declaring speech START. Default 0.2s. */
  vadStartSecs?: number;
  /** Sustained-quiet duration before declaring speech STOP. Default 0.8s. */
  vadStopSecs?: number;
  // --- D10 mic-silence watchdog ---
  /** Fire onMicSilence after this long with a fully-silent mic (ms). Default 500. */
  micSilenceMs?: number;
};

/**
 * The AudioWorkletProcessor source, registered from an inline Blob URL.
 *
 * Runs on the audio render thread. `process()` is called per 128-sample quantum;
 * we fill a reusable Int16 frame buffer and, once 20ms (FRAME_SAMPLES) is ready,
 * copy it into a fresh ArrayBuffer and postMessage it transferable. Allocation in
 * process() is kept minimal (one small buffer per 20ms frame, transferred away).
 */
const WORKLET_SOURCE = `
class BudCaptureProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = (options && options.processorOptions) || {};
    // 20ms @ the context sample rate (e.g. 320 samples @16k). Computed from the
    // real render-thread sampleRate so it is correct even if the context rate
    // differs from the requested rate.
    this.frameSamples = Math.max(1, Math.round(sampleRate * 0.02));
    this.buffer = new Int16Array(this.frameSamples);
    this.offset = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0) return true;
    const channel = input[0];
    if (!channel) return true;

    for (let i = 0; i < channel.length; i++) {
      let s = channel[i];
      if (s > 1) s = 1; else if (s < -1) s = -1;
      this.buffer[this.offset++] = s < 0 ? s * 0x8000 : s * 0x7fff;
      if (this.offset >= this.frameSamples) {
        // Copy the completed frame into a fresh buffer and transfer it (zero-copy).
        const out = this.buffer.slice(0);
        this.port.postMessage(out.buffer, [out.buffer]);
        this.offset = 0;
      }
    }
    return true;
  }
}
registerProcessor('bud-capture', BudCaptureProcessor);
`;

export class AudioRecorder {
  private stream: MediaStream | null = null;
  private audioContext: AudioContext | null = null;
  private processor: ScriptProcessorNode | null = null;
  private workletNode: AudioWorkletNode | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private workletUrl: string | null = null;
  private options: Required<Omit<RecorderOptions, 'voiceIsolation'>> & {
    voiceIsolation?: boolean;
  };
  private onDataCallback: ((data: Int16Array) => void) | null = null;
  private onSilenceCallback: (() => void) | null = null;
  private onSpeechCallback: (() => void) | null = null;
  private onMicSilenceCallback: (() => void) | null = null;

  // D3b VAD hysteresis state (all times in performance.now() ms).
  private silenceThreshold: number;
  /** Sustained-loud window required to START speech (ms). */
  private vadStartMs: number;
  /** Sustained-quiet window required to STOP speech (ms). */
  private vadStopMs: number;
  private isSpeaking = false;
  /** When the current sustained-loud run began (null = not currently loud). */
  private loudRunStart: number | null = null;
  /** When the current sustained-quiet run began (null = not currently quiet). */
  private quietRunStart: number | null = null;

  // D10 mic-silence watchdog.
  private micSilenceMs: number;
  /** Last time ANY non-silent frame arrived (mic-alive signal). */
  private lastAudibleFrameTime = 0;
  /** True once onMicSilence has fired for the current silent run (debounce). */
  private micSilenceFired = false;
  private micWatchdogId: ReturnType<typeof setInterval> | null = null;

  constructor(options: RecorderOptions = {}) {
    this.options = {
      sampleRate: options.sampleRate || 16000,
      channels: options.channels || 1,
      echoCancellation: options.echoCancellation ?? true,
      noiseSuppression: options.noiseSuppression ?? true,
      autoGainControl: options.autoGainControl ?? true,
      voiceIsolation: options.voiceIsolation,
      vadMinVolume: options.vadMinVolume ?? 0.01,
      vadStartSecs: options.vadStartSecs ?? 0.2,
      vadStopSecs: options.vadStopSecs ?? 0.8,
      micSilenceMs: options.micSilenceMs ?? 500,
    };
    this.silenceThreshold = this.options.vadMinVolume;
    this.vadStartMs = this.options.vadStartSecs * 1000;
    this.vadStopMs = this.options.vadStopSecs * 1000;
    this.micSilenceMs = this.options.micSilenceMs;
  }

  async start(): Promise<void> {
    try {
      // D10 capture constraints: browser AEC + NS + AGC default-on, each
      // independently overridable; voiceIsolation only when explicitly opted in
      // (non-standard). These names match the MediaTrackConstraints the gateway
      // can't do for us (server-side has no echo path to the user's speaker).
      const audioConstraints: Record<string, unknown> = {
        echoCancellation: this.options.echoCancellation,
        noiseSuppression: this.options.noiseSuppression,
        autoGainControl: this.options.autoGainControl,
        sampleRate: this.options.sampleRate,
        channelCount: this.options.channels,
      };
      if (this.options.voiceIsolation !== undefined) {
        audioConstraints.voiceIsolation = this.options.voiceIsolation;
      }

      this.stream = await navigator.mediaDevices.getUserMedia({
        audio: audioConstraints as MediaTrackConstraints,
      });

      this.audioContext = new AudioContext({
        sampleRate: this.options.sampleRate,
      });

      this.source = this.audioContext.createMediaStreamSource(this.stream);

      // Prefer the off-main-thread AudioWorklet; fall back to ScriptProcessor.
      if (this.workletSupported()) {
        await this.startWorklet();
      } else {
        this.startScriptProcessor();
      }

      // D10: arm the mic-silence watchdog. If NO audible frame arrives for
      // micSilenceMs the mic is likely OS-muted / dead; fire onMicSilence once.
      this.startMicWatchdog();
    } catch (error) {
      throw new Error(`Failed to start recording: ${error}`);
    }
  }

  /**
   * D10 mic-silence watchdog: a mic that is OS-muted or a dead device delivers
   * frames that are pure zero (or never delivers any), so the capture path looks
   * "running" while the call is silent. Track the last audible frame; if nothing
   * audible arrives within micSilenceMs, fire onMicSilence ONCE (debounced until
   * audio returns) so the app can prompt "mic may be muted".
   */
  private startMicWatchdog(): void {
    if (this.micWatchdogId !== null) return;
    // Seed alive so we don't fire instantly before the first frame lands.
    this.lastAudibleFrameTime = performance.now();
    this.micSilenceFired = false;
    // Check at half the deadline so detection lands within ~micSilenceMs.
    const period = Math.max(50, Math.floor(this.micSilenceMs / 2));
    this.micWatchdogId = setInterval(() => {
      if (this.micSilenceFired) return;
      if (performance.now() - this.lastAudibleFrameTime >= this.micSilenceMs) {
        this.micSilenceFired = true;
        if (this.onMicSilenceCallback) {
          this.onMicSilenceCallback();
        }
      }
    }, period);
    // Don't let this background timer hold the process/event loop open on its own
    // (Node's setInterval returns a Timeout with .unref(); the browser returns a
    // number without it, so guard the call).
    const t = this.micWatchdogId as unknown as { unref?: () => void };
    if (typeof t.unref === 'function') t.unref();
  }

  private stopMicWatchdog(): void {
    if (this.micWatchdogId !== null) {
      clearInterval(this.micWatchdogId);
      this.micWatchdogId = null;
    }
  }

  /** AudioWorklet is usable only if both the node and the addModule API exist. */
  private workletSupported(): boolean {
    return (
      typeof AudioWorkletNode !== 'undefined' &&
      !!this.audioContext &&
      !!this.audioContext.audioWorklet &&
      typeof this.audioContext.audioWorklet.addModule === 'function' &&
      typeof Blob !== 'undefined' &&
      typeof URL !== 'undefined' &&
      typeof URL.createObjectURL === 'function'
    );
  }

  /** D3 primary path: register the inline worklet and stream 20ms Int16 frames. */
  private async startWorklet(): Promise<void> {
    if (!this.audioContext || !this.source) return;

    const blob = new Blob([WORKLET_SOURCE], { type: 'application/javascript' });
    this.workletUrl = URL.createObjectURL(blob);
    await this.audioContext.audioWorklet.addModule(this.workletUrl);

    this.workletNode = new AudioWorkletNode(this.audioContext, 'bud-capture', {
      numberOfInputs: 1,
      numberOfOutputs: 0,
      channelCount: this.options.channels || 1,
    });

    this.workletNode.port.onmessage = (event: MessageEvent) => {
      // Transferable ArrayBuffer carrying one 20ms Int16 frame.
      const frame = new Int16Array(event.data as ArrayBuffer);
      this.handleFrame(frame);
    };

    // No output: connecting to a node sink would route mic audio to speakers.
    // The worklet pulls input as long as its source is connected.
    this.source.connect(this.workletNode);
  }

  /** Legacy fallback for browsers without AudioWorklet. */
  private startScriptProcessor(): void {
    if (!this.audioContext || !this.source) return;

    // Deprecated but widely supported; only used when AudioWorklet is absent.
    const bufferSize = 4096;
    this.processor = this.audioContext.createScriptProcessor(
      bufferSize,
      this.options.channels || 1,
      this.options.channels || 1
    );

    this.processor.onaudioprocess = (event) => {
      const inputData = event.inputBuffer.getChannelData(0);
      const int16Data = this.float32ToInt16(inputData);
      this.handleFrame(int16Data);
    };

    this.source.connect(this.processor);
    // ScriptProcessor only pulls audio while connected to a destination.
    this.processor.connect(this.audioContext.destination);
  }

  /**
   * Per-frame main-thread handling shared by both capture paths: a D3b hysteresis
   * RMS VAD (start/stop-secs so a noisy room with brief dips does not mis-segment)
   * + a D10 mic-alive signal + forwarding the Int16 frame to onData.
   *
   * Hysteresis: a frame is "loud" when rms > vadMinVolume. Speech STARTS only after
   * loud has been sustained for vadStartMs (a single loud blip won't trigger);
   * speech STOPS only after quiet has been sustained for vadStopMs (a brief dip
   * mid-utterance won't end it). This is the energy-path analog of Pipecat's
   * start_secs/stop_secs/min_volume.
   */
  private handleFrame(int16Data: Int16Array): void {
    const rms = this.calculateRMSInt16(int16Data);
    const now = performance.now();
    const loud = rms > this.silenceThreshold;

    // D10 mic-alive: a frame carrying ANY nonzero sample proves the device is
    // delivering real audio (even quiet room tone). Only HARD-silent frames
    // (every sample exactly 0 = OS hard-mute / dead device) leave the alive clock
    // stale — so a quiet-but-live room does NOT false-trigger "mic muted". Audio
    // returning re-arms the (already-fired) detector for the next gap.
    if (this.frameHasSignal(int16Data)) {
      this.lastAudibleFrameTime = now;
      this.micSilenceFired = false;
    }

    if (loud) {
      // A loud run is in progress; the quiet run (if any) is over.
      this.quietRunStart = null;
      if (this.loudRunStart === null) this.loudRunStart = now;
      if (!this.isSpeaking && now - this.loudRunStart >= this.vadStartMs) {
        // Sustained-loud long enough -> START speech.
        this.isSpeaking = true;
        if (this.onSpeechCallback) {
          this.onSpeechCallback();
        }
      }
    } else {
      // A quiet run is in progress; the loud run (if any) is over.
      this.loudRunStart = null;
      if (this.quietRunStart === null) this.quietRunStart = now;
      if (this.isSpeaking && now - this.quietRunStart >= this.vadStopMs) {
        // Sustained-quiet long enough -> STOP speech.
        this.isSpeaking = false;
        if (this.onSilenceCallback) {
          this.onSilenceCallback();
        }
      }
    }

    if (this.onDataCallback) {
      this.onDataCallback(int16Data);
    }
  }

  stop(): void {
    this.stopMicWatchdog();

    if (this.workletNode) {
      this.workletNode.port.onmessage = null;
      this.workletNode.disconnect();
      this.workletNode = null;
    }

    if (this.processor) {
      this.processor.disconnect();
      this.processor = null;
    }

    if (this.source) {
      this.source.disconnect();
      this.source = null;
    }

    if (this.audioContext) {
      this.audioContext.close();
      this.audioContext = null;
    }

    if (this.stream) {
      this.stream.getTracks().forEach((track) => track.stop());
      this.stream = null;
    }

    // Release the inline worklet Blob URL.
    if (this.workletUrl) {
      try {
        URL.revokeObjectURL(this.workletUrl);
      } catch (e) {
        // ignore
      }
      this.workletUrl = null;
    }

    // Reset VAD hysteresis + mic-silence state for the next start().
    this.isSpeaking = false;
    this.loudRunStart = null;
    this.quietRunStart = null;
    this.micSilenceFired = false;
  }

  onData(callback: (data: Int16Array) => void): void {
    this.onDataCallback = callback;
  }

  /**
   * D10: fired ~micSilenceMs after the mic goes fully silent (OS-muted / dead
   * device), once per silent run. The app can surface "mic may be muted".
   */
  onMicSilence(callback: () => void): void {
    this.onMicSilenceCallback = callback;
  }

  onSilence(callback: () => void): void {
    this.onSilenceCallback = callback;
  }

  onSpeech(callback: () => void): void {
    this.onSpeechCallback = callback;
  }

  private float32ToInt16(buffer: Float32Array): Int16Array {
    const result = new Int16Array(buffer.length);
    for (let i = 0; i < buffer.length; i++) {
      const s = Math.max(-1, Math.min(1, buffer[i]));
      result[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
    }
    return result;
  }

  /**
   * D10: true if the frame carries ANY nonzero sample (the device is delivering
   * real audio). A hard-muted mic / dead device delivers all-zero frames (or none),
   * which is the only thing that should trip the mic-silence watchdog — distinct
   * from a live-but-quiet room (low nonzero RMS).
   */
  private frameHasSignal(buffer: Int16Array): boolean {
    for (let i = 0; i < buffer.length; i++) {
      if (buffer[i] !== 0) return true;
    }
    return false;
  }

  /** RMS in [0,1) computed from Int16 samples (matches the old Float32 RMS scale). */
  private calculateRMSInt16(buffer: Int16Array): number {
    let sum = 0;
    for (let i = 0; i < buffer.length; i++) {
      const v = buffer[i] / 32768;
      sum += v * v;
    }
    return buffer.length ? Math.sqrt(sum / buffer.length) : 0;
  }

  get isRecording(): boolean {
    return this.stream !== null;
  }
}
