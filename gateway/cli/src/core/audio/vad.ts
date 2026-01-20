/**
 * WaaV CLI Voice Activity Detection
 *
 * Simple energy-based VAD for detecting speech in audio
 */

import { EventEmitter } from 'events';
import { logger } from '../../utils/logger.js';

/**
 * VAD events
 */
export interface VADEvents {
  speechStart: (preSpeechBuffer: Buffer) => void;
  speechEnd: (duration: number) => void;
  speaking: () => void;
  silence: () => void;
}

/**
 * VAD configuration
 */
export interface VADConfig {
  sampleRate?: number;
  threshold?: number;
  speechPadMs?: number;
  silenceDurationMs?: number;
  preSpeechBufferMs?: number;
  minSpeechDurationMs?: number;
}

/**
 * VAD state
 */
export type VADState = 'idle' | 'speech' | 'silence_after_speech';

/**
 * Voice Activity Detection
 *
 * Detects speech in audio using energy-based analysis
 */
export class VAD extends EventEmitter {
  private sampleRate: number;
  private threshold: number;
  private speechPadMs: number;
  private silenceDurationMs: number;
  private preSpeechBufferMs: number;
  private minSpeechDurationMs: number;

  private state: VADState = 'idle';
  private preSpeechBuffer: Buffer[] = [];
  private preSpeechBufferSize: number;
  private silenceStartTime = 0;
  private speechStartTime = 0;

  // Energy tracking
  private recentEnergies: number[] = [];
  private adaptiveThreshold: number;
  private noiseFloor = 0;

  constructor(config: VADConfig = {}) {
    super();
    this.sampleRate = config.sampleRate ?? 16000;
    this.threshold = config.threshold ?? 0.015; // Normalized threshold
    this.speechPadMs = config.speechPadMs ?? 300;
    this.silenceDurationMs = config.silenceDurationMs ?? 1000;
    this.preSpeechBufferMs = config.preSpeechBufferMs ?? 300;
    this.minSpeechDurationMs = config.minSpeechDurationMs ?? 100;

    // Calculate buffer sizes
    const bytesPerMs = (this.sampleRate * 2) / 1000; // 16-bit PCM
    this.preSpeechBufferSize = Math.ceil(bytesPerMs * this.preSpeechBufferMs);
    this.adaptiveThreshold = this.threshold;
  }

  /**
   * Get current state
   */
  getState(): VADState {
    return this.state;
  }

  /**
   * Check if currently in speech
   */
  isSpeaking(): boolean {
    return this.state === 'speech';
  }

  /**
   * Process audio buffer
   *
   * Returns true if speech is detected
   */
  process(buffer: Buffer): boolean {
    // Calculate RMS energy
    const energy = this.calculateEnergy(buffer);

    // Update energy history for adaptive threshold
    this.updateEnergyHistory(energy);

    // Determine if this is speech
    const isSpeech = this.detectSpeech(energy);

    // Update state machine
    this.updateState(isSpeech, buffer);

    return isSpeech;
  }

  /**
   * Calculate RMS energy from PCM buffer
   */
  private calculateEnergy(buffer: Buffer): number {
    let sumSquares = 0;
    const samples = buffer.length / 2; // 16-bit = 2 bytes per sample

    for (let i = 0; i < buffer.length; i += 2) {
      const sample = buffer.readInt16LE(i) / 32768; // Normalize to -1 to 1
      sumSquares += sample * sample;
    }

    return Math.sqrt(sumSquares / samples);
  }

  /**
   * Update energy history for adaptive threshold
   */
  private updateEnergyHistory(energy: number): void {
    this.recentEnergies.push(energy);

    // Keep history to ~1 second
    const maxHistory = Math.ceil(1000 / 20); // Assuming ~20ms frames
    if (this.recentEnergies.length > maxHistory) {
      this.recentEnergies.shift();
    }

    // Update noise floor estimate (lower 20th percentile)
    if (this.recentEnergies.length >= 10) {
      const sorted = [...this.recentEnergies].sort((a, b) => a - b);
      const percentileIdx = Math.floor(sorted.length * 0.2);
      this.noiseFloor = sorted[percentileIdx] ?? 0;

      // Adaptive threshold is noise floor + base threshold
      this.adaptiveThreshold = this.noiseFloor + this.threshold;
    }
  }

  /**
   * Detect speech based on energy
   */
  private detectSpeech(energy: number): boolean {
    // Use both absolute and adaptive thresholds
    const absoluteThreshold = this.threshold;
    const adaptiveThreshold = this.adaptiveThreshold;

    // Require energy to be above both thresholds
    return energy > absoluteThreshold && energy > adaptiveThreshold;
  }

  /**
   * Update VAD state machine
   */
  private updateState(isSpeech: boolean, buffer: Buffer): void {
    const now = Date.now();

    switch (this.state) {
      case 'idle':
        // Add to pre-speech buffer
        this.addToPreSpeechBuffer(buffer);

        if (isSpeech) {
          // Speech detected - start tracking
          this.speechStartTime = now;
          this.state = 'speech';
          this.emit('speaking');

          // Get pre-speech buffer for context
          const preSpeechData = this.getPreSpeechBuffer();
          this.emit('speechStart', preSpeechData);

          logger.debug('VAD: Speech started');
        }
        break;

      case 'speech':
        if (!isSpeech) {
          // Possible end of speech - wait for silence duration
          this.silenceStartTime = now;
          this.state = 'silence_after_speech';
        }
        break;

      case 'silence_after_speech':
        if (isSpeech) {
          // Speech resumed - go back to speech state
          this.state = 'speech';
          this.emit('speaking');
        } else {
          // Check if silence duration exceeded
          const silenceDuration = now - this.silenceStartTime;
          const speechDuration = this.silenceStartTime - this.speechStartTime;

          if (silenceDuration >= this.silenceDurationMs) {
            // Check minimum speech duration
            if (speechDuration >= this.minSpeechDurationMs) {
              this.state = 'idle';
              this.emit('speechEnd', speechDuration);
              this.emit('silence');
              logger.debug(`VAD: Speech ended (duration=${speechDuration}ms)`);
            } else {
              // Too short - ignore as noise
              this.state = 'idle';
              logger.debug(`VAD: Speech too short (${speechDuration}ms), ignoring`);
            }
          }
        }
        break;
    }
  }

  /**
   * Add buffer to pre-speech circular buffer
   */
  private addToPreSpeechBuffer(buffer: Buffer): void {
    this.preSpeechBuffer.push(buffer);

    // Calculate current buffer size
    const totalSize = this.preSpeechBuffer.reduce((sum, b) => sum + b.length, 0);

    // Trim if needed
    while (totalSize > this.preSpeechBufferSize && this.preSpeechBuffer.length > 1) {
      this.preSpeechBuffer.shift();
    }
  }

  /**
   * Get concatenated pre-speech buffer
   */
  private getPreSpeechBuffer(): Buffer {
    if (this.preSpeechBuffer.length === 0) {
      return Buffer.alloc(0);
    }
    return Buffer.concat(this.preSpeechBuffer);
  }

  /**
   * Reset VAD state
   */
  reset(): void {
    this.state = 'idle';
    this.preSpeechBuffer = [];
    this.silenceStartTime = 0;
    this.speechStartTime = 0;
    this.recentEnergies = [];
    this.noiseFloor = 0;
    this.adaptiveThreshold = this.threshold;
  }

  /**
   * Set threshold
   */
  setThreshold(threshold: number): void {
    this.threshold = Math.max(0, Math.min(1, threshold));
  }

  /**
   * Set silence duration
   */
  setSilenceDuration(ms: number): void {
    this.silenceDurationMs = Math.max(100, ms);
  }

  /**
   * Get configuration
   */
  getConfig(): VADConfig {
    return {
      sampleRate: this.sampleRate,
      threshold: this.threshold,
      speechPadMs: this.speechPadMs,
      silenceDurationMs: this.silenceDurationMs,
      preSpeechBufferMs: this.preSpeechBufferMs,
      minSpeechDurationMs: this.minSpeechDurationMs,
    };
  }

  /**
   * Get current energy statistics
   */
  getEnergyStats(): { current: number; noiseFloor: number; adaptiveThreshold: number } {
    return {
      current: this.recentEnergies[this.recentEnergies.length - 1] ?? 0,
      noiseFloor: this.noiseFloor,
      adaptiveThreshold: this.adaptiveThreshold,
    };
  }

  // Type-safe event methods
  on<K extends keyof VADEvents>(event: K, listener: VADEvents[K]): this {
    return super.on(event, listener as (...args: unknown[]) => void);
  }

  once<K extends keyof VADEvents>(event: K, listener: VADEvents[K]): this {
    return super.once(event, listener as (...args: unknown[]) => void);
  }

  emit<K extends keyof VADEvents>(event: K, ...args: Parameters<VADEvents[K]>): boolean {
    return super.emit(event, ...args);
  }

  off<K extends keyof VADEvents>(event: K, listener: VADEvents[K]): this {
    return super.off(event, listener as (...args: unknown[]) => void);
  }
}
