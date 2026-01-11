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

/** Turn detection configuration */
export interface TurnDetectionConfig {
  enabled: boolean;
  threshold?: number; // 0.0-1.0
  silenceMs?: number; // Silence duration
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
  /** Realtime configuration (for audio-to-audio mode) */
  realtime?: RealtimeConfig;
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
