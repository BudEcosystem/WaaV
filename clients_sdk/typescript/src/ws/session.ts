/**
 * WebSocket Session
 * High-level WebSocket session with automatic reconnection, message handling, and metrics
 */

import { ConnectionError, RateLimitError } from '../errors/index.js';
import type { STTConfig, TTSConfig, LiveKitConfig, DAGConfig, ConversationConfig, TurnDetectionConfig } from '../types/config.js';
import type { FeatureFlags } from '../types/features.js';
import type { IncomingMessage, STTResultMessage, TTSAudioMessage, ReadyMessage, ErrorMessage, ConfigWarningMessage } from '../types/messages.js';
import type { ConfigWarningEvent } from '../types/warnings.js';
import { PROTOCOL_VERSION } from '../types/messages.js';
import type { MetricsSummary } from '../types/metrics.js';
import { getMetricsCollector, MetricsCollector } from '../metrics/collector.js';
import { WebSocketConnection, type ConnectionState } from './connection.js';
import { ReconnectStrategy, type ReconnectConfig } from './reconnect.js';
import { createConfigMessage, createSpeakMessage, createClearMessage, createAudioEndMessage, createSendMessageMessage, type SDKOutgoingMessage } from './messages.js';
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
  /** DAG routing configuration. */
  dag?: DAGConfig;
  /**
   * Built-in conversation/agent loop (LLM + reasoning + barge-in + latency
   * filler). Serialized into `conversation_config`.
   */
  conversation?: ConversationConfig;
  /** ML turn detection (nested into `stt_config.turn_detection`). */
  turnDetection?: TurnDetectionConfig;
  /**
   * Proxy/alias name (P3): a server-defined name the gateway resolves to a full
   * {stt,tts,llm,dag} bundle before provider construction. The resolved concrete
   * providers come back on `ready` as `resolvedAlias`.
   */
  alias?: string;
  /** Stream identifier to request (wire: stream_id). */
  streamId?: string;
  /** Enable audio processing (STT/TTS). Defaults to true (gateway default). */
  audio?: boolean;
  /** Feature flags */
  features?: FeatureFlags;
  /**
   * @deprecated No longer used. Keepalive is handled by native WebSocket
   * ping/pong frames (the `ws` lib and browsers manage this automatically);
   * the SDK no longer sends a JSON ping. Kept only for backward compatibility.
   */
  pingInterval?: number;
  /** Whether to auto-send config on connect (default: true) */
  autoConfig?: boolean;
  /**
   * Behaviour when the gateway rate-limits the WebSocket upgrade (HTTP 429).
   * The gateway uses a per-IP token bucket that applies to WS upgrades, so a
   * burst of connects can be throttled. On 429 the SDK retries with jittered
   * exponential backoff, honouring any `Retry-After` header. Set
   * `maxRetries: 0` to disable and surface the RateLimitError immediately.
   */
  rateLimitRetry?: {
    /** Max retry attempts after the initial connect (default: 5) */
    maxRetries?: number;
    /** Base delay in ms when no Retry-After is provided (default: 500) */
    baseDelayMs?: number;
    /** Cap on any single backoff delay in ms (default: 20000) */
    maxDelayMs?: number;
  };
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
  private sessionId: string | null = null;
  private readyReceived = false;
  private lastReadyEvent: ReadyEvent | null = null;
  private sttConfig?: STTConfig;
  private ttsConfig?: TTSConfig;
  private livekitConfig?: LiveKitConfig;
  private dagConfig?: DAGConfig;
  private conversationConfig?: ConversationConfig;
  private turnDetectionConfig?: TurnDetectionConfig;
  private featuresConfig?: FeatureFlags;

  constructor(config: SessionConfig) {
    this.config = config;
    this.sttConfig = config.stt;
    this.ttsConfig = config.tts;
    this.livekitConfig = config.livekit;
    this.dagConfig = config.dag;
    this.conversationConfig = config.conversation;
    this.turnDetectionConfig = config.turnDetection;
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

    // NOTE: Keepalive is handled by NATIVE WebSocket ping/pong frames, which the
    // `ws` library (Node) and browsers manage automatically. We deliberately do
    // NOT send a JSON {type:'ping'} — the gateway has no such op and would emit a
    // parse-error every interval. (Removed in P0; see SDK_STANDARDIZATION_PLAN.)

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
      case 'config_warning':
        this.handleConfigWarning(message as ConfigWarningMessage);
        break;
      case 'pong':
        this.handlePong(message as { type: 'pong'; timestamp: number; server_time?: number });
        break;
    }
  }

  /**
   * Handle a non-fatal gateway config advisory. The session keeps running; we
   * surface it as a typed `configWarning` event so a silent server-side degrade
   * (unknown/misnested key, emotion ignored by the provider, reasoning model on
   * the voice path) becomes visible to the developer.
   */
  private handleConfigWarning(message: ConfigWarningMessage): void {
    this.metrics.increment('ws.configWarnings');
    const event: ConfigWarningEvent = {
      code: message.code,
      message: message.message,
      raw: message,
    };
    if (message.detail !== undefined) event.detail = message.detail;
    this.emitter.emit('configWarning', event);
  }

  /**
   * Handle ready message
   */
  private handleReady(message: ReadyMessage): void {
    this.readyReceived = true;
    this.sessionId = message.stream_id ?? null;

    const event: ReadyEvent = {
      streamId: message.stream_id,
      raw: message,
    };
    if (message.protocol_version !== undefined) event.protocolVersion = message.protocol_version;
    if (message.livekit_room_name !== undefined) event.livekitRoomName = message.livekit_room_name;
    if (message.livekit_url !== undefined) event.livekitUrl = message.livekit_url;
    if (message.waav_participant_identity !== undefined)
      event.waavParticipantIdentity = message.waav_participant_identity;
    if (message.waav_participant_name !== undefined)
      event.waavParticipantName = message.waav_participant_name;
    // P3: surface the resolved-alias echo so a developer SEES what their alias
    // (e.g. "support-bot") resolved to — and can re-point it server-side.
    if (message.resolved_alias !== undefined) event.resolvedAlias = message.resolved_alias;

    // Store the event for later retrieval by waitForReady()
    this.lastReadyEvent = event;

    // Assert the wire-protocol version matches. A mismatch means the gateway's
    // message contract has drifted from what this SDK expects, so surface it
    // loudly as a typed error event (recoverable) instead of letting fields
    // silently break (plan W-K1).
    if (message.protocol_version !== undefined && message.protocol_version !== PROTOCOL_VERSION) {
      this.emitter.emit('error', {
        code: 'PROTOCOL_VERSION_MISMATCH',
        message: `Gateway protocol_version "${message.protocol_version}" does not match SDK PROTOCOL_VERSION "${PROTOCOL_VERSION}". The wire contract may have changed; upgrade @bud-foundry/sdk.`,
        recoverable: true,
        raw: {
          type: 'error',
          code: 'PROTOCOL_VERSION_MISMATCH',
          message: `protocol_version mismatch: gateway=${message.protocol_version} sdk=${PROTOCOL_VERSION}`,
        },
      });
    }

    this.emitter.emit('ready', event);
  }

  /**
   * Handle STT result message
   */
  private handleSTTResult(message: STTResultMessage): void {
    if (message.is_final) {
      this.metrics.increment('stt.transcriptions');
      this.metrics.increment('stt.characters', message.transcript.length);
    }

    const event: TranscriptEvent = {
      text: message.transcript,
      isFinal: message.is_final,
      isSpeechFinal: message.is_speech_final,
      confidence: message.confidence,
      // P5: surface the uniform translations[] (camelCased) onto the event.
      translations: (message.translations ?? []).map((t) => {
        const out: { lang: string; text: string; isPartial?: boolean } = {
          lang: t.lang,
          text: t.text,
        };
        if (t.is_partial !== undefined) out.isPartial = t.is_partial;
        return out;
      }),
      raw: message,
    };
    if (message.segment_transcript !== undefined) {
      event.segmentTranscript = message.segment_transcript;
    }

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
      sampleRate: message.sample_rate ?? 24000,
      isFinal: message.is_final ?? false,
      raw: message,
    };
    if (message.duration !== undefined) event.duration = message.duration;
    if (message.sequence !== undefined) event.sequence = message.sequence;

    this.emitter.emit('audio', event);
  }

  /**
   * Handle error message
   */
  private handleErrorMessage(message: ErrorMessage): void {
    const event: SessionErrorEvent = {
      code: message.code ?? 'UNKNOWN',
      message: message.message,
      recoverable: message.recoverable ?? false,
      raw: message,
    };
    if (message.details !== undefined) event.details = message.details;

    this.emitter.emit('error', event);
  }

  /**
   * Handle pong message
   */
  private handlePong(message: { type: 'pong'; timestamp: number; server_time?: number }): void {
    // The gateway may echo a pong in response to an application-level message;
    // surface round-trip latency relative to that message's own timestamp.
    const latency = Date.now() - message.timestamp;
    const event: { latency: number; serverTime?: number } = { latency };
    if (message.server_time !== undefined) event.serverTime = message.server_time;
    this.emitter.emit('pong', event);
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
   * Flush queued messages
   */
  private flushQueue(): void {
    const messages = this.queue.drain();
    for (const { message, binaryData } of messages) {
      if (binaryData) {
        this.connection.sendBinary(binaryData);
      } else if (message) {
        this.connection.send(message);
      }
    }
  }

  /**
   * Send configuration message. Forwards the full 1:1 config surface: stt/tts/
   * livekit/dag/conversation, plus turn detection (nested into
   * stt_config.turn_detection by the serializer) and the audio/streamId knobs.
   */
  private sendConfig(): void {
    const hasConfig =
      this.sttConfig ||
      this.ttsConfig ||
      this.livekitConfig ||
      this.dagConfig ||
      this.conversationConfig ||
      this.turnDetectionConfig ||
      this.featuresConfig ||
      this.config.alias !== undefined ||
      this.config.audio !== undefined ||
      this.config.streamId !== undefined;
    if (hasConfig) {
      const configMessage = createConfigMessage(
        this.sttConfig,
        this.ttsConfig,
        this.livekitConfig,
        this.featuresConfig,
        {
          dag: this.dagConfig,
          conversation: this.conversationConfig,
          turnDetection: this.turnDetectionConfig,
          alias: this.config.alias,
          streamId: this.config.streamId,
          audio: this.config.audio,
        }
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

    const maxRetries = this.config.rateLimitRetry?.maxRetries ?? 5;
    const baseDelayMs = this.config.rateLimitRetry?.baseDelayMs ?? 500;
    const maxDelayMs = this.config.rateLimitRetry?.maxDelayMs ?? 20000;

    let attempt = 0;
    // Retry loop that only triggers on a typed 429 RateLimitError. All other
    // errors propagate immediately.
    for (;;) {
      try {
        await this.connection.connect();
        break;
      } catch (err) {
        if (err instanceof RateLimitError && attempt < maxRetries) {
          this.metrics.increment('ws.rateLimited');
          const delay = this.computeBackoffMs(err.retryAfterMs, attempt, baseDelayMs, maxDelayMs);
          this.emitter.emit('reconnect', {
            state: { attempt: attempt + 1, delay, lastConnectedAt: null, reconnecting: true, exhausted: false },
            event: 'reconnecting',
          });
          attempt++;
          await new Promise((resolve) => setTimeout(resolve, delay));
          continue;
        }
        this.state = 'disconnected';
        this.metrics.setWSState('disconnected');
        throw err;
      }
    }

    const duration = Date.now() - startTime;
    this.metrics.record('ws.connect', duration);
  }

  /**
   * Compute a jittered backoff delay for a rate-limited (429) connect attempt.
   * Honours an explicit Retry-After when present; otherwise uses capped
   * exponential backoff. A small random jitter (±20%) avoids a thundering herd
   * of clients all retrying in lockstep.
   */
  private computeBackoffMs(retryAfterMs: number | undefined, attempt: number, baseDelayMs: number, maxDelayMs: number): number {
    const base = retryAfterMs !== undefined
      ? retryAfterMs
      : Math.min(baseDelayMs * Math.pow(2, attempt), maxDelayMs);
    const jitter = base * 0.2 * (Math.random() * 2 - 1);
    return Math.max(0, Math.round(Math.min(base + jitter, maxDelayMs)));
  }

  /**
   * Disconnect from server
   */
  async disconnect(): Promise<void> {
    this.reconnectStrategy?.abort();
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
  send(message: SDKOutgoingMessage): void {
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
      // Always copy out the exact slice into a fresh ArrayBuffer. This both
      // avoids sending a larger backing buffer when the view is a window, and
      // narrows ArrayBufferLike (which may be a SharedArrayBuffer) to a plain
      // ArrayBuffer for the WS send path.
      const copy = new Uint8Array(data.byteLength);
      copy.set(data);
      buffer = copy.buffer;
    } else {
      buffer = data;
    }

    this.metrics.increment('ws.bytesSent', buffer.byteLength);

    if (this.connection.isConnected()) {
      this.connection.sendBinary(buffer);
    } else {
      // Raw binary audio frame — no JSON message wrapper.
      this.queue.enqueue(undefined, buffer);
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
   * Barge-in / cancel: stop the bot's current TTS playback immediately.
   * Sends the gateway `clear` op.
   */
  clear(): void {
    this.send(createClearMessage());
  }

  /**
   * Finalize: tell the gateway the inbound audio stream has ended so it
   * flushes any pending transcript. Sends the gateway `audio_end` op.
   */
  audioEnd(): void {
    this.send(createAudioEndMessage());
  }

  /**
   * Send a data-channel message (gateway `send_message` op).
   */
  sendMessage(message: string, role: string, options?: { topic?: string; debug?: Record<string, unknown> }): void {
    this.send(createSendMessageMessage(message, role, options));
  }

  /**
   * @deprecated Use {@link clear} (barge-in/cancel). The gateway has no `stop`
   * op; this now maps to `clear`.
   */
  stop(): void {
    this.clear();
  }

  /**
   * @deprecated Use {@link audioEnd} (finalize). The gateway has no `flush`
   * op; this now maps to `audio_end`.
   */
  flush(): void {
    this.audioEnd();
  }

  /**
   * @deprecated Use {@link clear} (barge-in). The gateway has no `interrupt`
   * op; this now maps to `clear`.
   */
  interrupt(): void {
    this.clear();
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
        reject(new ConnectionError('Timeout waiting for ready state', { context: { timeout } }));
      }, timeout);

      this.once('ready', handler);
    });
  }
}
