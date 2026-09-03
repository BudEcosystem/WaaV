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
import { WebSocketConnection, DEFAULT_CONNECT_TIMEOUT_MS, type ConnectionState } from './connection.js';
import { ReconnectStrategy, type ReconnectConfig } from './reconnect.js';
import { LivenessWatchdog, type LivenessConfig } from './liveness.js';
import { UplinkAudioShedder, type UplinkConfig } from './uplink.js';
import { KeepaliveSilence, type KeepaliveConfig } from './keepalive.js';
import type { ConnectGate } from './connect-gate.js';
import { createConfigMessage, createSpeakMessage, createClearMessage, createAudioEndMessage, createSendMessageMessage, type SDKOutgoingMessage } from './messages.js';
import { MessageQueue, type MessageQueueConfig } from './queue.js';
import { SessionEventEmitter, type SessionEventMap, type SessionEventHandler, type TranscriptEvent, type AudioEvent, type ReadyEvent, type SessionErrorEvent } from './events.js';

/**
 * Extract the MAJOR component of a dotted protocol version string (e.g.
 * "1.0" -> "1", "2.3.1" -> "2"). Used to grade a protocol_version drift: a
 * different major is a breaking error, a same-major drift is a soft warning.
 */
function majorOf(version: string): string {
  const dot = version.indexOf('.');
  return dot === -1 ? version : version.slice(0, dot);
}

/**
 * Session configuration
 */
export interface SessionConfig {
  /** WebSocket URL */
  url: string;
  /** API key for authentication */
  apiKey?: string;
  /**
   * Connection timeout in milliseconds (default: 35000). D6: must exceed the
   * gateway's ~30s PROVIDER_READY for an audio=true session (config_handler.rs:45)
   * — a smaller value fails an otherwise-healthy connect mid-provider-build.
   */
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
   * Liveness / zombie-socket watchdog tuning (D1). The gateway never pings
   * clients, so the SDK detects a dead-but-OPEN socket itself: Node sends native
   * WS pings (pong deadline); the browser watches for stale inbound frames and
   * probes on network/visibility resume. Pass `false` to disable, or an object
   * to override the thresholds. Enabled by default.
   */
  liveness?: LivenessConfig | false;
  /**
   * Uplink audio backpressure shedding (D5). When the gateway/socket can't keep
   * up, outbound audio frames are bounded in a drop-oldest ring and only handed
   * to the socket while its `bufferedAmount` is below a high-water mark — so a
   * slow gateway never accumulates uplink latency (mirrors the gateway's own
   * drop-oldest egress). Pass `false` to send every frame immediately, or an
   * object to tune the ring size / high-water mark. Enabled by default.
   */
  uplink?: UplinkConfig | false;
  /**
   * STT keepalive-silence (D6). On an `audio` session, the gateway's auto-pong
   * keeps the socket open during a conversational pause but NOT the upstream
   * provider STT stream warm; this sends a short zero-PCM frame every few idle
   * seconds so the next word after a lull doesn't pay a stream-warmup penalty.
   * It is GATED on real audio (never overlaps genuine frames) and only runs when
   * `audio !== false`. Pass `false` to disable, or an object to tune the idle
   * gap / frame size. Enabled by default for audio sessions.
   */
  keepalive?: KeepaliveConfig | false;
  /**
   * Shared WS connect-concurrency gate (D7). When a burst of sessions connects
   * at once they can self-DDoS the gateway's per-IP 60rps/burst10 bucket into a
   * multi-minute 429/503 storm; a gate (one instance shared across all sessions
   * from a client) caps concurrent connects + paces their starts so the burst
   * cooperates with the bucket. Injected by {@link BudClient}; absent for a
   * standalone session (no cap).
   */
  connectGate?: ConnectGate;
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
  /** D1: per-socket liveness watchdog (recreated after each (re)connect). */
  private liveness: LivenessWatchdog | null = null;
  /** D5: uplink audio shedder (bounded drop-oldest ring + bufferedAmount gate). */
  private uplink: UplinkAudioShedder | null = null;
  /** D5: timer that keeps draining the uplink ring while frames remain. */
  private uplinkPumpTimer: ReturnType<typeof setInterval> | null = null;
  /** D6: STT keepalive-silence pump (audio sessions only; gated on real audio). */
  private keepalive: KeepaliveSilence | null = null;
  /** D1 verify-after-reconnect: true while we await proof the new socket carries data. */
  private awaitingReconnectProof = false;
  /** D1: bound browser network/visibility listeners, for teardown. */
  private networkListenersBound = false;
  private readonly onNetworkOnline = (): void => this.handleNetworkResume('online');
  private readonly onNetworkOffline = (): void => this.handleNetworkOffline();
  private readonly onVisibility = (): void => {
    if (typeof document !== 'undefined' && document.visibilityState === 'visible') {
      this.handleNetworkResume('visibilitychange');
    }
  };

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
      // D6: default to the 35s connect timeout so an audio=true session's
      // server-side PROVIDER_READY (up to ~30s) isn't tripped as a failure.
      url: wsUrl,
      timeout: config.connectionTimeout ?? DEFAULT_CONNECT_TIMEOUT_MS,
      WebSocket: config.WebSocket,
    });

    this.reconnectStrategy = config.reconnect !== false
      ? new ReconnectStrategy(typeof config.reconnect === 'object' ? config.reconnect : undefined)
      : null;

    this.queue = new MessageQueue(config.queue);
    this.emitter = new SessionEventEmitter();
    this.metrics = getMetricsCollector();

    // D5: uplink audio shedder (drop-oldest ring + bufferedAmount-gated drain).
    if (config.uplink !== false) {
      this.uplink = new UplinkAudioShedder(
        {
          send: (frame) => this.connection.sendBinary(frame),
          bufferedAmount: () => this.connection.bufferedAmount(),
          isConnected: () => this.connection.isConnected(),
        },
        typeof config.uplink === 'object' ? config.uplink : {}
      );
    }

    // D6: STT keepalive-silence — only for audio sessions (audio !== false), and
    // only when not explicitly disabled. Sends zero-PCM during a silence gap so
    // the upstream STT stream stays warm; gated on real audio in sendAudio().
    if (config.keepalive !== false && config.audio !== false) {
      const kaCfg: KeepaliveConfig = typeof config.keepalive === 'object' ? config.keepalive : {};
      // Default the keepalive frame's sample rate to the configured STT rate so
      // the zero-PCM matches the ingress format the gateway expects.
      if (kaCfg.sampleRate === undefined && config.stt?.sampleRate !== undefined) {
        kaCfg.sampleRate = config.stt.sampleRate;
      }
      this.keepalive = new KeepaliveSilence(
        {
          // Route keepalive frames straight to the socket (bypassing the uplink
          // shedder): they are tiny, must not be shed, and must not reset the
          // shedder's fast path. They never overlap real audio (gated below).
          sendAudio: (frame) => this.connection.sendBinary(frame),
          isConnected: () => this.connection.isConnected(),
        },
        kaCfg
      );
    }

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
      // D1: a native WS pong (Node) proves the socket is alive — feed the watchdog.
      onPong: () => this.liveness?.markPong(),
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
        onReconnectFatal: (state) => {
          // D4: the quick-failure breaker tripped — stop the storm and surface a
          // typed FATAL error so the app shows "gateway unreachable / check
          // key/config" instead of silently retrying forever.
          this.metrics.increment('ws.reconnectFatal');
          this.emitter.emit('reconnect', { state, event: 'failed' });
          this.emitter.emit('error', {
            code: 'RECONNECT_FATAL',
            message: `Gateway unreachable: ${state.consecutiveQuickFailures} consecutive connections each failed within the stability window. Reconnection has been abandoned; check the gateway URL, API key, and config.`,
            recoverable: false,
            raw: {
              type: 'error',
              code: 'RECONNECT_FATAL',
              message: `quick-failure breaker tripped after ${state.consecutiveQuickFailures} unstable connections`,
            },
          });
        },
      });
    }
  }

  /**
   * Handle connection open
   */
  private handleOpen(): void {
    const previousState = this.state;
    // If this open follows a reconnecting state, we must PROVE the new socket
    // carries data before declaring it healthy (verify-after-reconnect).
    const wasReconnect = previousState === 'reconnecting';
    this.state = 'connected';
    this.metrics.setWSState('connected');
    this.reconnectStrategy?.markConnected();

    this.emitter.emit('connectionState', {
      previousState: previousState as 'disconnected' | 'connecting' | 'connected' | 'reconnecting',
      currentState: 'connected',
      timestamp: Date.now(),
    });

    // D1: Keepalive is NOT a JSON ping (the gateway has no such op). Instead the
    // liveness watchdog uses native WS pings (Node) or stale-inbound detection
    // (browser). Start a fresh watchdog for this socket and (in the browser)
    // subscribe network/visibility resume probes.
    this.bindNetworkListeners();
    this.startLiveness();

    // Verify-after-reconnect: require inbound data on the NEW socket before
    // treating the reconnect as healed. On Node, ping immediately so a frozen
    // peer is caught fast; either the pong or the `ready` ack clears the flag.
    if (wasReconnect) {
      this.awaitingReconnectProof = true;
      this.connection.ping();
    }

    // Send queued messages
    this.flushQueue();

    // Send config if auto-config enabled
    if (this.config.autoConfig !== false) {
      this.sendConfig();
    }

    // D6: (re)start the STT keepalive-silence pump for this socket. Idempotent;
    // it only emits during a genuine idle gap and only while connected.
    this.keepalive?.start();
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
    // D1: the socket is gone — stop its watchdog so its timers don't fire on a
    // dead/replaced socket. A fresh watchdog is created on the next open. The
    // network listeners stay bound across a reconnect cycle (they drive resume
    // probes); they are only removed on an explicit disconnect().
    this.stopLiveness();
    // D5: drop any in-flight uplink frames — the gateway session is gone and a
    // reconnect starts a fresh one, so stale mid-utterance audio is loss-tolerant.
    this.stopUplinkPump();
    this.uplink?.clear();
    // D6: pause the keepalive pump; a fresh `ready`/open restarts it.
    this.keepalive?.stop();

    this.emitter.emit('close', { code, reason });
    this.emitter.emit('connectionState', {
      previousState: previousState as 'disconnected' | 'connecting' | 'connected' | 'reconnecting',
      currentState: 'disconnected',
      timestamp: Date.now(),
    });

    // Attempt reconnection if enabled and not a clean close
    if (this.reconnectStrategy && code !== 1000) {
      // D4: record this close for the quick-failure breaker BEFORE deciding to
      // reconnect — a sub-minStableMs survival counts toward the FATAL trip and
      // gates even the fast first-hop. Then re-check shouldReconnect (the
      // breaker may already have tripped on a prior close).
      this.reconnectStrategy.noteConnectionClosed();
      if (this.reconnectStrategy.shouldReconnect()) {
        this.reconnectStrategy.scheduleReconnect(() => this.connection.connect()).catch((err) => {
          // A fatal-breaker throw is surfaced via onReconnectFatal (below) as a
          // distinct typed event; this generic path covers abort/exhaust/other.
          if (this.reconnectStrategy?.getState().fatal) return;
          this.emitter.emit('error', {
            code: 'RECONNECT_FAILED',
            message: err instanceof Error ? err.message : 'Reconnection failed',
            recoverable: false,
            raw: { type: 'error', code: 'RECONNECT_FAILED', message: String(err) },
          });
        });
      }
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
   * D1: record an inbound frame for the liveness watchdog and resolve a pending
   * verify-after-reconnect (any inbound data on the new socket proves it is live,
   * not merely OPEN — the Pipecat `_verify_connection` discipline).
   */
  private noteInbound(): void {
    this.liveness?.markInbound();
    if (this.awaitingReconnectProof) {
      this.awaitingReconnectProof = false;
      this.metrics.increment('ws.reconnectVerified');
    }
  }

  /**
   * D1: start a fresh liveness watchdog for the current socket. Called once the
   * socket is open + (re)configured. Idempotent per socket; a prior watchdog is
   * stopped first so timers never overlap across reconnects.
   */
  private startLiveness(): void {
    if (this.config.liveness === false) return;
    this.stopLiveness();
    const cfg = typeof this.config.liveness === 'object' ? this.config.liveness : {};
    this.liveness = new LivenessWatchdog(
      {
        ping: () => this.connection.ping(),
        isPingCapable: () => this.connection.isPingCapable(),
        onZombie: (reason) => this.handleZombie(reason),
      },
      cfg
    );
    this.liveness.start();
  }

  /** D1: stop and drop the current liveness watchdog. */
  private stopLiveness(): void {
    if (this.liveness) {
      this.liveness.stop();
      this.liveness = null;
    }
  }

  /**
   * D1: the watchdog judged the socket dead (missed pong, or stale inbound).
   * Terminate the zombie socket immediately (a graceful close can hang on a
   * frozen peer) and fall through to the EXISTING reconnect path via onClose.
   */
  private handleZombie(reason: string): void {
    this.stopLiveness();
    this.metrics.increment('ws.zombieDetected');
    this.emitter.emit('error', {
      code: 'LIVENESS_ZOMBIE',
      message: `Connection appears dead (${reason}); terminating and reconnecting.`,
      recoverable: true,
      raw: { type: 'error', code: 'LIVENESS_ZOMBIE', message: reason },
    });
    // terminate() fires onClose -> handleClose -> scheduleReconnect (with code
    // 4000 != 1000, so reconnect is NOT suppressed).
    this.connection.terminate();
  }

  /**
   * D1 network-change: on `online` / tab-foreground, fire an immediate liveness
   * probe so a socket that died while the NIC was down / the tab was hidden is
   * healed promptly instead of after the full watchdog window.
   */
  private handleNetworkResume(_source: 'online' | 'visibilitychange'): void {
    this.liveness?.probeNow();
  }

  /**
   * D1 network-change: on `offline`, stop the watchdog from firing into a known
   * down NIC (avoids a reconnect storm against an unreachable network). The
   * watchdog resumes/heals on the paired `online`.
   */
  private handleNetworkOffline(): void {
    this.stopLiveness();
  }

  /** D1: subscribe browser network/visibility events (no-op under Node). */
  private bindNetworkListeners(): void {
    if (this.networkListenersBound) return;
    if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
      window.addEventListener('online', this.onNetworkOnline);
      window.addEventListener('offline', this.onNetworkOffline);
      this.networkListenersBound = true;
    }
    if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
      document.addEventListener('visibilitychange', this.onVisibility);
    }
  }

  /** D1: unsubscribe browser network/visibility events. */
  private unbindNetworkListeners(): void {
    if (typeof window !== 'undefined' && typeof window.removeEventListener === 'function') {
      window.removeEventListener('online', this.onNetworkOnline);
      window.removeEventListener('offline', this.onNetworkOffline);
    }
    if (typeof document !== 'undefined' && typeof document.removeEventListener === 'function') {
      document.removeEventListener('visibilitychange', this.onVisibility);
    }
    this.networkListenersBound = false;
  }

  /**
   * Handle incoming message
   */
  private handleMessage(message: IncomingMessage): void {
    this.metrics.increment('ws.received');
    // D1: every inbound frame proves the socket carries data (resets the
    // browser staleness clock; satisfies a Node pong deadline) and confirms a
    // pending verify-after-reconnect.
    this.noteInbound();

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
      // NOTE: the gateway never sends a JSON `pong` (verified: no JSON ping op).
      // Native WS pong control frames are handled out-of-band by the liveness
      // watchdog via the connection `onPong` hook — there is no JSON pong case.
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
    // D8: surface the negotiated transport codecs so callers (and the opus wiring) can detect a
    // downgrade — if `opus` was requested but the gateway echoes `linear16`, send/decode linear16.
    if (message.audio_in_codec !== undefined) event.audioInCodec = message.audio_in_codec;
    if (message.audio_out_codec !== undefined) event.audioOutCodec = message.audio_out_codec;

    // Store the event for later retrieval by waitForReady()
    this.lastReadyEvent = event;

    // Assert the wire-protocol version on `ready`. A drift means the gateway's
    // message contract has moved from what this SDK expects (plan W-K1). We
    // grade by SEVERITY: a MAJOR mismatch (e.g. gateway 2.x vs SDK 1.x) is a
    // breaking, recoverable error event; a minor/patch drift (same major) is a
    // softer configWarning so a forward-compatible bump isn't alarming.
    if (message.protocol_version !== undefined && message.protocol_version !== PROTOCOL_VERSION) {
      const gwMajor = majorOf(message.protocol_version);
      const sdkMajor = majorOf(PROTOCOL_VERSION);
      if (gwMajor !== sdkMajor) {
        this.emitter.emit('error', {
          code: 'PROTOCOL_VERSION_MISMATCH',
          message: `Gateway protocol_version "${message.protocol_version}" has a MAJOR mismatch with SDK PROTOCOL_VERSION "${PROTOCOL_VERSION}". The wire contract may have changed incompatibly; upgrade @bud-foundry/sdk.`,
          recoverable: true,
          raw: {
            type: 'error',
            code: 'PROTOCOL_VERSION_MISMATCH',
            message: `protocol_version major mismatch: gateway=${message.protocol_version} sdk=${PROTOCOL_VERSION}`,
          },
        });
      } else {
        this.emitter.emit('configWarning', {
          code: 'PROTOCOL_VERSION_MINOR_DRIFT',
          message: `Gateway protocol_version "${message.protocol_version}" differs from SDK PROTOCOL_VERSION "${PROTOCOL_VERSION}" (same major; likely forward-compatible).`,
          raw: {
            type: 'config_warning',
            code: 'PROTOCOL_VERSION_MINOR_DRIFT',
            message: `protocol_version minor drift: gateway=${message.protocol_version} sdk=${PROTOCOL_VERSION}`,
          },
        });
      }
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
   * Handle binary message (audio frames)
   */
  private handleBinaryMessage(data: ArrayBuffer): void {
    this.metrics.increment('ws.bytesReceived', data.byteLength);
    // D1: TTS audio arrives as raw binary frames — they prove liveness too.
    this.noteInbound();

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

    // D7: route the connect through the shared concurrency gate when present, so
    // a burst of sessions paces under the gateway's per-IP rate bucket instead
    // of stampeding it. The whole rate-limit-retry loop runs inside one slot.
    try {
      if (this.config.connectGate) {
        await this.config.connectGate.run(() => this.connectWithRateLimitRetry());
      } else {
        await this.connectWithRateLimitRetry();
      }
    } catch (err) {
      this.state = 'disconnected';
      this.metrics.setWSState('disconnected');
      throw err;
    }

    const duration = Date.now() - startTime;
    this.metrics.record('ws.connect', duration);
  }

  /**
   * The connect attempt with the typed-429/503 back-off retry loop (D7). Only a
   * RateLimitError (per-IP 429 throttle OR global 503 "Server at capacity") is
   * retried — both are back-off-not-fatal; every other error propagates
   * immediately. Honours any Retry-After via {@link computeBackoffMs}.
   */
  private async connectWithRateLimitRetry(): Promise<void> {
    const maxRetries = this.config.rateLimitRetry?.maxRetries ?? 5;
    const baseDelayMs = this.config.rateLimitRetry?.baseDelayMs ?? 500;
    const maxDelayMs = this.config.rateLimitRetry?.maxDelayMs ?? 20000;

    let attempt = 0;
    for (;;) {
      try {
        await this.connection.connect();
        return;
      } catch (err) {
        if (err instanceof RateLimitError && attempt < maxRetries) {
          // statusCode distinguishes 429 (per-IP throttle) from 503 (gateway at
          // capacity); both back off here, neither is treated as fatal.
          this.metrics.increment(err.isAtCapacity() ? 'ws.serverAtCapacity' : 'ws.rateLimited');
          const delay = this.computeBackoffMs(err.retryAfterMs, attempt, baseDelayMs, maxDelayMs);
          this.emitter.emit('reconnect', {
            state: {
              attempt: attempt + 1,
              delay,
              lastConnectedAt: null,
              reconnecting: true,
              exhausted: false,
              consecutiveQuickFailures: 0,
              fatal: false,
            },
            event: 'reconnecting',
          });
          attempt++;
          await new Promise((resolve) => setTimeout(resolve, delay));
          continue;
        }
        throw err;
      }
    }
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
    // D1: tear down the watchdog + network listeners on an explicit disconnect.
    this.stopLiveness();
    this.unbindNetworkListeners();
    this.awaitingReconnectProof = false;
    // D5: stop + clear the uplink shedder.
    this.stopUplinkPump();
    this.uplink?.clear();
    // D6: stop the keepalive pump on explicit disconnect.
    this.keepalive?.stop();
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

    // D6: this is a REAL audio frame — reset the keepalive idle clock so the
    // keepalive-silence pump never overlaps genuine audio (it only fills gaps).
    this.keepalive?.noteRealAudio();

    if (this.connection.isConnected()) {
      // D5: route uplink audio through the backpressure shedder so a slow socket
      // sheds the OLDEST frame instead of accumulating latency. When disabled,
      // send immediately.
      if (this.uplink) {
        this.uplink.push(buffer);
        this.ensureUplinkPump();
      } else {
        this.connection.sendBinary(buffer);
      }
    } else {
      // Raw binary audio frame — no JSON message wrapper. Offline buffering is
      // the MessageQueue's drop-oldest ring (flushed on reconnect open).
      this.queue.enqueue(undefined, buffer);
    }
  }

  /**
   * D5: keep a low-frequency pump running while the uplink ring has frames, so a
   * socket that drains AFTER the last push still gets the remaining audio. The
   * timer stops itself once the ring empties (no idle wakeups during silence).
   */
  private ensureUplinkPump(): void {
    if (!this.uplink || this.uplinkPumpTimer) return;
    if (!this.uplink.hasPending()) return;
    this.uplinkPumpTimer = setInterval(() => {
      if (!this.uplink) {
        this.stopUplinkPump();
        return;
      }
      this.uplink.pump();
      if (!this.uplink.hasPending()) {
        this.stopUplinkPump();
      }
    }, 10);
  }

  /** D5: stop the uplink drain pump timer. */
  private stopUplinkPump(): void {
    if (this.uplinkPumpTimer) {
      clearInterval(this.uplinkPumpTimer);
      this.uplinkPumpTimer = null;
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
   * D5: uplink shedder statistics — frames currently queued in the pre-socket
   * ring and the total shed (oldest-dropped) under backpressure. `{0,0}` when
   * shedding is disabled.
   */
  getUplinkStats(): { pending: number; droppedFrames: number } {
    return {
      pending: this.uplink?.size() ?? 0,
      droppedFrames: this.uplink?.getDroppedFrames() ?? 0,
    };
  }

  /**
   * D6: STT keepalive-silence statistics — how many zero-PCM keepalive frames
   * have been sent (and the per-frame byte size). `{0,0}` when keepalive is
   * disabled or this is not an audio session.
   */
  getKeepaliveStats(): { sent: number; frameBytes: number } {
    return {
      sent: this.keepalive?.getSentCount() ?? 0,
      frameBytes: this.keepalive?.frameBytes() ?? 0,
    };
  }

  /**
   * Wait for ready state.
   *
   * D6: defaults to the 35s connect timeout, not 10s. The gateway only sends
   * `ready` AFTER PROVIDER_READY, which for an audio=true session can take up to
   * ~30s while it builds the STT/TTS upstreams (config_handler.rs:45); a 10s
   * default would reject on a perfectly-healthy-but-still-warming session.
   */
  waitForReady(timeout = DEFAULT_CONNECT_TIMEOUT_MS): Promise<ReadyEvent> {
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
