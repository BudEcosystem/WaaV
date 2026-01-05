/**
 * BudSTT Pipeline
 * Speech-to-Text pipeline for real-time transcription
 */

import type { STTConfig } from '../types/config.js';
import type { FeatureFlags } from '../types/features.js';
import type { TranscriptEvent } from '../ws/events.js';
import { BasePipeline, type BasePipelineConfig } from './base.js';
import { AudioRecorder, type RecorderConfig } from '../audio/recorder.js';
import { VAD, type VADConfig } from '../audio/vad.js';

/**
 * BudSTT configuration
 */
export interface BudSTTConfig extends BasePipelineConfig {
  /** STT provider (e.g., 'deepgram', 'whisper', 'azure') */
  provider?: string;
  /** Language code (e.g., 'en-US') */
  language?: string;
  /** Model to use (e.g., 'nova-3' for Deepgram) */
  model?: string;
  /** Sample rate of input audio (default: 16000) */
  sampleRate?: number;
  /** Audio encoding (default: 'linear16') */
  encoding?: string;
  /** Number of audio channels (default: 1) */
  channels?: number;
  /** Enable interim results (default: true) */
  interimResults?: boolean;
  /** Enable punctuation (default: true) */
  punctuate?: boolean;
  /** Enable profanity filter (default: false) */
  profanityFilter?: boolean;
  /** Enable smart formatting (default: true) */
  smartFormat?: boolean;
  /** Enable speaker diarization (default: false) */
  diarize?: boolean;
  /** Keywords to boost recognition */
  keywords?: string[];
  /** Custom vocabulary */
  customVocabulary?: string[];
  /** VAD configuration */
  vadConfig?: VADConfig;
  /** Recorder configuration */
  recorderConfig?: RecorderConfig;
}

/**
 * BudSTT - Speech-to-Text Pipeline
 */
export class BudSTT extends BasePipeline {
  private sttConfig: STTConfig;
  private recorder: AudioRecorder | null = null;
  private vad: VAD | null = null;
  private vadEnabled: boolean;
  private isListening = false;

  constructor(config: BudSTTConfig) {
    const sttConfig: STTConfig = {
      provider: config.provider ?? 'deepgram',
      language: config.language ?? 'en-US',
      model: config.model,
      sampleRate: config.sampleRate ?? 16000,
      encoding: config.encoding ?? 'linear16',
      channels: config.channels ?? 1,
      interimResults: config.interimResults ?? true,
      punctuate: config.punctuate ?? true,
      profanityFilter: config.profanityFilter ?? false,
      smartFormat: config.smartFormat ?? true,
      diarize: config.diarize ?? false,
      keywords: config.keywords,
      customVocabulary: config.customVocabulary,
    };

    super({
      ...config,
      sessionConfig: {
        stt: sttConfig,
      },
    });

    this.sttConfig = sttConfig;
    this.vadEnabled = config.features?.vad ?? false;

    // Setup VAD if enabled
    if (this.vadEnabled) {
      this.vad = new VAD({
        sampleRate: sttConfig.sampleRate,
        ...config.vadConfig,
      });

      this.vad.setHandlers({
        onSpeechStart: (buffer) => this.handleSpeechStart(buffer),
        onSpeechEnd: (duration) => this.handleSpeechEnd(duration),
      });
    }

    // Setup transcript forwarding
    this.session.on('transcript', (e) => this.emitter.emit('transcript', e));
    this.session.on('listening', (e) => this.emitter.emit('listening', e));
  }

  /**
   * Handle VAD speech start
   */
  private handleSpeechStart(buffer: Float32Array): void {
    this.emitter.emit('listening', { listening: true, timestamp: Date.now() });

    // Send buffered pre-speech audio
    if (buffer.length > 0) {
      const int16 = new Int16Array(buffer.length);
      for (let i = 0; i < buffer.length; i++) {
        const sample = Math.max(-1, Math.min(1, buffer[i]!));
        int16[i] = sample < 0 ? sample * 32768 : sample * 32767;
      }
      this.session.sendAudio(int16.buffer);
    }
  }

  /**
   * Handle VAD speech end
   */
  private handleSpeechEnd(_duration: number): void {
    this.emitter.emit('listening', { listening: false, timestamp: Date.now() });
    this.session.flush();
  }

  /**
   * Send audio data for transcription
   * @param audio Int16 PCM audio data
   */
  sendAudio(audio: ArrayBuffer | Int16Array | Uint8Array): void {
    if (!this.isConnected()) {
      throw new Error('Not connected. Call connect() first.');
    }

    const buffer = audio instanceof Int16Array
      ? audio.buffer as ArrayBuffer
      : audio instanceof Uint8Array
        ? audio.buffer as ArrayBuffer
        : audio;

    // If VAD is enabled, process through VAD first
    if (this.vad) {
      const int16 = new Int16Array(buffer);
      const float32 = new Float32Array(int16.length);
      for (let i = 0; i < int16.length; i++) {
        float32[i] = int16[i]! / 32768;
      }

      const events = this.vad.process(float32);

      // Only send audio during speech
      for (const event of events) {
        if (event.state === 'speech') {
          this.session.sendAudio(buffer);
          break;
        }
      }
    } else {
      // No VAD, send all audio
      this.session.sendAudio(buffer);
    }
  }

  /**
   * Start listening from microphone
   * @param recorderConfig Optional recorder configuration
   */
  async startListening(recorderConfig?: RecorderConfig): Promise<void> {
    if (this.isListening) return;

    if (!this.isConnected()) {
      await this.connect();
    }

    this.recorder = new AudioRecorder({
      sampleRate: this.sttConfig.sampleRate,
      ...recorderConfig,
    });

    this.recorder.setHandlers({
      onData: (data) => this.sendAudio(data),
      onLevel: (level) => {
        // Could emit level events if needed
      },
      onError: (error) => {
        this.emitter.emit('error', {
          code: 'RECORDER_ERROR',
          message: error.message,
          recoverable: true,
          raw: { type: 'error', code: 'RECORDER_ERROR', message: error.message },
        });
      },
    });

    await this.recorder.start();
    this.isListening = true;
  }

  /**
   * Stop listening from microphone
   */
  stopListening(): void {
    if (!this.isListening || !this.recorder) return;

    this.recorder.stop();
    this.isListening = false;

    // Flush any remaining audio
    this.session.flush();
  }

  /**
   * Pause listening
   */
  pauseListening(): void {
    this.recorder?.pause();
  }

  /**
   * Resume listening
   */
  resumeListening(): void {
    this.recorder?.resume();
  }

  /**
   * Check if currently listening
   */
  getIsListening(): boolean {
    return this.isListening;
  }

  /**
   * Update STT configuration
   */
  updateConfig(config: Partial<STTConfig>): void {
    this.sttConfig = { ...this.sttConfig, ...config };
    this.session.updateSTTConfig(config);
  }

  /**
   * Set VAD threshold
   */
  setVADThreshold(thresholdDb: number): void {
    this.vad?.setThreshold(thresholdDb);
  }

  /**
   * Enable/disable VAD
   */
  setVADEnabled(enabled: boolean): void {
    this.vadEnabled = enabled;
    if (enabled && !this.vad) {
      this.vad = new VAD({ sampleRate: this.sttConfig.sampleRate });
      this.vad.setHandlers({
        onSpeechStart: (buffer) => this.handleSpeechStart(buffer),
        onSpeechEnd: (duration) => this.handleSpeechEnd(duration),
      });
    }
  }

  /**
   * Flush pending audio and finalize transcription
   */
  flush(): void {
    this.session.flush();
  }

  /**
   * Dispose resources
   */
  async dispose(): Promise<void> {
    this.stopListening();
    if (this.recorder) {
      await this.recorder.dispose();
      this.recorder = null;
    }
    await this.disconnect();
  }
}

/**
 * Create a BudSTT instance
 */
export function createBudSTT(config: BudSTTConfig): BudSTT {
  return new BudSTT(config);
}
