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
  private analyserNode: AnalyserNode | null = null;
  private levelCheckInterval: ReturnType<typeof setInterval> | null = null;
  private nativeSampleRate = 48000;

  constructor(config: RecorderConfig = {}) {
    this.config = {
      sampleRate: config.sampleRate ?? 16000,
      channels: config.channels ?? 1,
      bufferSize: config.bufferSize ?? 4096,
      echoCancellation: config.echoCancellation ?? true,
      noiseSuppression: config.noiseSuppression ?? true,
      autoGainControl: config.autoGainControl ?? true,
      deviceId: config.deviceId ?? '',
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

      // Create processor using ScriptProcessorNode (deprecated but widely supported)
      // TODO: Upgrade to AudioWorklet when better cross-browser support
      await this.setupScriptProcessor();

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
   * Setup script processor for audio capture
   */
  private async setupScriptProcessor(): Promise<void> {
    if (!this.audioContext || !this.sourceNode) return;

    // Use ScriptProcessorNode (deprecated but reliable)
    const processor = this.audioContext.createScriptProcessor(this.config.bufferSize, 1, 1);

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
      this.workletNode.disconnect();
      this.workletNode = null;
    }

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
