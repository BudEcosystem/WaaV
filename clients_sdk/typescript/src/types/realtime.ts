/**
 * Realtime (Audio-to-Audio) Types
 *
 * This module provides types for real-time audio-to-audio streaming,
 * abstracting provider-specific details behind a unified interface.
 */

/**
 * Supported realtime providers
 */
export type RealtimeProvider = 'openai';

/**
 * OpenAI Realtime model options
 */
export type OpenAIRealtimeModel =
  | 'gpt-4o-realtime-preview'
  | 'gpt-4o-realtime-preview-2024-10-01'
  | 'gpt-4o-mini-realtime-preview'
  | 'gpt-4o-mini-realtime-preview-2024-12-17';

/**
 * OpenAI voice options for realtime
 */
export type OpenAIRealtimeVoice =
  | 'alloy'
  | 'ash'
  | 'ballad'
  | 'coral'
  | 'echo'
  | 'sage'
  | 'shimmer'
  | 'verse';

/**
 * Voice Activity Detection configuration
 */
export interface VADConfig {
  /** Enable server-side VAD (default: true) */
  enabled?: boolean;
  /** VAD threshold (0.0 to 1.0, default: 0.5) */
  threshold?: number;
  /** Silence duration before speech end detection in ms (default: 500) */
  silenceDurationMs?: number;
  /** Prefix padding in ms (default: 300) */
  prefixPaddingMs?: number;
}

/**
 * Input audio transcription configuration
 */
export interface InputTranscriptionConfig {
  /** Enable input audio transcription (default: true) */
  enabled?: boolean;
  /** Model to use for transcription (default: "whisper-1") */
  model?: string;
}

/**
 * Provider-agnostic realtime session configuration
 *
 * This interface abstracts away provider-specific details while exposing
 * common functionality. Advanced users can access provider-specific options
 * through the `providerOptions` field.
 */
export interface RealtimeSessionConfig {
  /** Provider to use (currently only "openai" supported) */
  provider: RealtimeProvider;

  /**
   * Model to use (provider-specific)
   *
   * OpenAI: "gpt-4o-realtime-preview", "gpt-4o-mini-realtime-preview"
   */
  model?: string;

  /**
   * Voice to use for audio output
   *
   * OpenAI: "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"
   */
  voice?: string;

  /** System instructions for the AI assistant */
  instructions?: string;

  /** Voice Activity Detection configuration */
  vad?: VADConfig;

  /** Input audio transcription configuration */
  inputTranscription?: InputTranscriptionConfig;

  /** Turn detection mode */
  turnDetection?: 'server_vad' | 'none';

  /** Temperature for response generation (0.0 to 2.0) */
  temperature?: number;

  /** Maximum tokens for response (provider-specific limits apply) */
  maxResponseTokens?: number | 'inf';

  /**
   * Provider-specific options for advanced users
   *
   * These options are passed directly to the provider and may vary
   * between providers. Refer to provider documentation for details.
   */
  providerOptions?: Record<string, unknown>;
}

/**
 * Realtime transcript result
 */
export interface RealtimeTranscript {
  /** The transcribed or generated text */
  text: string;

  /** Role: "user" for input transcription, "assistant" for AI response */
  role: 'user' | 'assistant';

  /** Whether this is a final transcript (vs interim/streaming) */
  isFinal: boolean;

  /** Item ID from the provider (for correlation) */
  itemId?: string;

  /** Response ID from the provider (for correlation) */
  responseId?: string;

  /** Timestamp when transcript was received */
  timestamp: number;
}

/**
 * Speech event (speech started/stopped)
 */
export interface SpeechEvent {
  /** Event type */
  type: 'speech_started' | 'speech_stopped';

  /** Audio position in ms when event occurred */
  audioMs: number;

  /** Item ID from the provider */
  itemId?: string;

  /** Timestamp when event was received */
  timestamp: number;
}

/**
 * Realtime audio data chunk
 */
export interface RealtimeAudioChunk {
  /** Raw PCM audio data (24kHz, mono, 16-bit little-endian) */
  data: ArrayBuffer;

  /** Sample rate (always 24000 for OpenAI) */
  sampleRate: number;

  /** Number of channels (always 1 for mono) */
  channels: number;

  /** Whether this is the final chunk for this response */
  isFinal: boolean;

  /** Response ID from the provider */
  responseId?: string;

  /** Item ID from the provider */
  itemId?: string;

  /** Sequence number for ordering */
  sequence: number;

  /** Timestamp when chunk was received */
  timestamp: number;
}

/**
 * Realtime session state
 */
export type RealtimeSessionState =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'ready'
  | 'error';

/**
 * Realtime session events
 */
export interface RealtimeSessionEvents {
  /** Session is ready for audio input/output */
  ready: () => void;

  /** Received transcript (user input or assistant response) */
  transcript: (transcript: RealtimeTranscript) => void;

  /** Received audio chunk from assistant */
  audio: (chunk: RealtimeAudioChunk) => void;

  /** Speech event (started/stopped) */
  speech: (event: SpeechEvent) => void;

  /** Session state changed */
  stateChange: (state: RealtimeSessionState) => void;

  /** Error occurred */
  error: (error: Error) => void;

  /** Session disconnected */
  disconnect: (reason?: string) => void;
}

/**
 * Realtime session interface
 *
 * This interface provides a unified API for interacting with realtime
 * audio-to-audio providers. It abstracts protocol details and provides
 * a simple event-driven interface.
 */
export interface IRealtimeSession {
  /** Current session state */
  readonly state: RealtimeSessionState;

  /** Current configuration */
  readonly config: RealtimeSessionConfig;

  /**
   * Connect to the realtime service
   */
  connect(): Promise<void>;

  /**
   * Disconnect from the realtime service
   */
  disconnect(): Promise<void>;

  /**
   * Send audio data for processing
   *
   * @param audio - PCM audio data (16kHz or 24kHz depending on provider)
   */
  sendAudio(audio: ArrayBuffer): void;

  /**
   * Send text message to the assistant
   *
   * @param text - Text message to send
   */
  sendText(text: string): void;

  /**
   * Trigger response generation (if using manual turn detection)
   */
  createResponse(): void;

  /**
   * Cancel the current response generation
   */
  cancelResponse(): void;

  /**
   * Clear the audio input buffer
   */
  clearAudioBuffer(): void;

  /**
   * Commit the audio buffer (signal end of speech)
   */
  commitAudioBuffer(): void;

  /**
   * Update session configuration
   *
   * @param config - Partial configuration to update
   */
  updateConfig(config: Partial<RealtimeSessionConfig>): Promise<void>;

  /**
   * Register event listener
   */
  on<K extends keyof RealtimeSessionEvents>(
    event: K,
    listener: RealtimeSessionEvents[K]
  ): void;

  /**
   * Remove event listener
   */
  off<K extends keyof RealtimeSessionEvents>(
    event: K,
    listener: RealtimeSessionEvents[K]
  ): void;
}

/**
 * Default realtime configuration by provider
 */
export const REALTIME_DEFAULTS: Record<
  RealtimeProvider,
  Partial<RealtimeSessionConfig>
> = {
  openai: {
    model: 'gpt-4o-realtime-preview',
    voice: 'alloy',
    turnDetection: 'server_vad',
    vad: {
      enabled: true,
      threshold: 0.5,
      silenceDurationMs: 500,
      prefixPaddingMs: 300,
    },
    inputTranscription: {
      enabled: true,
      model: 'whisper-1',
    },
    temperature: 0.8,
    maxResponseTokens: 'inf',
  },
};

/**
 * Create a realtime session configuration with defaults
 *
 * @param provider - The realtime provider to use
 * @param overrides - Configuration overrides
 * @returns Complete session configuration
 */
export function createRealtimeConfig(
  provider: RealtimeProvider,
  overrides?: Partial<RealtimeSessionConfig>
): RealtimeSessionConfig {
  return {
    ...REALTIME_DEFAULTS[provider],
    provider,
    ...overrides,
  };
}
