/**
 * BudTranscribe Pipeline
 * Batch transcription pipeline for files and streams
 */

import type { STTConfig } from '../types/config.js';
import type { FeatureFlags } from '../types/features.js';
import type { TranscriptEvent } from '../ws/events.js';
import { BasePipeline, type BasePipelineConfig } from './base.js';
import { AudioProcessor } from '../audio/processor.js';

/**
 * BudTranscribe configuration
 */
export interface BudTranscribeConfig extends BasePipelineConfig {
  /** STT provider (e.g., 'deepgram', 'whisper', 'azure') */
  provider?: string;
  /** Language code (e.g., 'en-US') */
  language?: string;
  /** Model to use */
  model?: string;
  /** Enable punctuation (default: true) */
  punctuate?: boolean;
  /** Enable speaker diarization (default: false) */
  diarize?: boolean;
  /** Enable word timestamps (default: false) */
  wordTimestamps?: boolean;
  /** Keywords to boost */
  keywords?: string[];
  /** Custom vocabulary */
  customVocabulary?: string[];
}

/**
 * Transcription result
 */
export interface TranscriptionResult {
  /** Full transcript text */
  text: string;
  /** Confidence score (0-1) */
  confidence: number;
  /** Detected language */
  language?: string;
  /** Audio duration in seconds */
  durationSeconds: number;
  /** Processing time in milliseconds */
  processingTimeMs: number;
  /** Word-level details */
  words?: Array<{
    word: string;
    start: number;
    end: number;
    confidence: number;
    speakerId?: number;
  }>;
  /** Speaker segments (if diarization enabled) */
  speakers?: Array<{
    speakerId: number;
    segments: Array<{
      start: number;
      end: number;
      text: string;
    }>;
  }>;
}

/**
 * Transcription progress event
 */
export interface TranscriptionProgress {
  /** Bytes processed */
  bytesProcessed: number;
  /** Total bytes (if known) */
  totalBytes?: number;
  /** Progress percentage (0-100) */
  percentage?: number;
  /** Interim transcript */
  interimText: string;
}

/**
 * BudTranscribe - Batch Transcription Pipeline
 */
export class BudTranscribe extends BasePipeline {
  private sttConfig: STTConfig;
  private transcripts: string[] = [];
  private words: Array<{ word: string; start: number; end: number; confidence?: number; speakerId?: number }> = [];
  private confidences: number[] = [];
  private isTranscribing = false;
  private startTime = 0;
  private bytesProcessed = 0;
  private totalBytes = 0;
  private interimText = '';
  private resolveTranscription: ((result: TranscriptionResult) => void) | null = null;
  private rejectTranscription: ((error: Error) => void) | null = null;

  constructor(config: BudTranscribeConfig) {
    const sttConfig: STTConfig = {
      provider: config.provider ?? 'deepgram',
      language: config.language ?? 'en-US',
      model: config.model,
      sampleRate: 16000,
      encoding: 'linear16',
      channels: 1,
      interimResults: true,
      punctuate: config.punctuate ?? true,
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
    this.setupTranscriptHandler();
  }

  /**
   * Setup transcript event handler
   */
  private setupTranscriptHandler(): void {
    this.session.on('transcript', (event) => {
      if (event.isFinal) {
        this.transcripts.push(event.text);
        if (event.confidence !== undefined) {
          this.confidences.push(event.confidence);
        }
        if (event.words) {
          this.words.push(...event.words);
        }
        this.interimText = '';
      } else {
        this.interimText = event.text;
      }

      // Emit progress
      this.emitter.emit('transcript', event);
    });

    this.session.on('error', (e) => {
      if (this.rejectTranscription) {
        this.rejectTranscription(new Error(e.message));
        this.rejectTranscription = null;
        this.resolveTranscription = null;
      }
    });
  }

  /**
   * Transcribe an audio file
   * @param file File or Blob containing audio data
   * @param options Optional transcription options
   */
  async transcribeFile(file: File | Blob, options?: {
    onProgress?: (progress: TranscriptionProgress) => void;
  }): Promise<TranscriptionResult> {
    if (this.isTranscribing) {
      throw new Error('Transcription already in progress');
    }

    this.resetState();
    this.isTranscribing = true;
    this.startTime = Date.now();
    this.totalBytes = file.size;

    if (!this.isConnected()) {
      await this.connect();
    }

    return new Promise((resolve, reject) => {
      this.resolveTranscription = resolve;
      this.rejectTranscription = reject;

      this.processFile(file, options?.onProgress).catch(reject);
    });
  }

  /**
   * Process file in chunks
   */
  private async processFile(file: File | Blob, onProgress?: (progress: TranscriptionProgress) => void): Promise<void> {
    const arrayBuffer = await file.arrayBuffer();
    const audioData = await this.decodeAudio(arrayBuffer);

    // Convert to Int16 for transmission
    const int16Data = AudioProcessor.float32ToInt16(audioData);
    const chunkSize = 16000; // 1 second at 16kHz

    for (let i = 0; i < int16Data.length; i += chunkSize) {
      const chunk = int16Data.slice(i, Math.min(i + chunkSize, int16Data.length));
      this.session.sendAudio(chunk.buffer);

      this.bytesProcessed = Math.min((i + chunkSize) * 2, this.totalBytes);

      if (onProgress) {
        onProgress({
          bytesProcessed: this.bytesProcessed,
          totalBytes: this.totalBytes,
          percentage: (this.bytesProcessed / this.totalBytes) * 100,
          interimText: this.interimText,
        });
      }

      // Small delay to not overwhelm the server
      await new Promise((r) => setTimeout(r, 10));
    }

    // Flush and wait for final results
    this.session.flush();

    // Wait a bit for final transcripts
    await new Promise((r) => setTimeout(r, 1000));

    this.finishTranscription(audioData.length / 16000);
  }

  /**
   * Decode audio file to Float32Array
   */
  private async decodeAudio(arrayBuffer: ArrayBuffer): Promise<Float32Array> {
    // Try to use Web Audio API for decoding
    if (typeof AudioContext !== 'undefined') {
      const audioContext = new AudioContext({ sampleRate: 16000 });
      try {
        const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);
        const channelData = audioBuffer.getChannelData(0);

        // Resample if needed
        if (audioBuffer.sampleRate !== 16000) {
          return AudioProcessor.resample(channelData, audioBuffer.sampleRate, 16000);
        }

        return channelData;
      } finally {
        await audioContext.close();
      }
    }

    // Fallback: assume raw PCM
    const int16 = new Int16Array(arrayBuffer);
    return AudioProcessor.int16ToFloat32(int16);
  }

  /**
   * Finish transcription and return result
   */
  private finishTranscription(durationSeconds: number): void {
    if (!this.resolveTranscription) return;

    const processingTimeMs = Date.now() - this.startTime;
    const avgConfidence = this.confidences.length > 0
      ? this.confidences.reduce((a, b) => a + b, 0) / this.confidences.length
      : 0;

    const result: TranscriptionResult = {
      text: this.transcripts.join(' '),
      confidence: avgConfidence,
      language: this.sttConfig.language,
      durationSeconds,
      processingTimeMs,
      words: this.words.length > 0 ? this.words : undefined,
    };

    // Group by speaker if diarization was enabled
    if (this.sttConfig.diarize && this.words.length > 0) {
      const speakerMap = new Map<number, Array<{ start: number; end: number; text: string }>>();

      let currentSpeaker: number | undefined;
      let currentSegment: { start: number; end: number; words: string[] } | null = null;

      for (const word of this.words) {
        if (word.speakerId !== currentSpeaker) {
          // Save previous segment
          if (currentSegment && currentSpeaker !== undefined) {
            const segments = speakerMap.get(currentSpeaker) ?? [];
            segments.push({
              start: currentSegment.start,
              end: currentSegment.end,
              text: currentSegment.words.join(' '),
            });
            speakerMap.set(currentSpeaker, segments);
          }

          // Start new segment
          currentSpeaker = word.speakerId;
          currentSegment = {
            start: word.start,
            end: word.end,
            words: [word.word],
          };
        } else if (currentSegment) {
          currentSegment.end = word.end;
          currentSegment.words.push(word.word);
        }
      }

      // Save last segment
      if (currentSegment && currentSpeaker !== undefined) {
        const segments = speakerMap.get(currentSpeaker) ?? [];
        segments.push({
          start: currentSegment.start,
          end: currentSegment.end,
          text: currentSegment.words.join(' '),
        });
        speakerMap.set(currentSpeaker, segments);
      }

      result.speakers = Array.from(speakerMap.entries()).map(([speakerId, segments]) => ({
        speakerId,
        segments,
      }));
    }

    this.isTranscribing = false;
    this.resolveTranscription(result);
    this.resolveTranscription = null;
    this.rejectTranscription = null;
  }

  /**
   * Reset internal state
   */
  private resetState(): void {
    this.transcripts = [];
    this.words = [];
    this.confidences = [];
    this.bytesProcessed = 0;
    this.totalBytes = 0;
    this.interimText = '';
  }

  /**
   * Transcribe from async audio generator
   */
  async transcribeStream(
    audioStream: AsyncIterable<ArrayBuffer | Int16Array>,
    options?: {
      onProgress?: (progress: TranscriptionProgress) => void;
      onTranscript?: (transcript: TranscriptEvent) => void;
    }
  ): Promise<TranscriptionResult> {
    if (this.isTranscribing) {
      throw new Error('Transcription already in progress');
    }

    this.resetState();
    this.isTranscribing = true;
    this.startTime = Date.now();

    if (!this.isConnected()) {
      await this.connect();
    }

    // Setup transcript callback
    let unsubscribe: (() => void) | null = null;
    if (options?.onTranscript) {
      unsubscribe = this.on('transcript', options.onTranscript);
    }

    try {
      let totalSamples = 0;

      for await (const chunk of audioStream) {
        const buffer = chunk instanceof Int16Array ? chunk.buffer as ArrayBuffer : chunk;
        this.session.sendAudio(buffer);

        const samples = chunk instanceof Int16Array ? chunk.length : chunk.byteLength / 2;
        totalSamples += samples;
        this.bytesProcessed += chunk instanceof Int16Array ? chunk.byteLength : chunk.byteLength;

        if (options?.onProgress) {
          options.onProgress({
            bytesProcessed: this.bytesProcessed,
            interimText: this.interimText,
          });
        }
      }

      // Flush and wait for final results
      this.session.flush();
      await new Promise((r) => setTimeout(r, 1000));

      const durationSeconds = totalSamples / 16000;
      const processingTimeMs = Date.now() - this.startTime;
      const avgConfidence = this.confidences.length > 0
        ? this.confidences.reduce((a, b) => a + b, 0) / this.confidences.length
        : 0;

      this.isTranscribing = false;

      return {
        text: this.transcripts.join(' '),
        confidence: avgConfidence,
        language: this.sttConfig.language,
        durationSeconds,
        processingTimeMs,
        words: this.words.length > 0 ? this.words : undefined,
      };
    } finally {
      if (unsubscribe) {
        unsubscribe();
      }
    }
  }

  /**
   * Check if transcription is in progress
   */
  isTranscriptionInProgress(): boolean {
    return this.isTranscribing;
  }

  /**
   * Cancel current transcription
   */
  cancelTranscription(): void {
    if (!this.isTranscribing) return;

    this.isTranscribing = false;
    this.session.stop();

    if (this.rejectTranscription) {
      this.rejectTranscription(new Error('Transcription cancelled'));
      this.rejectTranscription = null;
      this.resolveTranscription = null;
    }
  }

  /**
   * Update STT config
   */
  updateConfig(config: Partial<STTConfig>): void {
    this.sttConfig = { ...this.sttConfig, ...config };
    this.session.updateSTTConfig(config);
  }
}

/**
 * Create a BudTranscribe instance
 */
export function createBudTranscribe(config: BudTranscribeConfig): BudTranscribe {
  return new BudTranscribe(config);
}
