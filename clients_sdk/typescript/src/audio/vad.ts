/**
 * Voice Activity Detection (VAD)
 * Simple energy-based VAD for detecting speech
 */

import { AudioProcessor } from './processor.js';

/**
 * VAD configuration
 */
export interface VADConfig {
  /** Sample rate of input audio (default: 16000) */
  sampleRate?: number;
  /** Frame size in milliseconds (default: 30) */
  frameSizeMs?: number;
  /** Energy threshold in dB for speech detection (default: -35) */
  energyThresholdDb?: number;
  /** Minimum speech duration in ms to trigger (default: 250) */
  minSpeechDurationMs?: number;
  /** Minimum silence duration in ms to end speech (default: 500) */
  minSilenceDurationMs?: number;
  /** Smoothing factor for energy calculation (default: 0.95) */
  smoothingFactor?: number;
  /** Pre-speech buffer duration in ms (default: 300) */
  preSpeechBufferMs?: number;
}

/**
 * VAD state
 */
export type VADState = 'silence' | 'speech' | 'uncertain';

/**
 * VAD event
 */
export interface VADEvent {
  /** Current state */
  state: VADState;
  /** Whether speech started */
  speechStart: boolean;
  /** Whether speech ended */
  speechEnd: boolean;
  /** Current energy level in dB */
  energyDb: number;
  /** Duration of current state in ms */
  stateDurationMs: number;
  /** Pre-speech buffer if speech started */
  preSpeechBuffer?: Float32Array;
}

/**
 * VAD event handlers
 */
export interface VADEventHandlers {
  /** Called when speech starts */
  onSpeechStart?: (preSpeechBuffer: Float32Array) => void;
  /** Called when speech ends */
  onSpeechEnd?: (speechDurationMs: number) => void;
  /** Called on every frame with VAD state */
  onFrame?: (event: VADEvent) => void;
}

/**
 * Voice Activity Detection
 */
export class VAD {
  private config: Required<VADConfig>;
  private handlers: VADEventHandlers = {};
  private state: VADState = 'silence';
  private smoothedEnergy = 0;
  private speechStartTime: number | null = null;
  private silenceStartTime: number | null = null;
  private stateStartTime: number = Date.now();
  private preSpeechBuffer: Float32Array[] = [];
  private preSpeechBufferSamples = 0;
  private maxPreSpeechSamples: number;
  private frameSizeSamples: number;

  constructor(config: VADConfig = {}) {
    this.config = {
      sampleRate: config.sampleRate ?? 16000,
      frameSizeMs: config.frameSizeMs ?? 30,
      energyThresholdDb: config.energyThresholdDb ?? -35,
      minSpeechDurationMs: config.minSpeechDurationMs ?? 250,
      minSilenceDurationMs: config.minSilenceDurationMs ?? 500,
      smoothingFactor: config.smoothingFactor ?? 0.95,
      preSpeechBufferMs: config.preSpeechBufferMs ?? 300,
    };

    this.frameSizeSamples = Math.floor((this.config.sampleRate * this.config.frameSizeMs) / 1000);
    this.maxPreSpeechSamples = Math.floor((this.config.sampleRate * this.config.preSpeechBufferMs) / 1000);
  }

  /**
   * Set event handlers
   */
  setHandlers(handlers: VADEventHandlers): void {
    this.handlers = { ...this.handlers, ...handlers };
  }

  /**
   * Get current state
   */
  getState(): VADState {
    return this.state;
  }

  /**
   * Process audio frame
   * @param audio Float32Array audio data
   * @returns VAD event
   */
  processFrame(audio: Float32Array): VADEvent {
    // Calculate frame energy
    const rms = AudioProcessor.calculateRMS(audio);
    const energyDb = AudioProcessor.linearToDb(rms);

    // Apply smoothing
    this.smoothedEnergy = this.config.smoothingFactor * this.smoothedEnergy +
                          (1 - this.config.smoothingFactor) * rms;
    const smoothedEnergyDb = AudioProcessor.linearToDb(this.smoothedEnergy);

    // Determine if this frame is speech or silence
    const isSpeechFrame = smoothedEnergyDb > this.config.energyThresholdDb;

    const now = Date.now();
    let speechStart = false;
    let speechEnd = false;
    const previousState = this.state;

    // State machine
    switch (this.state) {
      case 'silence':
        // Add to pre-speech buffer
        this.addToPreSpeechBuffer(audio);

        if (isSpeechFrame) {
          this.speechStartTime = now;
          this.state = 'uncertain';
          this.stateStartTime = now;
        }
        break;

      case 'uncertain':
        // Add to pre-speech buffer while uncertain
        this.addToPreSpeechBuffer(audio);

        if (isSpeechFrame) {
          const speechDuration = now - (this.speechStartTime ?? now);
          if (speechDuration >= this.config.minSpeechDurationMs) {
            // Confirmed speech
            this.state = 'speech';
            this.stateStartTime = now;
            speechStart = true;

            // Emit speech start with buffered audio
            const preSpeechBuffer = this.getPreSpeechBuffer();
            this.handlers.onSpeechStart?.(preSpeechBuffer);
            this.clearPreSpeechBuffer();
          }
        } else {
          // Returned to silence before min duration
          this.state = 'silence';
          this.stateStartTime = now;
          this.speechStartTime = null;
        }
        break;

      case 'speech':
        if (!isSpeechFrame) {
          if (!this.silenceStartTime) {
            this.silenceStartTime = now;
          }

          const silenceDuration = now - this.silenceStartTime;
          if (silenceDuration >= this.config.minSilenceDurationMs) {
            // End of speech
            this.state = 'silence';
            this.stateStartTime = now;
            speechEnd = true;

            const speechDuration = this.silenceStartTime - (this.speechStartTime ?? this.silenceStartTime);
            this.handlers.onSpeechEnd?.(speechDuration);

            this.speechStartTime = null;
            this.silenceStartTime = null;
          }
        } else {
          this.silenceStartTime = null;
        }
        break;
    }

    const event: VADEvent = {
      state: this.state,
      speechStart,
      speechEnd,
      energyDb: smoothedEnergyDb,
      stateDurationMs: now - this.stateStartTime,
    };

    if (speechStart) {
      event.preSpeechBuffer = this.getPreSpeechBuffer();
    }

    this.handlers.onFrame?.(event);

    return event;
  }

  /**
   * Process audio buffer (splits into frames)
   * @param audio Audio data
   * @returns Array of VAD events
   */
  process(audio: Float32Array): VADEvent[] {
    const events: VADEvent[] = [];
    let offset = 0;

    while (offset + this.frameSizeSamples <= audio.length) {
      const frame = audio.subarray(offset, offset + this.frameSizeSamples);
      events.push(this.processFrame(frame));
      offset += this.frameSizeSamples;
    }

    return events;
  }

  /**
   * Add audio to pre-speech buffer
   */
  private addToPreSpeechBuffer(audio: Float32Array): void {
    this.preSpeechBuffer.push(new Float32Array(audio));
    this.preSpeechBufferSamples += audio.length;

    // Remove old samples if buffer is full
    while (this.preSpeechBufferSamples > this.maxPreSpeechSamples && this.preSpeechBuffer.length > 0) {
      const removed = this.preSpeechBuffer.shift();
      if (removed) {
        this.preSpeechBufferSamples -= removed.length;
      }
    }
  }

  /**
   * Get concatenated pre-speech buffer
   */
  private getPreSpeechBuffer(): Float32Array {
    if (this.preSpeechBuffer.length === 0) {
      return new Float32Array(0);
    }
    return AudioProcessor.concatenate(this.preSpeechBuffer);
  }

  /**
   * Clear pre-speech buffer
   */
  private clearPreSpeechBuffer(): void {
    this.preSpeechBuffer = [];
    this.preSpeechBufferSamples = 0;
  }

  /**
   * Reset VAD state
   */
  reset(): void {
    this.state = 'silence';
    this.smoothedEnergy = 0;
    this.speechStartTime = null;
    this.silenceStartTime = null;
    this.stateStartTime = Date.now();
    this.clearPreSpeechBuffer();
  }

  /**
   * Update threshold dynamically
   * @param thresholdDb New threshold in dB
   */
  setThreshold(thresholdDb: number): void {
    this.config.energyThresholdDb = thresholdDb;
  }

  /**
   * Get current threshold
   */
  getThreshold(): number {
    return this.config.energyThresholdDb;
  }

  /**
   * Check if currently in speech state
   */
  isSpeaking(): boolean {
    return this.state === 'speech';
  }
}

/**
 * Create a VAD instance
 */
export function createVAD(sampleRate = 16000, config?: Omit<VADConfig, 'sampleRate'>): VAD {
  return new VAD({ ...config, sampleRate });
}
