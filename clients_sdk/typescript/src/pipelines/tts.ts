/**
 * BudTTS Pipeline
 * Text-to-Speech pipeline for real-time synthesis
 */

import type { TTSConfig } from '../types/config.js';
import type { FeatureFlags } from '../types/features.js';
import type { AudioEvent } from '../ws/events.js';
import { BasePipeline, type BasePipelineConfig } from './base.js';
import { PCMPlayer, type PlayerConfig } from '../audio/player.js';
import { RestClient } from '../rest/client.js';

/**
 * BudTTS configuration
 */
export interface BudTTSConfig extends BasePipelineConfig {
  /** TTS provider (e.g., 'elevenlabs', 'deepgram', 'azure') */
  provider?: string;
  /** Voice name or identifier */
  voice?: string;
  /** Voice ID (provider-specific) */
  voiceId?: string;
  /** Model to use (e.g., 'eleven_turbo_v2') */
  model?: string;
  /** Output sample rate (default: 24000) */
  sampleRate?: number;
  /** Output audio format (default: 'linear16') */
  audioFormat?: string;
  /** Speech rate multiplier (default: 1.0) */
  speed?: number;
  /** Pitch adjustment (default: 1.0) */
  pitch?: number;
  /** Volume adjustment (default: 1.0) */
  volume?: number;
  /** Voice stability (ElevenLabs specific, 0-1) */
  stability?: number;
  /** Voice similarity boost (ElevenLabs specific, 0-1) */
  similarityBoost?: number;
  /** Voice style (ElevenLabs specific, 0-1) */
  style?: number;
  /** Use speaker boost (ElevenLabs specific) */
  useSpeakerBoost?: boolean;
  /** Whether to auto-play received audio (default: true) */
  autoPlay?: boolean;
  /** Player configuration */
  playerConfig?: PlayerConfig;
  /** Base URL for REST API (used for one-shot synthesis) */
  restBaseUrl?: string;
}

/**
 * BudTTS - Text-to-Speech Pipeline
 */
export class BudTTS extends BasePipeline {
  private ttsConfig: TTSConfig;
  private player: PCMPlayer | null = null;
  private autoPlay: boolean;
  private restClient: RestClient | null = null;
  private isSpeaking = false;
  private speakQueue: string[] = [];
  private isProcessingQueue = false;

  constructor(config: BudTTSConfig) {
    const ttsConfig: TTSConfig = {
      provider: config.provider ?? 'deepgram',
      voice: config.voice,
      voiceId: config.voiceId,
      model: config.model,
      sampleRate: config.sampleRate ?? 24000,
      audioFormat: config.audioFormat ?? 'linear16',
      speed: config.speed,
      pitch: config.pitch,
      volume: config.volume,
      stability: config.stability,
      similarityBoost: config.similarityBoost,
      style: config.style,
      useSpeakerBoost: config.useSpeakerBoost,
    };

    super({
      ...config,
      sessionConfig: {
        tts: ttsConfig,
      },
    });

    this.ttsConfig = ttsConfig;
    this.autoPlay = config.autoPlay ?? true;

    // Setup audio player if auto-play enabled
    if (this.autoPlay) {
      this.player = new PCMPlayer({
        sampleRate: ttsConfig.sampleRate,
        ...config.playerConfig,
      });
    }

    // Setup REST client for one-shot synthesis
    if (config.restBaseUrl) {
      this.restClient = new RestClient({
        baseUrl: config.restBaseUrl,
        apiKey: config.apiKey,
      });
    }

    // Forward audio events
    this.session.on('audio', (e) => this.handleAudio(e));
    this.session.on('speaking', (e) => {
      this.isSpeaking = e.speaking;
      this.emitter.emit('speaking', e);
    });
  }

  /**
   * Handle received audio
   */
  private handleAudio(event: AudioEvent): void {
    this.emitter.emit('audio', event);

    if (this.autoPlay && this.player) {
      this.player.addPCM(event.audio);
    }
  }

  /**
   * Initialize the player
   */
  async initializePlayer(): Promise<void> {
    if (this.player) {
      await this.player.initialize();
    }
  }

  /**
   * Speak text
   * @param text Text to synthesize
   * @param options Optional speak options
   */
  async speak(text: string, options?: {
    voice?: string;
    voiceId?: string;
    provider?: string;
    model?: string;
    speed?: number;
    pitch?: number;
    flush?: boolean;
  }): Promise<void> {
    if (!this.isConnected()) {
      await this.connect();
    }

    // Ensure player is initialized
    if (this.autoPlay && this.player) {
      await this.player.initialize();
    }

    this.session.speak(text, options);
  }

  /**
   * Queue text for speaking
   * @param text Text to queue
   */
  queueSpeak(text: string): void {
    this.speakQueue.push(text);
    this.processQueue();
  }

  /**
   * Process speak queue
   */
  private async processQueue(): Promise<void> {
    if (this.isProcessingQueue || this.isSpeaking || this.speakQueue.length === 0) {
      return;
    }

    this.isProcessingQueue = true;

    while (this.speakQueue.length > 0 && !this.isSpeaking) {
      const text = this.speakQueue.shift();
      if (text) {
        await this.speak(text);
        // Wait for speaking to finish
        await this.waitForSpeakingDone();
      }
    }

    this.isProcessingQueue = false;
  }

  /**
   * Wait for current speaking to finish
   */
  private waitForSpeakingDone(): Promise<void> {
    return new Promise((resolve) => {
      if (!this.isSpeaking) {
        resolve();
        return;
      }

      const handler = (e: { speaking: boolean }) => {
        if (!e.speaking) {
          this.off('speaking', handler);
          resolve();
        }
      };

      this.on('speaking', handler);
    });
  }

  /**
   * One-shot synthesis using REST API
   * @param text Text to synthesize
   * @param options Optional options
   * @returns Audio buffer
   */
  async synthesize(text: string, options?: {
    provider?: string;
    voice?: string;
    voiceId?: string;
    model?: string;
    sampleRate?: number;
    format?: string;
  }): Promise<ArrayBuffer> {
    if (!this.restClient) {
      throw new Error('REST client not configured. Provide restBaseUrl in config.');
    }

    const result = await this.restClient.speak(text, {
      provider: options?.provider ?? this.ttsConfig.provider,
      voice: options?.voice ?? this.ttsConfig.voice,
      model: options?.model ?? this.ttsConfig.model,
      sampleRate: options?.sampleRate ?? this.ttsConfig.sampleRate,
      format: options?.format ?? this.ttsConfig.audioFormat,
    });

    return result.audio;
  }

  /**
   * Update TTS configuration
   */
  updateConfig(config: Partial<TTSConfig>): void {
    this.ttsConfig = { ...this.ttsConfig, ...config };
    this.session.updateTTSConfig(config);
  }

  /**
   * Stop current speech
   */
  stopSpeaking(): void {
    this.session.interrupt();
    this.player?.stop();
    this.speakQueue = [];
    this.isSpeaking = false;
  }

  /**
   * Pause playback
   */
  pausePlayback(): void {
    this.player?.pause();
  }

  /**
   * Resume playback
   */
  resumePlayback(): void {
    this.player?.play();
  }

  /**
   * Clear playback buffer
   */
  clearBuffer(): void {
    this.player?.clearBuffer();
  }

  /**
   * Set playback volume
   * @param gainDb Volume in decibels
   */
  setVolume(gainDb: number): void {
    this.player?.setGain(gainDb);
  }

  /**
   * Check if currently speaking
   */
  getIsSpeaking(): boolean {
    return this.isSpeaking;
  }

  /**
   * Get current queue length
   */
  getQueueLength(): number {
    return this.speakQueue.length;
  }

  /**
   * Dispose resources
   */
  async dispose(): Promise<void> {
    this.stopSpeaking();
    if (this.player) {
      await this.player.dispose();
      this.player = null;
    }
    await this.disconnect();
  }
}

/**
 * Create a BudTTS instance
 */
export function createBudTTS(config: BudTTSConfig): BudTTS {
  return new BudTTS(config);
}
