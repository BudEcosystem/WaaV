/**
 * Widget configuration types
 */

// =============================================================================
// Provider Types
// =============================================================================

/** All supported STT providers */
export type STTProvider =
  | 'deepgram'
  | 'google'
  | 'azure'
  | 'cartesia'
  | 'gateway'
  | 'assemblyai'
  | 'aws-transcribe'
  | 'ibm-watson'
  | 'groq'
  | 'openai-whisper';

/** All supported TTS providers */
export type TTSProvider =
  | 'deepgram'
  | 'elevenlabs'
  | 'google'
  | 'azure'
  | 'cartesia'
  | 'openai'
  | 'aws-polly'
  | 'ibm-watson'
  | 'hume'
  | 'lmnt'
  | 'playht'
  | 'kokoro';

/** Realtime providers */
export type RealtimeProvider = 'openai-realtime' | 'hume-evi';

/**
 * Reasoning/thinking-effort dial for the conversation LLM.
 * Mapped server-side to each vendor's native thinking control (gateway config.rs reasoning_effort).
 */
export type ReasoningEffort = 'off' | 'minimal' | 'low' | 'medium' | 'high';

/**
 * Latency-masking mode for slow LLM/reasoning turns.
 * 'auto' speaks one short filler phrase when first audio is slow (gateway config.rs latency_filler).
 */
export type LatencyFiller = 'off' | 'auto' | 'aggressive';

// =============================================================================
// Emotion System
// =============================================================================

/**
 * Emotion types for TTS.
 * Matches Python SDK's Emotion enum for cross-SDK consistency.
 */
export type EmotionType =
  | 'neutral'
  | 'happy'
  | 'sad'
  | 'angry'
  | 'fearful'
  | 'surprised'
  | 'disgusted'
  | 'excited'
  | 'calm'
  | 'anxious'
  | 'confident'
  | 'confused'
  | 'empathetic'
  | 'sarcastic'
  | 'hopeful'
  | 'disappointed'
  | 'curious'
  | 'grateful'
  | 'proud'
  | 'embarrassed'
  | 'content'
  | 'bored';

/**
 * Delivery styles for TTS.
 * Matches Python SDK's DeliveryStyle enum for cross-SDK consistency.
 */
export type DeliveryStyle =
  | 'normal'
  | 'whispered'
  | 'shouted'
  | 'rushed'
  | 'measured'
  | 'monotone'
  | 'expressive'
  | 'professional'
  | 'casual'
  | 'storytelling'
  | 'soft'
  | 'loud'
  | 'cheerful'
  | 'serious'
  | 'formal';

/** Emotion intensity */
export type EmotionIntensity = 'low' | 'medium' | 'high' | number;

/** Emotion configuration */
export interface EmotionConfig {
  emotion?: EmotionType;
  intensity?: EmotionIntensity;
  deliveryStyle?: DeliveryStyle;
  description?: string; // Max 100 chars (Hume)
}

// =============================================================================
// Audio Features
// =============================================================================

/**
 * ML turn-detection configuration.
 *
 * Maps to the gateway's `stt_config.turn_detection` (provider-agnostic smart-turn
 * detector). The gateway accepts only `enabled`, `threshold`, and `eager`
 * (gateway config.rs TurnDetectionWsConfig); `silenceMs`/`prefixPaddingMs` are
 * NOT wire keys on /ws and are intentionally not serialized.
 */
export interface TurnDetectionConfig {
  enabled: boolean;
  threshold?: number; // 0.0-1.0
  /** Eager end-of-turn: start the LLM speculatively on a turn-complete prediction. */
  eager?: boolean;
  /** @deprecated Not a /ws wire key; ignored when sending stt_config.turn_detection. */
  silenceMs?: number;
  /** @deprecated Not a /ws wire key; ignored when sending stt_config.turn_detection. */
  prefixPaddingMs?: number;
}

/** Noise filter configuration */
export interface NoiseFilterConfig {
  enabled: boolean;
  strength?: 'low' | 'medium' | 'high';
}

/** VAD (Voice Activity Detection) configuration */
export interface VADConfig {
  enabled: boolean;
  threshold?: number; // 0.0-1.0
  silenceMs?: number;
}

// =============================================================================
// Main Configuration
// =============================================================================

export interface WidgetConfig {
  /** WebSocket URL of the gateway */
  gatewayUrl: string;
  /** API key for authentication */
  apiKey?: string;
  /** STT configuration */
  stt?: STTConfig;
  /** TTS configuration */
  tts?: TTSConfig;
  /** Realtime configuration (for the separate /realtime S2S endpoint; NOT sent on /ws) */
  realtime?: RealtimeConfig;
  /**
   * Conversation-loop (LLM) configuration. When present, the gateway runs the
   * built-in STT -> LLM -> TTS loop so the bot talks back. Serialized to the
   * gateway `conversation_config` envelope.
   */
  conversation?: ConversationConfig;
  /** UI theme */
  theme?: 'light' | 'dark' | 'auto';
  /** Widget position */
  position?: 'bottom-right' | 'bottom-left' | 'top-right' | 'top-left';
  /** Voice activation mode */
  mode?: 'push-to-talk' | 'vad' | 'realtime';
  /** Show metrics overlay */
  showMetrics?: boolean;
  /** Feature flags */
  features?: FeatureFlags;
  /** Audio features */
  audioFeatures?: AudioFeatures;
  /** Custom CSS */
  customCss?: string;
}

export interface STTConfig {
  provider: STTProvider;
  language?: string;
  model?: string;
  sampleRate?: number;
  channels?: number;
  encoding?: string;
}

export interface TTSConfig {
  provider: TTSProvider;
  voice?: string;
  voiceId?: string;
  model?: string;
  sampleRate?: number;
  /** Emotion configuration */
  emotion?: EmotionConfig;
}

export interface RealtimeConfig {
  provider: RealtimeProvider;
  model?: string;
  systemPrompt?: string;
  voiceId?: string;
  temperature?: number;
  maxTokens?: number;
  /** EVI version (Hume) */
  eviVersion?: string;
  /** Enable verbose transcription (Hume) */
  verboseTranscription?: boolean;
  /** Resume from previous chat group (Hume) */
  resumedChatGroupId?: string;
  /** Input audio transcription config (OpenAI) */
  inputAudioTranscription?: {
    model?: string;
  };
  /** Turn detection config for realtime mode */
  turnDetection?: TurnDetectionConfig;
}

export interface AudioFeatures {
  turnDetection?: TurnDetectionConfig;
  noiseFilter?: NoiseFilterConfig;
  vad?: VADConfig;
}

/**
 * Conversation-loop (LLM) configuration -> gateway `conversation_config`.
 *
 * Drives the built-in STT -> LLM -> TTS loop. `baseUrl` + `model` are required
 * by the gateway (ConversationWebSocketConfig); everything else is optional and
 * maps 1:1 to the conversation_config fields, including the REALTIME_REASONING
 * dials (reasoningEffort, reasoningModel, latencyFiller, eagerEot).
 */
export interface ConversationConfig {
  /** OpenAI-compatible base URL, e.g. "http://localhost:11434/v1" (required). */
  baseUrl: string;
  /** Model identifier, e.g. "qwen2.5:7b-instruct" (required). */
  model: string;
  /** Optional system prompt seeding the conversation. */
  systemPrompt?: string;
  /** API key (literal or "${ENV_VAR}"); falls back to OPENAI_API_KEY server-side. */
  apiKey?: string;
  /** Sampling temperature. */
  temperature?: number;
  /** Max tokens per completion. */
  maxTokens?: number;
  /** Stream tokens to TTS as they arrive (gateway default true). */
  streaming?: boolean;
  /** Whether the bot's speech is interruptible / barge-in (gateway default true). */
  allowInterruption?: boolean;
  /** Eager end-of-turn: start the LLM speculatively on a turn-complete prediction. */
  eagerEot?: boolean;
  /** While the bot speaks, require >= N words to interrupt it. */
  bargeInMinWords?: number;
  /** Reasoning/thinking-effort dial. */
  reasoningEffort?: ReasoningEffort;
  /** Latency-masking mode for slow first audio. */
  latencyFiller?: LatencyFiller;
  /** Optional slow REASONING-tier model (keep `model` fast). */
  reasoningModel?: string;
}

export interface FeatureFlags {
  vad?: boolean;
  noiseCancellation?: boolean;
  speakerDiarization?: boolean;
  interimResults?: boolean;
  punctuation?: boolean;
  profanityFilter?: boolean;
  smartFormat?: boolean;
  echoCancellation?: boolean;
}

export interface TranscriptResult {
  text: string;
  isFinal: boolean;
  confidence?: number;
  speakerId?: number;
}

export interface AudioChunk {
  audio: ArrayBuffer;
  format: string;
  sampleRate: number;
  isFinal?: boolean;
}

export interface WidgetMetrics {
  sttTtft?: number;
  ttsTtfb?: number;
  e2eLatency?: number;
  messagesReceived: number;
  messagesSent: number;
}

export type WidgetState = 'idle' | 'connecting' | 'connected' | 'listening' | 'speaking' | 'error';

export interface WidgetEventMap {
  'ready': CustomEvent<{ streamId: string }>;
  'transcript': CustomEvent<TranscriptResult>;
  'audio': CustomEvent<AudioChunk>;
  'stateChange': CustomEvent<{ state: WidgetState; previousState: WidgetState }>;
  'metrics': CustomEvent<WidgetMetrics>;
  'error': CustomEvent<Error>;
}
