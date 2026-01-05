/**
 * PCM Audio Player
 * Plays PCM audio data using Web Audio API
 */

import { AudioProcessor } from './processor.js';

/**
 * Player configuration
 */
export interface PlayerConfig {
  /** Sample rate of input PCM (default: 24000) */
  sampleRate?: number;
  /** Number of channels (default: 1) */
  channels?: number;
  /** Buffer size in samples (default: 4096) */
  bufferSize?: number;
  /** Maximum buffer duration in seconds (default: 2.0) */
  maxBufferDuration?: number;
  /** Gain in dB (default: 0) */
  gainDb?: number;
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
}

/**
 * PCM Audio Player using Web Audio API
 */
export class PCMPlayer {
  private config: Required<PlayerConfig>;
  private audioContext: AudioContext | null = null;
  private gainNode: GainNode | null = null;
  private state: PlayerState = 'idle';
  private handlers: PlayerEventHandlers = {};
  private bufferQueue: Float32Array[] = [];
  private currentSource: AudioBufferSourceNode | null = null;
  private playbackStartTime = 0;
  private totalSamplesPlayed = 0;
  private isProcessing = false;

  constructor(config: PlayerConfig = {}) {
    this.config = {
      sampleRate: config.sampleRate ?? 24000,
      channels: config.channels ?? 1,
      bufferSize: config.bufferSize ?? 4096,
      maxBufferDuration: config.maxBufferDuration ?? 2.0,
      gainDb: config.gainDb ?? 0,
    };
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
   * Get audio context sample rate
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
   * Get buffered duration in seconds
   */
  getBufferedDuration(): number {
    const totalSamples = this.bufferQueue.reduce((sum, buf) => sum + buf.length, 0);
    return totalSamples / this.config.sampleRate;
  }

  /**
   * Check if buffer has space
   */
  hasBufferSpace(): boolean {
    return this.getBufferedDuration() < this.config.maxBufferDuration;
  }

  /**
   * Add PCM data to buffer (Int16 format)
   * @param pcmData Int16 PCM data
   */
  addPCM(pcmData: ArrayBuffer | Int16Array): void {
    const int16 = pcmData instanceof Int16Array ? pcmData : new Int16Array(pcmData);
    const float32 = AudioProcessor.int16ToFloat32(int16);
    this.addFloat32(float32);
  }

  /**
   * Add Float32 audio to buffer
   * @param audioData Float32 audio data
   */
  addFloat32(audioData: Float32Array): void {
    // Check if buffer is full
    if (!this.hasBufferSpace()) {
      this.handlers.onBufferFull?.();
      // Drop oldest data to make room
      while (!this.hasBufferSpace() && this.bufferQueue.length > 0) {
        this.bufferQueue.shift();
      }
    }

    this.bufferQueue.push(audioData);

    // Start playback if idle
    if (this.state === 'idle' && this.bufferQueue.length > 0) {
      this.processQueue();
    }
  }

  /**
   * Process buffer queue and schedule playback
   */
  private async processQueue(): Promise<void> {
    if (this.isProcessing || this.state === 'paused' || this.state === 'stopped') {
      return;
    }

    if (!this.audioContext || !this.gainNode) {
      await this.initialize();
    }

    if (this.bufferQueue.length === 0) {
      if (this.state === 'playing') {
        this.handlers.onBufferEmpty?.();
      }
      this.state = 'idle';
      return;
    }

    this.isProcessing = true;

    try {
      // Resume audio context if suspended
      if (this.audioContext!.state === 'suspended') {
        await this.audioContext!.resume();
      }

      // Concatenate buffered audio
      const audioData = AudioProcessor.concatenate(this.bufferQueue);
      this.bufferQueue = [];

      // Create audio buffer
      const audioBuffer = this.audioContext!.createBuffer(
        this.config.channels,
        audioData.length,
        this.config.sampleRate
      );
      audioBuffer.getChannelData(0).set(audioData);

      // Create source node
      const source = this.audioContext!.createBufferSource();
      source.buffer = audioBuffer;
      source.connect(this.gainNode!);

      // Track current source for stopping
      this.currentSource = source;

      // Handle playback end
      source.onended = () => {
        this.totalSamplesPlayed += audioData.length;
        this.currentSource = null;
        this.isProcessing = false;

        // Process more data if available
        if (this.bufferQueue.length > 0 && this.state !== 'stopped') {
          this.processQueue();
        } else if (this.state === 'playing') {
          this.handlers.onBufferEmpty?.();
          this.state = 'idle';
        }
      };

      // Start playback
      if (this.state !== 'playing') {
        this.state = 'playing';
        this.playbackStartTime = this.audioContext!.currentTime;
        this.handlers.onPlay?.();
      }

      source.start();

      // Report progress
      const currentTime = this.totalSamplesPlayed / this.config.sampleRate;
      const bufferedTime = currentTime + audioData.length / this.config.sampleRate;
      this.handlers.onProgress?.(currentTime, bufferedTime);
    } catch (err) {
      this.isProcessing = false;
      const error = err instanceof Error ? err : new Error(String(err));
      this.handlers.onError?.(error);
    }
  }

  /**
   * Start/resume playback
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
      this.state = 'playing';
      this.handlers.onPlay?.();
      this.processQueue();
    } else if (this.bufferQueue.length > 0) {
      this.processQueue();
    }
  }

  /**
   * Pause playback
   */
  pause(): void {
    if (this.state !== 'playing') return;

    this.state = 'paused';
    this.handlers.onPause?.();

    if (this.currentSource) {
      try {
        this.currentSource.stop();
      } catch {
        // Ignore if already stopped
      }
      this.currentSource = null;
    }
  }

  /**
   * Stop playback and clear buffer
   */
  stop(): void {
    this.state = 'stopped';
    this.handlers.onStop?.();

    if (this.currentSource) {
      try {
        this.currentSource.stop();
      } catch {
        // Ignore if already stopped
      }
      this.currentSource = null;
    }

    this.bufferQueue = [];
    this.totalSamplesPlayed = 0;
    this.isProcessing = false;
  }

  /**
   * Clear buffer without stopping
   */
  clearBuffer(): void {
    this.bufferQueue = [];
  }

  /**
   * Get current playback time in seconds
   */
  getCurrentTime(): number {
    return this.totalSamplesPlayed / this.config.sampleRate;
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
