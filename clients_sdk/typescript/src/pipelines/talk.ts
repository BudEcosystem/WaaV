/**
 * BudTalk Pipeline
 * Bidirectional voice pipeline combining STT + TTS
 */

import type { STTConfig, TTSConfig, LiveKitConfig } from '../types/config.js';
import type { FeatureFlags } from '../types/features.js';
import type { MetricsSummary } from '../types/metrics.js';
import type { TranscriptEvent, AudioEvent } from '../ws/events.js';
import { BasePipeline, type BasePipelineConfig } from './base.js';
import { AudioRecorder, type RecorderConfig } from '../audio/recorder.js';
import { PCMPlayer, type PlayerConfig } from '../audio/player.js';
import { VAD, type VADConfig } from '../audio/vad.js';
import { getMetricsCollector } from '../metrics/collector.js';

/**
 * BudTalk configuration
 */
export interface BudTalkConfig extends BasePipelineConfig {
  /** STT configuration */
  stt?: Partial<STTConfig>;
  /** TTS configuration */
  tts?: Partial<TTSConfig>;
  /** LiveKit configuration for room-based communication */
  livekit?: LiveKitConfig;
  /** VAD configuration */
  vadConfig?: VADConfig;
  /** Recorder configuration */
  recorderConfig?: RecorderConfig;
  /** Player configuration */
  playerConfig?: PlayerConfig;
  /** Whether to auto-play received audio (default: true) */
  autoPlay?: boolean;
  /** Whether to auto-record from microphone (default: false) */
  autoRecord?: boolean;
  /** Interrupt TTS when user starts speaking (default: true) */
  bargeIn?: boolean;
}

/**
 * Conversation turn
 */
export interface ConversationTurn {
  /** Who spoke: 'user' or 'assistant' */
  role: 'user' | 'assistant';
  /** The text content */
  text: string;
  /** Timestamp */
  timestamp: number;
  /** Whether it's complete */
  isFinal: boolean;
  /** Audio duration if available */
  audioDurationMs?: number;
}

/**
 * BudTalk - Bidirectional Voice Pipeline
 */
export class BudTalk extends BasePipeline {
  private sttConfig: STTConfig;
  private ttsConfig: TTSConfig;
  private livekitConfig?: LiveKitConfig;
  private recorder: AudioRecorder | null = null;
  private player: PCMPlayer | null = null;
  private vad: VAD | null = null;
  private autoPlay: boolean;
  private autoRecord: boolean;
  private bargeIn: boolean;
  private isListening = false;
  private isSpeaking = false;
  private conversationHistory: ConversationTurn[] = [];
  private currentUserUtterance = '';
  private e2eLatencyStart: number | null = null;

  constructor(config: BudTalkConfig) {
    const sttConfig: STTConfig = {
      provider: config.stt?.provider ?? 'deepgram',
      language: config.stt?.language ?? 'en-US',
      model: config.stt?.model,
      sampleRate: config.stt?.sampleRate ?? 16000,
      encoding: config.stt?.encoding ?? 'linear16',
      channels: config.stt?.channels ?? 1,
      interimResults: config.stt?.interimResults ?? true,
      punctuate: config.stt?.punctuate ?? true,
      profanityFilter: config.stt?.profanityFilter ?? false,
      smartFormat: config.stt?.smartFormat ?? true,
      diarize: config.stt?.diarize ?? false,
    };

    const ttsConfig: TTSConfig = {
      provider: config.tts?.provider ?? 'deepgram',
      voice: config.tts?.voice,
      voiceId: config.tts?.voiceId,
      model: config.tts?.model,
      sampleRate: config.tts?.sampleRate ?? 24000,
      audioFormat: config.tts?.audioFormat ?? 'linear16',
      speed: config.tts?.speed,
      pitch: config.tts?.pitch,
      volume: config.tts?.volume,
    };

    super({
      ...config,
      sessionConfig: {
        stt: sttConfig,
        tts: ttsConfig,
        livekit: config.livekit,
      },
    });

    this.sttConfig = sttConfig;
    this.ttsConfig = ttsConfig;
    this.livekitConfig = config.livekit;
    this.autoPlay = config.autoPlay ?? true;
    this.autoRecord = config.autoRecord ?? false;
    this.bargeIn = config.bargeIn ?? true;

    // Setup VAD
    if (config.features?.vad !== false) {
      this.vad = new VAD({
        sampleRate: sttConfig.sampleRate,
        ...config.vadConfig,
      });

      this.vad.setHandlers({
        onSpeechStart: (buffer) => this.handleSpeechStart(buffer),
        onSpeechEnd: (duration) => this.handleSpeechEnd(duration),
      });
    }

    // Setup player
    if (this.autoPlay) {
      this.player = new PCMPlayer({
        sampleRate: ttsConfig.sampleRate,
        ...config.playerConfig,
      });
    }

    this.setupEventHandlers();
  }

  /**
   * Setup internal event handlers
   */
  private setupEventHandlers(): void {
    // Handle transcripts
    this.session.on('transcript', (e) => {
      this.handleTranscript(e);
    });

    // Handle audio
    this.session.on('audio', (e) => {
      this.handleAudio(e);
    });

    // Handle speaking state
    this.session.on('speaking', (e) => {
      this.isSpeaking = e.speaking;

      if (e.speaking && this.e2eLatencyStart) {
        // Record E2E latency (from user speech end to bot speech start)
        const latency = Date.now() - this.e2eLatencyStart;
        getMetricsCollector().record('e2e.latency', latency);
        this.e2eLatencyStart = null;
      }

      this.emitter.emit('speaking', e);
    });

    // Handle listening state
    this.session.on('listening', (e) => {
      this.emitter.emit('listening', e);
    });
  }

  /**
   * Handle VAD speech start
   */
  private handleSpeechStart(buffer: Float32Array): void {
    this.isListening = true;
    this.emitter.emit('listening', { listening: true, timestamp: Date.now() });

    // Barge-in: interrupt current TTS
    if (this.bargeIn && this.isSpeaking) {
      this.interrupt();
    }

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
    this.isListening = false;
    this.emitter.emit('listening', { listening: false, timestamp: Date.now() });

    // Mark E2E latency start
    this.e2eLatencyStart = Date.now();

    // Flush to finalize transcription
    this.session.flush();
  }

  /**
   * Handle transcript events
   */
  private handleTranscript(event: TranscriptEvent): void {
    if (event.isFinal) {
      // Add to conversation history
      this.conversationHistory.push({
        role: 'user',
        text: event.text,
        timestamp: Date.now(),
        isFinal: true,
      });
      this.currentUserUtterance = '';
    } else {
      this.currentUserUtterance = event.text;
    }

    this.emitter.emit('transcript', event);
  }

  /**
   * Handle audio events
   */
  private handleAudio(event: AudioEvent): void {
    this.emitter.emit('audio', event);

    if (this.autoPlay && this.player) {
      this.player.addPCM(event.audio);
    }
  }

  /**
   * Connect and optionally start recording
   */
  async connect(): Promise<void> {
    await super.connect();

    // Initialize player
    if (this.autoPlay && this.player) {
      await this.player.initialize();
    }

    // Auto-record if enabled
    if (this.autoRecord) {
      await this.startListening();
    }
  }

  /**
   * Send audio data
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

    // Process through VAD if enabled
    if (this.vad) {
      const int16 = new Int16Array(buffer);
      const float32 = new Float32Array(int16.length);
      for (let i = 0; i < int16.length; i++) {
        float32[i] = int16[i]! / 32768;
      }

      const events = this.vad.process(float32);

      for (const event of events) {
        if (event.state === 'speech') {
          this.session.sendAudio(buffer);
          break;
        }
      }
    } else {
      this.session.sendAudio(buffer);
    }
  }

  /**
   * Speak text
   */
  async speak(text: string, options?: {
    voice?: string;
    voiceId?: string;
    provider?: string;
    model?: string;
    speed?: number;
    pitch?: number;
  }): Promise<void> {
    // Add to conversation history
    this.conversationHistory.push({
      role: 'assistant',
      text,
      timestamp: Date.now(),
      isFinal: true,
    });

    this.session.speak(text, options);
  }

  /**
   * Start listening from microphone
   */
  async startListening(recorderConfig?: RecorderConfig): Promise<void> {
    if (this.isListening && this.recorder) return;

    if (!this.isConnected()) {
      await this.connect();
    }

    this.recorder = new AudioRecorder({
      sampleRate: this.sttConfig.sampleRate,
      ...recorderConfig,
    });

    this.recorder.setHandlers({
      onData: (data) => this.sendAudio(data),
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
   * Stop listening
   */
  stopListening(): void {
    if (!this.recorder) return;

    this.recorder.stop();
    this.isListening = false;
    this.session.flush();
  }

  /**
   * Interrupt current TTS
   */
  interrupt(): void {
    super.interrupt();
    this.player?.stop();
  }

  /**
   * Get conversation history
   */
  getConversationHistory(): ConversationTurn[] {
    return [...this.conversationHistory];
  }

  /**
   * Clear conversation history
   */
  clearConversationHistory(): void {
    this.conversationHistory = [];
  }

  /**
   * Check if listening
   */
  getIsListening(): boolean {
    return this.isListening;
  }

  /**
   * Check if speaking
   */
  getIsSpeaking(): boolean {
    return this.isSpeaking;
  }

  /**
   * Update STT config
   */
  updateSTTConfig(config: Partial<STTConfig>): void {
    this.sttConfig = { ...this.sttConfig, ...config };
    this.session.updateSTTConfig(config);
  }

  /**
   * Update TTS config
   */
  updateTTSConfig(config: Partial<TTSConfig>): void {
    this.ttsConfig = { ...this.ttsConfig, ...config };
    this.session.updateTTSConfig(config);
  }

  /**
   * Set barge-in enabled
   */
  setBargeIn(enabled: boolean): void {
    this.bargeIn = enabled;
  }

  /**
   * Set playback volume
   */
  setVolume(gainDb: number): void {
    this.player?.setGain(gainDb);
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

    if (this.player) {
      await this.player.dispose();
      this.player = null;
    }

    await this.disconnect();
  }
}

/**
 * Create a BudTalk instance
 */
export function createBudTalk(config: BudTalkConfig): BudTalk {
  return new BudTalk(config);
}
