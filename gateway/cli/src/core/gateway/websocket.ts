/**
 * WaaV CLI Gateway WebSocket Client
 *
 * WebSocket client for real-time voice streaming with the WaaV Gateway
 */

import WebSocket from 'ws';
import { EventEmitter } from 'events';
import { nanoid } from 'nanoid';
import type {
  GatewayMessage,
  ConfigMessage,
  ReadyMessage,
  STTResultMessage,
  TTSAudioMessage,
  ErrorMessage,
  SpeakMessage,
  AudioMessage,
  InterruptMessage,
  FlushMessage,
  STTConfigMessage,
  TTSConfigMessage,
  FeatureFlags,
} from '../../types/index.js';
import { WebSocketError, ErrorCode } from '../../utils/errors.js';
import { logger } from '../../utils/logger.js';
import { sleep } from '../../utils/helpers.js';

/**
 * WebSocket connection state
 */
export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'error';

/**
 * WebSocket client events
 */
export interface WebSocketClientEvents {
  ready: (message: ReadyMessage) => void;
  transcript: (message: STTResultMessage) => void;
  audio: (message: TTSAudioMessage) => void;
  error: (message: ErrorMessage) => void;
  connectionStateChange: (state: ConnectionState) => void;
  message: (message: GatewayMessage) => void;
  close: (code: number, reason: string) => void;
}

/**
 * WebSocket client configuration
 */
export interface WebSocketClientConfig {
  url: string;
  apiKey?: string;
  autoReconnect?: boolean;
  maxReconnectAttempts?: number;
  reconnectDelay?: number;
  reconnectBackoffMultiplier?: number;
  pingInterval?: number;
  pingTimeout?: number;
}

/**
 * Session configuration
 */
export interface SessionConfig {
  streamId?: string;
  sttConfig?: STTConfigMessage;
  ttsConfig?: TTSConfigMessage;
  features?: FeatureFlags;
}

/**
 * WebSocket Client for Gateway Communication
 */
export class WebSocketClient extends EventEmitter {
  private ws: WebSocket | null = null;
  private url: string;
  private apiKey?: string;
  private state: ConnectionState = 'disconnected';
  private streamId: string | null = null;
  private sessionConfig: SessionConfig | null = null;

  // Reconnection settings
  private autoReconnect: boolean;
  private maxReconnectAttempts: number;
  private reconnectDelay: number;
  private reconnectBackoffMultiplier: number;
  private reconnectAttempts = 0;
  private reconnecting = false;

  // Ping/pong settings
  private pingInterval: number;
  private pingTimeout: number;
  private pingTimer: NodeJS.Timeout | null = null;
  private pongTimer: NodeJS.Timeout | null = null;
  private lastPingTime = 0;
  private latencyMs = 0;

  // Metrics
  private messagesSent = 0;
  private messagesReceived = 0;
  private audioBytesSent = 0;
  private audioBytesReceived = 0;

  constructor(config: WebSocketClientConfig) {
    super();
    this.url = config.url;
    this.apiKey = config.apiKey;
    this.autoReconnect = config.autoReconnect ?? true;
    this.maxReconnectAttempts = config.maxReconnectAttempts ?? 5;
    this.reconnectDelay = config.reconnectDelay ?? 1000;
    this.reconnectBackoffMultiplier = config.reconnectBackoffMultiplier ?? 2;
    this.pingInterval = config.pingInterval ?? 30000;
    this.pingTimeout = config.pingTimeout ?? 10000;
  }

  /**
   * Get current connection state
   */
  getState(): ConnectionState {
    return this.state;
  }

  /**
   * Get stream ID
   */
  getStreamId(): string | null {
    return this.streamId;
  }

  /**
   * Get connection latency
   */
  getLatency(): number {
    return this.latencyMs;
  }

  /**
   * Check if connected
   */
  isConnected(): boolean {
    return this.state === 'connected' && this.ws?.readyState === WebSocket.OPEN;
  }

  /**
   * Connect to the gateway
   */
  async connect(sessionConfig?: SessionConfig): Promise<ReadyMessage> {
    if (this.isConnected()) {
      throw new WebSocketError('Already connected');
    }

    this.sessionConfig = sessionConfig ?? null;
    this.setState('connecting');

    return new Promise((resolve, reject) => {
      try {
        // Build URL with auth
        const url = new URL(this.url);
        if (this.apiKey) {
          url.searchParams.set('token', this.apiKey);
        }

        this.ws = new WebSocket(url.toString());
        this.ws.binaryType = 'arraybuffer';

        // Set up event handlers
        this.ws.onopen = () => {
          logger.debug('WebSocket connected');
          this.setState('connected');
          this.reconnectAttempts = 0;
          this.startPingTimer();

          // Send config message if provided
          if (sessionConfig) {
            this.sendConfig(sessionConfig);
          }
        };

        this.ws.onmessage = (event) => {
          this.handleMessage(event.data, resolve);
        };

        this.ws.onerror = (error) => {
          logger.error('WebSocket error', error);
          this.setState('error');
          reject(new WebSocketError('Connection failed', {
            code: ErrorCode.WEBSOCKET_CONNECTION_FAILED,
          }));
        };

        this.ws.onclose = (event) => {
          this.handleClose(event.code, event.reason);
        };

        // Connection timeout
        const timeout = setTimeout(() => {
          if (this.state === 'connecting') {
            this.ws?.close();
            reject(new WebSocketError('Connection timeout', {
              code: ErrorCode.WEBSOCKET_TIMEOUT,
            }));
          }
        }, 10000);

        // Clear timeout on success
        this.once('ready', () => clearTimeout(timeout));
      } catch (error) {
        this.setState('error');
        reject(new WebSocketError(`Failed to connect: ${error instanceof Error ? error.message : String(error)}`, {
          code: ErrorCode.WEBSOCKET_CONNECTION_FAILED,
        }));
      }
    });
  }

  /**
   * Disconnect from the gateway
   */
  disconnect(): void {
    this.autoReconnect = false;
    this.stopPingTimer();

    if (this.ws) {
      this.ws.close(1000, 'Client disconnect');
      this.ws = null;
    }

    this.streamId = null;
    this.setState('disconnected');
    logger.debug('WebSocket disconnected');
  }

  /**
   * Send configuration message
   */
  sendConfig(config: SessionConfig): void {
    const message: ConfigMessage = {
      type: 'config',
      stream_id: config.streamId ?? nanoid(),
      stt_config: config.sttConfig,
      tts_config: config.ttsConfig,
      features: config.features,
    };

    this.sendMessage(message);
  }

  /**
   * Send audio data
   */
  sendAudio(audio: Uint8Array | Buffer, options?: {
    sampleRate?: number;
    channels?: number;
    encoding?: 'pcm_s16le' | 'pcm_f32le';
  }): void {
    if (!this.isConnected()) {
      throw new WebSocketError('Not connected');
    }

    // For binary audio, we send a JSON header followed by binary data
    const header: AudioMessage = {
      type: 'audio',
      audio: '', // Placeholder - actual audio sent as binary
      sample_rate: options?.sampleRate ?? 16000,
      channels: options?.channels ?? 1,
      encoding: options?.encoding ?? 'pcm_s16le',
    };

    // Send as binary frame
    const headerJson = JSON.stringify(header);
    const headerBytes = Buffer.from(headerJson, 'utf-8');
    const headerLength = Buffer.alloc(4);
    headerLength.writeUInt32LE(headerBytes.length, 0);

    const frame = Buffer.concat([
      headerLength,
      headerBytes,
      Buffer.from(audio),
    ]);

    this.ws?.send(frame);
    this.messagesSent++;
    this.audioBytesSent += audio.length;
  }

  /**
   * Send speak command (TTS)
   */
  sendSpeak(text: string, options?: {
    voiceId?: string;
    speed?: number;
    emotion?: string;
  }): void {
    const message: SpeakMessage = {
      type: 'speak',
      text,
      voice_id: options?.voiceId,
      speed: options?.speed,
      emotion: options?.emotion,
    };

    this.sendMessage(message);
  }

  /**
   * Send interrupt command
   */
  sendInterrupt(): void {
    const message: InterruptMessage = {
      type: 'interrupt',
    };
    this.sendMessage(message);
  }

  /**
   * Send flush command
   */
  sendFlush(): void {
    const message: FlushMessage = {
      type: 'flush',
    };
    this.sendMessage(message);
  }

  /**
   * Send a message to the gateway
   */
  private sendMessage(message: GatewayMessage): void {
    if (!this.isConnected()) {
      throw new WebSocketError('Not connected');
    }

    const json = JSON.stringify(message);
    this.ws?.send(json);
    this.messagesSent++;
    logger.debug(`Sent message: ${message.type}`);
  }

  /**
   * Handle incoming message
   */
  private handleMessage(data: WebSocket.Data, resolveConnect?: (message: ReadyMessage) => void): void {
    this.messagesReceived++;

    try {
      let message: GatewayMessage;

      if (data instanceof ArrayBuffer) {
        // Binary message (TTS audio)
        const buffer = Buffer.from(data);
        const headerLength = buffer.readUInt32LE(0);
        const headerJson = buffer.slice(4, 4 + headerLength).toString('utf-8');
        const header = JSON.parse(headerJson) as TTSAudioMessage;

        // Extract audio data
        const audioData = buffer.slice(4 + headerLength);
        this.audioBytesReceived += audioData.length;

        message = {
          ...header,
          audio: audioData,
        };
      } else {
        // Text message (JSON)
        message = JSON.parse(data.toString()) as GatewayMessage;
      }

      // Emit generic message event
      this.emit('message', message);

      // Handle specific message types
      switch (message.type) {
        case 'ready': {
          const readyMessage = message as ReadyMessage;
          this.streamId = readyMessage.stream_id;
          this.emit('ready', readyMessage);
          resolveConnect?.(readyMessage);
          logger.debug(`Ready: stream_id=${this.streamId}`);
          break;
        }

        case 'stt_result': {
          const sttMessage = message as STTResultMessage;
          this.emit('transcript', sttMessage);
          logger.debug(`Transcript: "${sttMessage.transcript}" (final=${sttMessage.is_final})`);
          break;
        }

        case 'tts_audio': {
          const ttsMessage = message as TTSAudioMessage;
          this.emit('audio', ttsMessage);
          logger.debug(`Audio: ${ttsMessage.is_final ? 'final' : 'chunk'}`);
          break;
        }

        case 'error': {
          const errorMessage = message as ErrorMessage;
          this.emit('error', errorMessage);
          logger.error(`Gateway error: ${errorMessage.message}`);
          break;
        }

        case 'pong': {
          const latency = Date.now() - this.lastPingTime;
          this.latencyMs = latency;
          this.clearPongTimer();
          logger.debug(`Pong received: latency=${latency}ms`);
          break;
        }

        default:
          logger.debug(`Unknown message type: ${message.type}`);
      }
    } catch (error) {
      logger.error('Failed to parse message', error);
    }
  }

  /**
   * Handle connection close
   */
  private handleClose(code: number, reason: string): void {
    logger.debug(`WebSocket closed: code=${code}, reason=${reason}`);
    this.stopPingTimer();
    this.emit('close', code, reason);

    // Attempt reconnection if enabled
    if (this.autoReconnect && !this.reconnecting && code !== 1000) {
      this.attemptReconnect();
    } else {
      this.setState('disconnected');
    }
  }

  /**
   * Attempt to reconnect
   */
  private async attemptReconnect(): Promise<void> {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      logger.error(`Max reconnection attempts (${this.maxReconnectAttempts}) reached`);
      this.setState('error');
      return;
    }

    this.reconnecting = true;
    this.setState('reconnecting');
    this.reconnectAttempts++;

    const delay = this.reconnectDelay * Math.pow(this.reconnectBackoffMultiplier, this.reconnectAttempts - 1);
    logger.info(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts}/${this.maxReconnectAttempts})`);

    await sleep(delay);

    try {
      await this.connect(this.sessionConfig ?? undefined);
      logger.info('Reconnected successfully');
    } catch (error) {
      logger.error('Reconnection failed', error);
      this.attemptReconnect();
    } finally {
      this.reconnecting = false;
    }
  }

  /**
   * Set connection state
   */
  private setState(state: ConnectionState): void {
    if (this.state !== state) {
      this.state = state;
      this.emit('connectionStateChange', state);
    }
  }

  /**
   * Start ping timer
   */
  private startPingTimer(): void {
    this.stopPingTimer();
    this.pingTimer = setInterval(() => {
      this.sendPing();
    }, this.pingInterval);
  }

  /**
   * Stop ping timer
   */
  private stopPingTimer(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
    this.clearPongTimer();
  }

  /**
   * Send ping message
   */
  private sendPing(): void {
    if (!this.isConnected()) {
      return;
    }

    this.lastPingTime = Date.now();
    this.sendMessage({ type: 'ping', timestamp: this.lastPingTime });

    // Set pong timeout
    this.pongTimer = setTimeout(() => {
      logger.warn('Ping timeout - connection may be stale');
      this.ws?.close(4000, 'Ping timeout');
    }, this.pingTimeout);
  }

  /**
   * Clear pong timer
   */
  private clearPongTimer(): void {
    if (this.pongTimer) {
      clearTimeout(this.pongTimer);
      this.pongTimer = null;
    }
  }

  /**
   * Get metrics
   */
  getMetrics(): {
    messagesSent: number;
    messagesReceived: number;
    audioBytesSent: number;
    audioBytesReceived: number;
    latencyMs: number;
    reconnectAttempts: number;
  } {
    return {
      messagesSent: this.messagesSent,
      messagesReceived: this.messagesReceived,
      audioBytesSent: this.audioBytesSent,
      audioBytesReceived: this.audioBytesReceived,
      latencyMs: this.latencyMs,
      reconnectAttempts: this.reconnectAttempts,
    };
  }

  /**
   * Reset metrics
   */
  resetMetrics(): void {
    this.messagesSent = 0;
    this.messagesReceived = 0;
    this.audioBytesSent = 0;
    this.audioBytesReceived = 0;
  }

  // Type-safe event methods
  on<K extends keyof WebSocketClientEvents>(event: K, listener: WebSocketClientEvents[K]): this {
    return super.on(event, listener as (...args: unknown[]) => void);
  }

  once<K extends keyof WebSocketClientEvents>(event: K, listener: WebSocketClientEvents[K]): this {
    return super.once(event, listener as (...args: unknown[]) => void);
  }

  emit<K extends keyof WebSocketClientEvents>(event: K, ...args: Parameters<WebSocketClientEvents[K]>): boolean {
    return super.emit(event, ...args);
  }

  off<K extends keyof WebSocketClientEvents>(event: K, listener: WebSocketClientEvents[K]): this {
    return super.off(event, listener as (...args: unknown[]) => void);
  }
}
