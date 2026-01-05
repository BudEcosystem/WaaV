/**
 * TTS (Text-to-Speech) Types
 */

/**
 * Voice information from provider
 */
export interface Voice {
  /** Unique voice identifier */
  id: string;
  /** Display name of the voice */
  name: string;
  /** Provider this voice belongs to */
  provider: string;
  /** Language code(s) supported */
  languages?: string[];
  /** Voice gender (if available) */
  gender?: 'male' | 'female' | 'neutral';
  /** Voice age category */
  age?: 'child' | 'young' | 'adult' | 'senior';
  /** Voice accent/style description */
  accent?: string;
  /** Sample audio URL (if available) */
  sampleUrl?: string;
  /** Whether this is a premium voice */
  premium?: boolean;
}

/**
 * TTS audio chunk
 */
export interface TTSAudioChunk {
  /** Raw PCM audio data */
  data: ArrayBuffer;
  /** Sample rate of the audio */
  sampleRate: number;
  /** Number of channels */
  channels: number;
  /** Audio format (e.g., "linear16") */
  format: string;
  /** Sequence number for ordering */
  sequence: number;
  /** Whether this is the final chunk */
  isFinal: boolean;
}

/**
 * TTS speak options
 */
export interface SpeakOptions {
  /** Text to synthesize */
  text: string;
  /** Flush TTS buffer immediately */
  flush?: boolean;
  /** Allow this TTS to be interrupted */
  allowInterruption?: boolean;
}

/**
 * TTS connection options
 */
export interface TTSConnectOptions {
  /** Provider name (e.g., "deepgram", "elevenlabs", "google", "azure", "cartesia") */
  provider: string;
  /** Voice ID to use */
  voice?: string;
  /** Model name */
  model: string;
  /** Speaking rate (0.25 to 4.0, 1.0 is normal) */
  speakingRate?: number;
  /** Sample rate preference (e.g., 16000, 24000, 48000) */
  sampleRate?: number;
  /** Audio format preference */
  audioFormat?: string;
  /** Pronunciation replacements */
  pronunciations?: Array<{ from: string; to: string }>;
}

/**
 * TTS synthesis result from REST API
 */
export interface TTSSynthesisResult {
  /** Audio data as ArrayBuffer */
  audio: ArrayBuffer;
  /** Audio format */
  format: string;
  /** Sample rate */
  sampleRate: number;
  /** Duration in seconds */
  duration: number;
  /** Characters processed */
  characters: number;
}

/**
 * TTS playback complete event
 */
export interface TTSPlaybackCompleteEvent {
  /** Timestamp when playback completed */
  timestamp: number;
  /** Text that was spoken */
  text?: string;
  /** Duration of playback in ms */
  durationMs?: number;
}

/**
 * Voice list response
 */
export interface VoiceListResponse {
  /** List of available voices */
  voices: Voice[];
  /** Provider this list is from */
  provider?: string;
}

/**
 * Provider-specific voice configurations
 */
export const VOICE_DEFAULTS: Record<string, { model: string; voice?: string }> = {
  deepgram: {
    model: 'aura-asteria-en',
    voice: 'aura-asteria-en',
  },
  elevenlabs: {
    model: 'eleven_turbo_v2',
    voice: 'rachel',
  },
  google: {
    model: 'en-US-Studio-O',
    voice: 'en-US-Studio-O',
  },
  azure: {
    model: 'en-US-JennyNeural',
    voice: 'en-US-JennyNeural',
  },
  cartesia: {
    model: 'sonic-3',
    voice: undefined, // Requires voice_id from Cartesia library
  },
};
