/**
 * WebSocket Session Events
 * Type-safe event definitions for session callbacks
 */

import type { STTResultMessage, ErrorMessage, TTSAudioMessage, ReadyMessage, ResolvedAlias, ConfigWarningMessage } from '../types/messages.js';
import type { MetricsSummary } from '../types/metrics.js';
import type { ReconnectState } from './reconnect.js';

export type { ConfigWarningEvent, ConfigWarningCode } from '../types/warnings.js';
import type { ConfigWarningEvent } from '../types/warnings.js';

/**
 * STT transcript event
 */
export interface TranscriptEvent {
  /** Transcribed text (gateway `transcript` field) */
  text: string;
  /** Whether this is a final result */
  isFinal: boolean;
  /** Whether speech has ended (gateway `is_speech_final`) */
  isSpeechFinal: boolean;
  /** Confidence score (0-1) */
  confidence?: number;
  /**
   * The FULL accumulated segment text, present only on a speech_final whose
   * segment spans multiple finals. Prefer this over `text` when displaying
   * per-final text (gateway `segment_transcript`).
   */
  segmentTranscript?: string;
  /** Speaker ID for diarization (reserved; not yet emitted on /ws) */
  speakerId?: number;
  /** Detected language (reserved; not yet emitted on /ws) */
  language?: string;
  /** Start time in seconds (reserved; not yet emitted on /ws) */
  startTime?: number;
  /** End time in seconds (reserved; not yet emitted on /ws) */
  endTime?: number;
  /** Word-level details (reserved; not yet emitted on /ws) */
  words?: Array<{
    word: string;
    start: number;
    end: number;
    confidence?: number;
    speakerId?: number;
  }>;
  /**
   * Uniform in-stream translations merged onto this transcript (P5). Empty
   * unless a translation-capable provider (Speechmatics/Gladia/OpenAI EN fast
   * path) returned a `translations:[{lang,text}]` array on this stt_result frame
   * (gateway `translations`).
   */
  translations: Array<{ lang: string; text: string; isPartial?: boolean }>;
  /** Original message for advanced use */
  raw: STTResultMessage;
}

/**
 * TTS audio event
 */
export interface AudioEvent {
  /** Audio data (PCM) */
  audio: ArrayBuffer;
  /** Audio format */
  format: string;
  /** Sample rate in Hz */
  sampleRate: number;
  /** Duration in seconds */
  duration?: number;
  /** Whether this is the final chunk */
  isFinal: boolean;
  /** Sequence number for ordering */
  sequence?: number;
  /** Original message for advanced use */
  raw: TTSAudioMessage;
}

/**
 * Session ready event
 */
export interface ReadyEvent {
  /** Unique stream/session identifier assigned by the gateway */
  streamId: string;
  /**
   * Wire-protocol version reported by the gateway (e.g. "1.0"). The SDK
   * asserts this matches its own PROTOCOL_VERSION and emits a typed error
   * event on mismatch.
   */
  protocolVersion?: string;
  /** LiveKit room name that was created (if LiveKit was requested) */
  livekitRoomName?: string;
  /** LiveKit URL to connect to (if LiveKit was requested) */
  livekitUrl?: string;
  /** Identity of the AI agent participant in the room */
  waavParticipantIdentity?: string;
  /** Display name of the AI agent participant */
  waavParticipantName?: string;
  /**
   * P3 proxy/alias echo: the concrete providers the gateway resolved an `alias`
   * to (no secrets), e.g. `{ name: 'support-bot', tts: { provider: 'cartesia' } }`.
   * Present only when an `alias` was sent and recognized.
   */
  resolvedAlias?: ResolvedAlias;
  /** Original message for advanced use */
  raw: ReadyMessage;
}

/**
 * Error event
 */
export interface SessionErrorEvent {
  /** Error code */
  code: string;
  /** Error message */
  message: string;
  /** Additional error details */
  details?: Record<string, unknown>;
  /** Whether the error is recoverable */
  recoverable: boolean;
  /** Original message for advanced use */
  raw: ErrorMessage;
}

/**
 * Connection state change event
 */
export interface ConnectionStateEvent {
  /** Previous state */
  previousState: 'disconnected' | 'connecting' | 'connected' | 'reconnecting';
  /** Current state */
  currentState: 'disconnected' | 'connecting' | 'connected' | 'reconnecting';
  /** Timestamp of state change */
  timestamp: number;
}

/**
 * Metrics update event
 */
export interface MetricsEvent {
  /** Current metrics summary */
  metrics: MetricsSummary;
  /** Timestamp of update */
  timestamp: number;
}

/**
 * Reconnection event
 */
export interface ReconnectEvent {
  /** Reconnection state */
  state: ReconnectState;
  /** Event type */
  event: 'reconnecting' | 'reconnected' | 'failed' | 'exhausted';
  /** Error if event is 'failed' */
  error?: Error;
}

/**
 * Speaking state event
 */
export interface SpeakingEvent {
  /** Whether speaking started or finished */
  speaking: boolean;
  /** Timestamp */
  timestamp: number;
}

/**
 * Listening state event
 */
export interface ListeningEvent {
  /** Whether listening started or stopped */
  listening: boolean;
  /** Timestamp */
  timestamp: number;
}

/**
 * D10 mic-silence event: the microphone has been below the silence threshold
 * for the configured window (likely muted/dead) — or has recovered. Mirrors
 * Pipecat base_input's mic-timeout signal.
 */
export interface MicSilenceEvent {
  /** True when the mic went silent, false when audio returned. */
  silent: boolean;
  /** How long the mic was silent in ms (only meaningful when `silent` is true). */
  silentForMs?: number;
  /** Timestamp */
  timestamp: number;
}

/**
 * Session event map for type-safe event handling
 */
export interface SessionEventMap {
  /** Session is ready */
  ready: ReadyEvent;
  /** Transcript received */
  transcript: TranscriptEvent;
  /** Audio received */
  audio: AudioEvent;
  /** Error occurred */
  error: SessionErrorEvent;
  /**
   * Non-fatal gateway config advisory (e.g. an unknown/misnested key, an
   * emotion the provider ignores, a reasoning model on the voice path). The
   * session keeps running; this surfaces a silent server-side degrade so a
   * developer can see and fix it.
   */
  configWarning: ConfigWarningEvent;
  /** Connection state changed */
  connectionState: ConnectionStateEvent;
  /** Metrics updated */
  metrics: MetricsEvent;
  /** Reconnection event */
  reconnect: ReconnectEvent;
  /** Speaking state changed */
  speaking: SpeakingEvent;
  /** Listening state changed */
  listening: ListeningEvent;
  /** D10: mic went silent (likely muted/dead) or recovered. */
  micSilent: MicSilenceEvent;
  /** Session closed */
  close: { code: number; reason: string };
  /** Ping/pong roundtrip */
  pong: { latency: number; serverTime?: number };
}

/**
 * Event handler type
 */
export type SessionEventHandler<K extends keyof SessionEventMap> = (event: SessionEventMap[K]) => void;

/**
 * Type-safe event emitter for session events
 */
export class SessionEventEmitter {
  private handlers: Map<keyof SessionEventMap, Set<SessionEventHandler<keyof SessionEventMap>>> = new Map();

  /**
   * Add event listener
   */
  on<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): () => void {
    if (!this.handlers.has(event)) {
      this.handlers.set(event, new Set());
    }
    this.handlers.get(event)!.add(handler as SessionEventHandler<keyof SessionEventMap>);

    // Return unsubscribe function
    return () => this.off(event, handler);
  }

  /**
   * Add one-time event listener
   */
  once<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): () => void {
    const wrappedHandler = ((e: SessionEventMap[K]) => {
      this.off(event, wrappedHandler as SessionEventHandler<K>);
      handler(e);
    }) as SessionEventHandler<K>;

    return this.on(event, wrappedHandler);
  }

  /**
   * Remove event listener
   */
  off<K extends keyof SessionEventMap>(event: K, handler: SessionEventHandler<K>): void {
    const eventHandlers = this.handlers.get(event);
    if (eventHandlers) {
      eventHandlers.delete(handler as SessionEventHandler<keyof SessionEventMap>);
    }
  }

  /**
   * Emit event
   */
  emit<K extends keyof SessionEventMap>(event: K, data: SessionEventMap[K]): void {
    const eventHandlers = this.handlers.get(event);
    if (eventHandlers) {
      for (const handler of eventHandlers) {
        try {
          (handler as SessionEventHandler<K>)(data);
        } catch (err) {
          console.error(`Error in event handler for ${String(event)}:`, err);
        }
      }
    }
  }

  /**
   * Remove all listeners for an event or all events
   */
  removeAllListeners(event?: keyof SessionEventMap): void {
    if (event) {
      this.handlers.delete(event);
    } else {
      this.handlers.clear();
    }
  }

  /**
   * Get listener count for an event
   */
  listenerCount(event: keyof SessionEventMap): number {
    return this.handlers.get(event)?.size ?? 0;
  }
}
