/**
 * WebSocket Session
 * High-level WebSocket session with automatic reconnection, message handling, and metrics
 */

import { ConnectionError } from '../errors/index.js';
import type { STTConfig, TTSConfig, LiveKitConfig } from '../types/config.js';
import type { FeatureFlags } from '../types/features.js';
import type { IncomingMessage, OutgoingMessage, STTResultMessage, TTSAudioMessage, ReadyMessage, ErrorMessage } from '../types/messages.js';
import type { MetricsSummary } from '../types/metrics.js';
import { getMetricsCollector, MetricsCollector } from '../metrics/collector.js';
import { WebSocketConnection, type ConnectionState } from './connection.js';
import { ReconnectStrategy, type ReconnectConfig } from './reconnect.js';
import { createConfigMessage, createSpeakMessage, createPingMessage, createStopMessage, createFlushMessage, createInterruptMessage } from './messages.js';
import { MessageQueue, type MessageQueueConfig } from './queue.js';
import { SessionEventEmitter, type SessionEventMap, type SessionEventHandler, type TranscriptEvent, type AudioEvent, type ReadyEvent, type SessionErrorEvent } from './events.js';

/**
 * Session configuration
 */
export interface SessionConfig {
  /** WebSocket URL */
  url: string;
  /** API key for authentication */
  apiKey?: string;
  /** Connection timeout in milliseconds */
  connectionTimeout?: number;
  /** Reconnection configuration */
  reconnect?: ReconnectConfig | false;
  /** Message queue configuration */
  queue?: MessageQueueConfig;
  /** Custom WebSocket implementation */
  WebSocket?: typeof WebSocket;
  /** STT configuration */
  stt?: STTConfig;
  /** TTS configuration */
  tts?: TTSConfig;
  /** LiveKit configuration */
  livekit?: LiveKitConfig;
  /** Feature flags */
  features?: FeatureFlags;
  /** Ping interval in milliseconds (default: 30000, 0 to disable) */
  pingInterval?: number;
  /** Whether to auto-send config on connect (default: true) */
  autoConfig?: boolean;
}

/**
 * Session state
 */
export type SessionState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting';

/**
 * WebSocket session with full functionality
 */
export class WebSocketSession {
  private config: SessionConfig;
  private connection: WebSocketConnection;
  private reconnectStrategy: ReconnectStrategy | null;
  private queue: MessageQueue;
  private emitter: SessionEventEmitter;
  private metrics: MetricsCollector;
  private state: SessionState = 'disconnected';
  private pingIntervalId: ReturnType<typeof setInterval> | null = null;
  private lastPingTime: number | null = null;
  private sessionId: string | null = null;
  private readyReceived = false;
  private lastReadyEvent: ReadyEvent | null = null;
  private sttConfig?: STTConfig;
  private ttsConfig?: TTSConfig;
  private livekitConfig?: LiveKitConfig;
  private featuresConfig?: FeatureFlags;

  constructor(config: SessionConfig) {
    this.config = config;
    this.sttConfig = config.stt;
    this.ttsConfig = config.tts;
    this.livekitConfig = config.livekit;
    this.featuresConfig = config.features;

    // Build WebSocket URL with auth
    let wsUrl = config.url;
    if (config.apiKey) {
      const urlObj = new URL(config.url);
      urlObj.searchParams.set('token', config.apiKey);
      wsUrl = urlObj.toString();
    }

    this.connection = new WebSocketConnection({
      url: wsUrl,
      timeout: config.connectionTimeout ?? 10000,
      WebSocket: config.WebSocket,
    });

    this.reconnectStrategy = config.reconnect !== false
      ? new ReconnectStrategy(typeof config.reconnect === 'object' ? config.reconnect : undefined)
      : null;

    this.queue = new MessageQueue(config.queue);
    this.emitter = new SessionEventEmitter();
    this.metrics = getMetricsCollector();

    this.setupConnectionHandlers();
  }

  /**
   * Setup connection event handlers
   */
  private setupConnectionHandlers(): void {
    this.connection.setHandlers({
      onOpen: () => this.handleOpen(),
      onClose: (code, reason) => this.handleClose(code, reason),
      onError: (error) => this.handleError(error),
      onMessage: (message) => this.handleMessage(message),
      onBinaryMessage: (data) => this.handleBinaryMessage(data),
    });

    if (this.reconnectStrategy) {
      this.reconnectStrategy.setHandlers({
        onReconnecting: (state) => {
          this.state = 'reconnecting';
          this.metrics.setWSState('reconnecting');
          this.emitter.emit('reconnect', { state, event: 'reconnecting' });
          this.emitter.emit('connectionState', {
            previousState: 'disconnected',
            currentState: 'reconnecting',
            timestamp: Date.now(),
          });
        },
        onReconnected: (state) => {
          this.metrics.increment('ws.reconnects');
          this.emitter.emit('reconnect', { state, event: 'reconnected' });
        },
        onReconnectFailed: (error, state) => {
          this.emitter.emit('reconnect', { state, event: 'failed', error });
        },
        onReconnectExhausted: (state) => {
          this.emitter.emit('reconnect', { state, event: 'exhausted' });
        },
      });
    }
  }

  /**
   * Handle connection open
   */
  private handleOpen(): void {
    const previousState = this.state;
    this.state = 'connected';
    this.metrics.setWSState('connected');
    this.reconnectStrategy?.markConnected();

    this.emitter.emit('connectionState', {
      previousState: previousState as 'disconnected' | 'connecting' | 'connected' | 'reconnecting',
      currentState: 'connected',
      timestamp: Date.now(),
    });

    // Start ping interval
    if (this.config.pingInterval !== 0) {
      this.startPingInterval();
    }

    // Send queued messages
    this.flushQueue();

    // Send config if auto-config enabled
    if (this.config.autoConfig !== false) {
      this.sendConfig();
    }
  }

  /**
   * Handle connection close
   */
  private handleClose(code: number, reason: string): void {
    const previousState = this.state;
    this.state = 'disconnected';
    this.metrics.setWSState('disconnected');
    this.readyReceived = false;
    this.lastReadyEvent = null;
    this.stopPingInterval();

    this.emitter.emit('close', { code, reason });
    this.emitter.emit('connectionState', {
      previousState: previousState as 'disconnected' | 'connecting' | 'connected' | 'reconnecting',
      currentState: 'disconnected',
      timestamp: Date.now(),
    });

    // Attempt reconnection if enabled and not a clean close
    if (this.reconnectStrategy?.shouldReconnect() && code !== 1000) {
      this.reconnectStrategy.scheduleReconnect(() => this.connection.connect()).catch((err) => {
        this.emitter.emit('error', {
          code: 'RECONNECT_FAILED',
          message: err instanceof Error ? err.message : 'Reconnection failed',
          recoverable: false,
          raw: { type: 'error', code: 'RECONNECT_FAILED', message: String(err) },
        });
      });
    }
  }

  /**
   * Handle connection error
   */
  private handleError(error: Error): void {
    this.emitter.emit('error', {
      code: 'CONNECTION_ERROR',
      message: error.message,
      recoverable: this.reconnectStrategy?.shouldReconnect() ?? false,
      raw: { type: 'error', code: 'CONNECTION_ERROR', message: error.message },
    });
  }

  /**
   * Handle incoming message
   */
  private handleMessage(message: IncomingMessage): void {
    this.metrics.increment('ws.received');

    switch (message.type) {
      case 'ready':
        this.handleReady(message as ReadyMessage);
        break;
      case 'stt_result':
        this.handleSTTResult(message as STTResultMessage);
        break;
      case 'tts_audio':
        this.handleTTSAudio(message as TTSAudioMessage);
        break;
      case 'error':
        this.handleErrorMessage(message as ErrorMessage);
        break;
      case 'pong':
        this.handlePong(message as { type: 'pong'; timestamp: number; serverTime?: number });
        break;
      case 'speaking_started':
        this.emitter.emit('speaking', { speaking: true, timestamp: Date.now() });
        break;
      case 'speaking_finished':
        this.emitter.emit('speaking', { speaking: false, timestamp: Date.now() });
        break;
      case 'listening_started':
        this.emitter.emit('listening', { listening: true, timestamp: Date.now() });
        break;
      case 'listening_stopped':
        this.emitter.emit('listening', { listening: false, timestamp: Date.now() });
        break;
    }
  }

  /**
   * Handle ready message
   */
  private handleReady(message: ReadyMessage): void {
    this.readyReceived = true;
    this.sessionId = message.sessionId ?? null;

    const event: ReadyEvent = {
      sessionId: message.sessionId,
      sttReady: message.sttReady ?? false,
      ttsReady: message.ttsReady ?? false,
      livekitConnected: message.livekitConnected ?? false,
      serverVersion: message.serverVersion,
      capabilities: message.capabilities ?? [],
      raw: message,
    };

    // Store the event for later retrieval by waitForReady()
    this.lastReadyEvent = event;

    this.emitter.emit('ready', event);
  }

  /**
   * Handle STT result message
   */
  private handleSTTResult(message: STTResultMessage): void {
    if (message.isFinal) {
      this.metrics.increment('stt.transcriptions');
      this.metrics.increment('stt.characters', message.text.length);
    }

    const event: TranscriptEvent = {
      text: message.text,
      isFinal: message.isFinal,
      confidence: message.confidence,
      speakerId: message.speakerId,
      language: message.language,
      startTime: message.startTime,
      endTime: message.endTime,
      words: message.words,
      raw: message,
    };

    this.emitter.emit('transcript', event);
  }

  /**
   * Handle TTS audio message
   */
  private handleTTSAudio(message: TTSAudioMessage): void {
    // Decode base64 audio
    const binaryString = atob(message.audio);
    const bytes = new Uint8Array(binaryString.length);
    for (let i = 0; i < binaryString.length; i++) {
      bytes[i] = binaryString.charCodeAt(i);
    }

    const event: AudioEvent = {
      audio: bytes.buffer,
      format: message.format ?? 'linear16',
      sampleRate: message.sampleRate ?? 24000,
      duration: message.duration,
      isFinal: message.isFinal ?? false,
      sequence: message.sequence,
      raw: message,
    };

    this.emitter.emit('audio', event);
  }

  /**
   * Handle error message
   */
  private handleErrorMessage(message: ErrorMessage): void {
    const event: SessionErrorEvent = {
      code: message.code,
      message: message.message,
      details: message.details,
      recoverable: message.recoverable ?? false,
      raw: message,
    };

    this.emitter.emit('error', event);
  }

  /**
   * Handle pong message
   */
  private handlePong(message: { type: 'pong'; timestamp: number; serverTime?: number }): void {
    if (this.lastPingTime) {
      const latency = Date.now() - this.lastPingTime;
      this.emitter.emit('pong', { latency, serverTime: message.serverTime });
    }
  }

  /**
   * Handle binary message (audio frames)
   */
  private handleBinaryMessage(data: ArrayBuffer): void {
    this.metrics.increment('ws.bytesReceived', data.byteLength);

    // Binary data is typically raw audio - emit as audio event
    const event: AudioEvent = {
      audio: data,
      format: 'linear16',
      sampleRate: 24000,
      isFinal: false,
      raw: {
        type: 'tts_audio',
        audio: '', // Not base64 for binary
      },
    };

    this.emitter.emit('audio', event);
  }

  /**
   * Start ping interval
   */
  private startPingInterval(): void {
    const interval = this.config.pingInterval ?? 30000;
    if (interval <= 0) return;

    this.pingIntervalId = setInterval(() => {
      if (this.connection.isConnected()) {
        this.lastPingTime = Date.now();
        this.connection.send(createPingMessage());
      }
    }, interval);
  }

  /**
   * Stop ping interval
   */
  private stopPingInterval(): void {
    if (this.pingIntervalId) {
      clearInterval(this.pingIntervalId);
      this.pingIntervalId = null;
    }
  }

  /**
   * Flush queued messages
   */
  private flushQueue(): void {
    const messages = this.queue.drain();
    for (const { message, binaryData } of messages) {
      if (binaryData) {
        this.connection.sendBinary(binaryData);
      } else {
        this.connection.send(message);
      }
    }
  }

  /**
   * Send configuration message
   */
  private sendConfig(): void {
    if (this.sttConfig || this.ttsConfig || this.livekitConfig || this.featuresConfig) {
      const configMessage = createConfigMessage(
        this.sttConfig,
        this.ttsConfig,
        this.livekitConfig,
        this.featuresConfig
      );
      this.send(configMessage);
    }
  }

  // Public API

  /**
   * Connect to server
   */
  async connect(): Promise<void> {
    if (this.state === 'connected') {
      return;
    }

    this.state = 'connecting';
    this.metrics.setWSState('connecting');

    const startTime = Date.now();

    this.emitter.emit('connectionState', {
      previousState: 'disconnected',
      currentState: 'connecting',
      timestamp: Date.now(),
    });

    await this.connection.connect();

    const duration = Date.now() - startTime;
    this.metrics.record('ws.connect', duration);
  }

  /**
   * Disconnect from server
   */
  async disconnect(): Promise<void> {
    this.reconnectStrategy?.abort();
    this.stopPingInterval();
    await this.connection.close();
  }

  /**
   * Check if connected
   */
  isConnected(): boolean {
    return this.connection.isConnected();
  }

  /**
   * Check if ready (config acknowledged)
   */
  isReady(): boolean {
    return this.readyReceived;
  }

  /**
   * Get current session state
   */
  getState(): SessionState {
    return this.state;
  }

  /**
   * Get session ID
   */
  getSessionId(): string | null {
    return this.sessionId;
  }

  /**
   * Send a message
   */
  send(message: OutgoingMessage): void {
    this.metrics.increment('ws.sent');

    if (this.connection.isConnected()) {
      this.connection.send(message);
    } else {
      this.queue.enqueue(message);
    }
  }

  /**
   * Send audio data
   */
  sendAudio(data: ArrayBuffer | Uint8Array): void {
    // Extract the correct ArrayBuffer slice from Uint8Array views
    // If Uint8Array is a view into a larger buffer, data.buffer returns the full buffer
    // which would corrupt the data. We need to extract just the portion we're using.
    let buffer: ArrayBuffer;
    if (data instanceof Uint8Array) {
      if (data.byteOffset === 0 && data.byteLength === data.buffer.byteLength) {
        // Uint8Array uses the whole buffer, safe to use directly
        buffer = data.buffer;
      } else {
        // Uint8Array is a view into a larger buffer, need to copy the slice
        buffer = data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
      }
    } else {
      buffer = data;
    }

    this.metrics.increment('ws.bytesSent', buffer.byteLength);

    if (this.connection.isConnected()) {
      this.connection.sendBinary(buffer);
    } else {
      this.queue.enqueue({ type: 'audio' }, buffer);
    }
  }

  /**
   * Speak text
   */
  speak(text: string, options?: {
    voice?: string;
    voiceId?: string;
    provider?: string;
    model?: string;
    speed?: number;
    pitch?: number;
    flush?: boolean;
  }): void {
    this.send(createSpeakMessage(text, options));
    this.metrics.increment('tts.speaks');
    this.metrics.increment('tts.characters', text.length);
  }

  /**
   * Stop current operation
   */
  stop(): void {
    this.send(createStopMessage());
  }

  /**
   * Flush pending audio
   */
  flush(): void {
    this.send(createFlushMessage());
  }

  /**
   * Interrupt current operation
   */
  interrupt(): void {
    this.send(createInterruptMessage());
  }

  /**
   * Update STT configuration
   */
  updateSTTConfig(config: Partial<STTConfig>): void {
    this.sttConfig = { ...this.sttConfig, ...config } as STTConfig;
    this.send(createConfigMessage(this.sttConfig));
  }

  /**
   * Update TTS configuration
   */
  updateTTSConfig(config: Partial<TTSConfig>): void {
    this.ttsConfig = { ...this.ttsConfig, ...config } as TTSConfig;
    this.send(createConfigMessage(undefined, this.ttsConfig));
  }

  /**
   * Update feature flags
   */
  updateFeatures(features: Partial<FeatureFlags>): void {
    this.featuresConfig = { ...this.featuresConfig, ...features } as FeatureFlags;
    this.send(createConfigMessage(undefined, undefined, undefined, this.featuresConfig));
  }

  /**
   * Add event listener
   */
  on<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): () => void {
    return this.emitter.on(event, handler);
  }

  /**
   * Add one-time event listener
   */
  once<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): () => void {
    return this.emitter.once(event, handler);
  }

  /**
   * Remove event listener
   */
  off<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): void {
    this.emitter.off(event, handler);
  }

  /**
   * Get current metrics
   */
  getMetrics(): MetricsSummary {
    return this.metrics.getMetrics();
  }

  /**
   * Get queue statistics
   */
  getQueueStats(): { size: number; maxSize: number; droppedCount: number; oldestAge: number | null } {
    return this.queue.getStats();
  }

  /**
   * Wait for ready state
   */
  waitForReady(timeout = 10000): Promise<ReadyEvent> {
    // Return stored event if already received (with actual values, not hardcoded)
    if (this.readyReceived && this.lastReadyEvent) {
      return Promise.resolve(this.lastReadyEvent);
    }

    return new Promise((resolve, reject) => {
      // Define handler before setTimeout to avoid temporal dead zone issues
      // and ensure proper cleanup on timeout
      const handler = (event: ReadyEvent) => {
        clearTimeout(timeoutId);
        resolve(event);
      };

      const timeoutId = setTimeout(() => {
        this.off('ready', handler);
        reject(new ConnectionError('Timeout waiting for ready state', { timeout }));
      }, timeout);

      this.once('ready', handler);
    });
  }
}
