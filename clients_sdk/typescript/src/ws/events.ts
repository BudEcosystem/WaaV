/**
 * WebSocket Session Events
 * Type-safe event definitions for session callbacks
 */

import type { STTResultMessage, ErrorMessage, TTSAudioMessage, ReadyMessage } from '../types/messages.js';
import type { MetricsSummary } from '../types/metrics.js';
import type { ReconnectState } from './reconnect.js';

/**
 * STT transcript event
 */
export interface TranscriptEvent {
  /** Transcribed text */
  text: string;
  /** Whether this is a final result */
  isFinal: boolean;
  /** Confidence score (0-1) */
  confidence?: number;
  /** Speaker ID for diarization */
  speakerId?: number;
  /** Detected language */
  language?: string;
  /** Start time in seconds */
  startTime?: number;
  /** End time in seconds */
  endTime?: number;
  /** Word-level details */
  words?: Array<{
    word: string;
    start: number;
    end: number;
    confidence?: number;
    speakerId?: number;
  }>;
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
  /** Session ID */
  sessionId?: string;
  /** Whether STT is ready */
  sttReady: boolean;
  /** Whether TTS is ready */
  ttsReady: boolean;
  /** Whether LiveKit is connected */
  livekitConnected: boolean;
  /** Server version */
  serverVersion?: string;
  /** Available capabilities */
  capabilities: string[];
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
