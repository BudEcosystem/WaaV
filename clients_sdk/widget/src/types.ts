/**
 * Widget configuration types
 */

export interface WidgetConfig {
  /** WebSocket URL of the gateway */
  gatewayUrl: string;
  /** API key for authentication */
  apiKey?: string;
  /** STT configuration */
  stt?: STTConfig;
  /** TTS configuration */
  tts?: TTSConfig;
  /** UI theme */
  theme?: 'light' | 'dark' | 'auto';
  /** Widget position */
  position?: 'bottom-right' | 'bottom-left' | 'top-right' | 'top-left';
  /** Voice activation mode */
  mode?: 'push-to-talk' | 'vad';
  /** Show metrics overlay */
  showMetrics?: boolean;
  /** Feature flags */
  features?: FeatureFlags;
  /** Custom CSS */
  customCss?: string;
}

export interface STTConfig {
  provider: string;
  language?: string;
  model?: string;
  sampleRate?: number;
  channels?: number;
  encoding?: string;
}

export interface TTSConfig {
  provider: string;
  voice?: string;
  voiceId?: string;
  model?: string;
  sampleRate?: number;
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
