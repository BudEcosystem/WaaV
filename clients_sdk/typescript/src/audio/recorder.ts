/**
 * Audio Recorder
 * Records audio from microphone using Web Audio API
 */

import { AudioProcessor } from './processor.js';

/**
 * Recorder configuration
 */
export interface RecorderConfig {
  /** Target sample rate for output (default: 16000) */
  sampleRate?: number;
  /** Number of channels (default: 1 for mono) */
  channels?: number;
  /** Buffer size in samples (default: 4096) */
  bufferSize?: number;
  /** Whether to request echo cancellation (default: true) */
  echoCancellation?: boolean;
  /** Whether to request noise suppression (default: true) */
  noiseSuppression?: boolean;
  /** Whether to request auto gain control (default: true) */
  autoGainControl?: boolean;
  /** Device ID to use (default: system default) */
  deviceId?: string;
  /**
   * URL of the standalone capture AudioWorklet module (built to
   * `dist/capture.worklet.js`). When omitted, the recorder tries to derive it
   * from the package location (`import.meta.url`); if that fails or the runtime
   * lacks AudioWorklet, it falls back to the legacy ScriptProcessor path. Pass
   * an explicit URL when your bundler relocates assets.
   */
  workletUrl?: string;
  /**
   * Force the legacy ScriptProcessor capture path even when AudioWorklet is
   * available (default false). The worklet is preferred because it runs off the
   * main thread; this escape hatch is for debugging only.
   */
  forceScriptProcessor?: boolean;
}

/**
 * Recorder state
 */
export type RecorderState = 'idle' | 'recording' | 'paused' | 'stopped';

/**
 * Recorder event handlers
 */
export interface RecorderEventHandlers {
  /** Called when recording starts */
  onStart?: () => void;
  /** Called when recording pauses */
  onPause?: () => void;
  /** Called when recording resumes */
  onResume?: () => void;
  /** Called when recording stops */
  onStop?: () => void;
  /** Called with audio data (Int16 PCM) */
  onData?: (data: Int16Array) => void;
  /** Called with audio level (0-1) */
  onLevel?: (level: number) => void;
  /** Called on error */
  onError?: (error: Error) => void;
  /** Called when audio track ends (device disconnected) */
  onDeviceDisconnected?: () => void;
}

/**
 * Audio Recorder using Web Audio API
 */
export class AudioRecorder {
  private config: Required<RecorderConfig>;
  private state: RecorderState = 'idle';
  private handlers: RecorderEventHandlers = {};
  private audioContext: AudioContext | null = null;
  private mediaStream: MediaStream | null = null;
  private sourceNode: MediaStreamAudioSourceNode | null = null;
  private workletNode: AudioWorkletNode | null = null;
  private scriptProcessorNode: ScriptProcessorNode | null = null;
  private analyserNode: AnalyserNode | null = null;
  private levelCheckInterval: ReturnType<typeof setInterval> | null = null;
  private nativeSampleRate = 48000;
  /** Which capture path is live (for diagnostics + correct teardown). */
  private captureMode: 'worklet' | 'scriptprocessor' | null = null;

  constructor(config: RecorderConfig = {}) {
    this.config = {
      sampleRate: config.sampleRate ?? 16000,
      channels: config.channels ?? 1,
      bufferSize: config.bufferSize ?? 4096,
      echoCancellation: config.echoCancellation ?? true,
      noiseSuppression: config.noiseSuppression ?? true,
      autoGainControl: config.autoGainControl ?? true,
      deviceId: config.deviceId ?? '',
      workletUrl: config.workletUrl ?? '',
      forceScriptProcessor: config.forceScriptProcessor ?? false,
    };
  }

  /**
   * Set event handlers
   */
  setHandlers(handlers: RecorderEventHandlers): void {
    this.handlers = { ...this.handlers, ...handlers };
  }

  /**
   * Get current state
   */
  getState(): RecorderState {
    return this.state;
  }

  /**
   * Get list of available audio input devices
   */
  static async getDevices(): Promise<MediaDeviceInfo[]> {
    const devices = await navigator.mediaDevices.enumerateDevices();
    return devices.filter((device) => device.kind === 'audioinput');
  }

  /**
   * Request microphone permission
   */
  static async requestPermission(): Promise<boolean> {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      stream.getTracks().forEach((track) => track.stop());
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Initialize audio context and get media stream
   */
  private async initialize(): Promise<void> {
    if (this.audioContext && this.mediaStream) return;

    try {
      // Get media stream
      const constraints: MediaStreamConstraints = {
        audio: {
          echoCancellation: this.config.echoCancellation,
          noiseSuppression: this.config.noiseSuppression,
          autoGainControl: this.config.autoGainControl,
          channelCount: this.config.channels,
          ...(this.config.deviceId ? { deviceId: { exact: this.config.deviceId } } : {}),
        },
      };

      this.mediaStream = await navigator.mediaDevices.getUserMedia(constraints);

      // Create audio context
      this.audioContext = new AudioContext();
      this.nativeSampleRate = this.audioContext.sampleRate;

      // Create source node
      this.sourceNode = this.audioContext.createMediaStreamSource(this.mediaStream);

      // Create analyser for level monitoring
      this.analyserNode = this.audioContext.createAnalyser();
      this.analyserNode.fftSize = 256;
      this.sourceNode.connect(this.analyserNode);

      // D3: prefer the off-main-thread AudioWorklet capture; fall back to the
      // legacy ScriptProcessor only when the worklet is unavailable.
      await this.setupCapture();

      // Monitor track ended
      const track = this.mediaStream.getAudioTracks()[0];
      if (track) {
        track.onended = () => {
          this.handlers.onDeviceDisconnected?.();
          this.stop();
        };
      }
    } catch (err) {
      const error = err instanceof Error ? err : new Error(String(err));
      this.handlers.onError?.(error);
      throw error;
    }
  }

  /**
   * D3: set up audio capture, preferring the off-main-thread AudioWorklet and
   * falling back to the legacy ScriptProcessor only if the worklet path is
   * unavailable (no AudioWorklet, no resolvable module URL, or addModule fails).
   */
  private async setupCapture(): Promise<void> {
    if (!this.config.forceScriptProcessor && this.canUseWorklet()) {
      try {
        await this.setupWorklet();
        this.captureMode = 'worklet';
        return;
      } catch {
        // Worklet unavailable/failed (e.g. addModule URL not resolvable in this
        // bundler) — fall through to the legacy path. Reset any partial state.
        if (this.workletNode) {
          try { this.workletNode.disconnect(); } catch { /* ignore */ }
          this.workletNode = null;
        }
      }
    }
    this.setupScriptProcessor();
    this.captureMode = 'scriptprocessor';
  }

  /** Whether this runtime can run an AudioWorklet capture node. */
  private canUseWorklet(): boolean {
    return (
      typeof AudioWorkletNode !== 'undefined' &&
      !!this.audioContext &&
      'audioWorklet' in this.audioContext &&
      typeof this.audioContext.audioWorklet?.addModule === 'function'
    );
  }

  /** Resolve the worklet module URL (explicit config, else derive from package). */
  private resolveWorkletUrl(): string | null {
    if (this.config.workletUrl) return this.config.workletUrl;
    // Derive the standalone worklet asset that sits next to the built bundle.
    // `import.meta.url` points at this module's location; the worklet is emitted
    // as a sibling `capture.worklet.js`. Guarded because some toolchains rewrite
    // import.meta — on failure we return null and the caller falls back.
    try {
      const base = (import.meta as unknown as { url?: string }).url;
      if (base) return new URL('./capture.worklet.js', base).href;
    } catch {
      // ignore — fall through
    }
    return null;
  }

  /**
   * Set up the AudioWorklet capture path: load the module, create the node,
   * configure it (target rate + 20ms frames), and pump Int16 frames up to
   * onData. The worklet posts TRANSFERABLE buffers; we send them back via a
   * 'recycle' message so the worklet's process() stays allocation-free.
   */
  private async setupWorklet(): Promise<void> {
    if (!this.audioContext || !this.sourceNode) throw new Error('audio context not ready');
    const url = this.resolveWorkletUrl();
    if (!url) throw new Error('worklet URL unresolved');

    await this.audioContext.audioWorklet.addModule(url);

    const node = new AudioWorkletNode(this.audioContext, 'waav-capture', {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      channelCount: 1,
    });
    this.workletNode = node;

    node.port.onmessage = (e: MessageEvent) => {
      const data = e.data as { type?: string; buffer?: ArrayBuffer; samples?: number };
      if (data?.type === 'frame' && data.buffer && this.state === 'recording') {
        const samples = data.samples ?? data.buffer.byteLength / 2;
        // Wrap the transferred buffer as Int16 (zero-copy) for the consumer.
        const frame = new Int16Array(data.buffer, 0, samples);
        this.handlers.onData?.(frame);
        // Return the buffer to the worklet pool to avoid per-frame allocation.
        try {
          node.port.postMessage({ type: 'recycle', buffer: data.buffer }, [data.buffer]);
        } catch {
          // If the consumer retained the buffer, just let the worklet allocate.
        }
      }
    };

    // Tell the worklet the target rate + frame size (20ms @ target rate).
    node.port.postMessage({ type: 'config', targetSampleRate: this.config.sampleRate, frameMs: 20 });

    // Connect capture graph. A muted sink keeps the node pulled without audible
    // output (some browsers require an output connection to run the processor).
    this.sourceNode.connect(node);
    const mute = this.audioContext.createGain();
    mute.gain.value = 0;
    node.connect(mute);
    mute.connect(this.audioContext.destination);
  }

  /**
   * Legacy fallback: ScriptProcessorNode capture (deprecated, main-thread).
   * Retained only for runtimes without AudioWorklet. Frames are resampled +
   * Int16-converted on the main thread (the very contention the worklet avoids).
   */
  private setupScriptProcessor(): void {
    if (!this.audioContext || !this.sourceNode) return;

    const processor = this.audioContext.createScriptProcessor(this.config.bufferSize, 1, 1);
    this.scriptProcessorNode = processor;

    processor.onaudioprocess = (event) => {
      if (this.state !== 'recording') return;

      const inputData = event.inputBuffer.getChannelData(0);

      // Resample if needed
      let outputData: Float32Array;
      if (this.nativeSampleRate !== this.config.sampleRate) {
        outputData = AudioProcessor.resample(inputData, this.nativeSampleRate, this.config.sampleRate);
      } else {
        outputData = new Float32Array(inputData);
      }

      // Convert to Int16 for transmission
      const int16Data = AudioProcessor.float32ToInt16(outputData);
      this.handlers.onData?.(int16Data);
    };

    this.sourceNode.connect(processor);
    processor.connect(this.audioContext.destination);
  }

  /** Which capture path is active ('worklet' | 'scriptprocessor' | null). */
  getCaptureMode(): 'worklet' | 'scriptprocessor' | null {
    return this.captureMode;
  }

  /**
   * Start level monitoring
   */
  private startLevelMonitoring(): void {
    if (!this.analyserNode || !this.handlers.onLevel) return;

    const dataArray = new Uint8Array(this.analyserNode.frequencyBinCount);

    this.levelCheckInterval = setInterval(() => {
      if (this.state !== 'recording' || !this.analyserNode) return;

      this.analyserNode.getByteFrequencyData(dataArray);

      // Calculate average level
      let sum = 0;
      for (let i = 0; i < dataArray.length; i++) {
        sum += dataArray[i]!;
      }
      const average = sum / dataArray.length / 255;

      this.handlers.onLevel?.(average);
    }, 100);
  }

  /**
   * Stop level monitoring
   */
  private stopLevelMonitoring(): void {
    if (this.levelCheckInterval) {
      clearInterval(this.levelCheckInterval);
      this.levelCheckInterval = null;
    }
  }

  /**
   * Start recording
   */
  async start(): Promise<void> {
    if (this.state === 'recording') return;

    await this.initialize();

    if (this.audioContext?.state === 'suspended') {
      await this.audioContext.resume();
    }

    this.state = 'recording';
    this.startLevelMonitoring();
    this.handlers.onStart?.();
  }

  /**
   * Pause recording
   */
  pause(): void {
    if (this.state !== 'recording') return;

    this.state = 'paused';
    this.stopLevelMonitoring();
    this.handlers.onPause?.();
  }

  /**
   * Resume recording
   */
  resume(): void {
    if (this.state !== 'paused') return;

    this.state = 'recording';
    this.startLevelMonitoring();
    this.handlers.onResume?.();
  }

  /**
   * Stop recording
   */
  stop(): void {
    if (this.state === 'stopped' || this.state === 'idle') return;

    this.state = 'stopped';
    this.stopLevelMonitoring();
    this.handlers.onStop?.();
  }

  /**
   * Dispose all resources
   */
  async dispose(): Promise<void> {
    this.stop();

    if (this.workletNode) {
      try { this.workletNode.port.onmessage = null; } catch { /* ignore */ }
      this.workletNode.disconnect();
      this.workletNode = null;
    }

    if (this.scriptProcessorNode) {
      this.scriptProcessorNode.onaudioprocess = null;
      this.scriptProcessorNode.disconnect();
      this.scriptProcessorNode = null;
    }

    this.captureMode = null;

    if (this.analyserNode) {
      this.analyserNode.disconnect();
      this.analyserNode = null;
    }

    if (this.sourceNode) {
      this.sourceNode.disconnect();
      this.sourceNode = null;
    }

    if (this.mediaStream) {
      this.mediaStream.getTracks().forEach((track) => track.stop());
      this.mediaStream = null;
    }

    if (this.audioContext) {
      await this.audioContext.close();
      this.audioContext = null;
    }

    this.state = 'idle';
  }

  /**
   * Get native sample rate
   */
  getNativeSampleRate(): number {
    return this.nativeSampleRate;
  }

  /**
   * Get target sample rate
   */
  getTargetSampleRate(): number {
    return this.config.sampleRate;
  }
}

/**
 * Create an audio recorder instance
 */
export function createRecorder(sampleRate = 16000, config?: Omit<RecorderConfig, 'sampleRate'>): AudioRecorder {
  return new AudioRecorder({ ...config, sampleRate });
}
