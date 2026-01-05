/**
 * STT (Speech-to-Text) Configuration
 * Maps to Sayna's STTWebSocketConfig in src/handlers/ws/config.rs
 */
export interface STTConfig {
  /** Provider name (e.g., "deepgram", "google", "elevenlabs", "microsoft-azure", "cartesia") */
  provider: string;
  /** Language code for transcription (e.g., "en-US", "es-ES") */
  language?: string;
  /** Sample rate of the audio in Hz (default: 16000) */
  sampleRate?: number;
  /** Number of audio channels (1 for mono, 2 for stereo, default: 1) */
  channels?: number;
  /** Enable punctuation in results (default: true) */
  punctuation?: boolean;
  /** Alias for punctuation (Deepgram style) */
  punctuate?: boolean;
  /** Encoding of the audio (default: "linear16") */
  encoding?: string;
  /** Model to use for transcription (e.g., "nova-2", "nova-3") */
  model?: string;
  /** Enable interim/partial results */
  interimResults?: boolean;
  /** Enable profanity filter */
  profanityFilter?: boolean;
  /** Enable smart formatting */
  smartFormat?: boolean;
  /** Enable speaker diarization */
  diarize?: boolean;
  /** Keywords to boost recognition */
  keywords?: string[];
  /** Custom vocabulary */
  customVocabulary?: string[];
  /** Endpointing settings */
  endpointing?: number;
  /** Utterance end timeout in ms */
  utteranceEndMs?: number;
}

/**
 * TTS (Text-to-Speech) Configuration
 * Maps to Sayna's TTSWebSocketConfig in src/handlers/ws/config.rs
 */
export interface TTSConfig {
  /** Provider name (e.g., "deepgram", "elevenlabs", "google", "azure", "cartesia") */
  provider: string;
  /** Voice name */
  voice?: string;
  /** Voice ID or name to use for synthesis */
  voiceId?: string;
  /** Speaking rate (0.25 to 4.0, 1.0 is normal) */
  speakingRate?: number;
  /** Alias for speakingRate */
  speed?: number;
  /** Pitch adjustment */
  pitch?: number;
  /** Volume adjustment */
  volume?: number;
  /** Audio format preference (e.g., "linear16", "mp3", "wav") */
  audioFormat?: string;
  /** Sample rate preference (e.g., 16000, 24000, 48000) */
  sampleRate?: number;
  /** Connection timeout in seconds */
  connectionTimeout?: number;
  /** Request timeout in seconds */
  requestTimeout?: number;
  /** Model to use for TTS */
  model?: string;
  /** Pronunciation replacements to apply before TTS */
  pronunciations?: Pronunciation[];
  /** Voice stability (ElevenLabs specific, 0-1) */
  stability?: number;
  /** Voice similarity boost (ElevenLabs specific, 0-1) */
  similarityBoost?: number;
  /** Voice style (ElevenLabs specific, 0-1) */
  style?: number;
  /** Use speaker boost (ElevenLabs specific) */
  useSpeakerBoost?: boolean;
}

/**
 * Pronunciation replacement rule
 */
export interface Pronunciation {
  /** Text pattern to match */
  from: string;
  /** Replacement text */
  to: string;
}

/**
 * LiveKit Configuration
 * Maps to Sayna's LiveKitWebSocketConfig in src/handlers/ws/config.rs
 */
export interface LiveKitConfig {
  /** Room name to join or create */
  roomName: string;
  /** Enable recording for this session */
  enableRecording?: boolean;
  /** Sayna AI participant identity (defaults to "sayna-ai") */
  saynaParticipantIdentity?: string;
  /** Sayna AI participant display name (defaults to "Sayna AI") */
  saynaParticipantName?: string;
  /** List of participant identities to listen to for audio tracks and data messages */
  listenParticipants?: string[];
}

/**
 * Complete WebSocket session configuration
 */
export interface SessionConfig {
  /** Optional unique identifier for this WebSocket session */
  streamId?: string;
  /** Enable audio processing (STT/TTS). Defaults to true */
  audio?: boolean;
  /** STT configuration (required when audio=true) */
  sttConfig?: STTConfig;
  /** TTS configuration (required when audio=true) */
  ttsConfig?: TTSConfig;
  /** LiveKit configuration for real-time audio streaming */
  livekit?: LiveKitConfig;
}

/**
 * Default STT configuration values
 */
export const DEFAULT_STT_CONFIG: Partial<STTConfig> = {
  sampleRate: 16000,
  channels: 1,
  punctuation: true,
  encoding: 'linear16',
};

/**
 * Default TTS configuration values
 */
export const DEFAULT_TTS_CONFIG: Partial<TTSConfig> = {
  sampleRate: 24000,
  audioFormat: 'linear16',
  speakingRate: 1.0,
};

/**
 * Create a complete STT config with defaults
 */
export function createSTTConfig(config: Partial<STTConfig> & Pick<STTConfig, 'provider' | 'language' | 'model'>): STTConfig {
  return {
    ...DEFAULT_STT_CONFIG,
    ...config,
  } as STTConfig;
}

/**
 * Create a complete TTS config with defaults
 */
export function createTTSConfig(config: Partial<TTSConfig> & Pick<TTSConfig, 'provider' | 'model'>): TTSConfig {
  return {
    ...DEFAULT_TTS_CONFIG,
    ...config,
  } as TTSConfig;
}
