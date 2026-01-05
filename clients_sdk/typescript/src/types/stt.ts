/**
 * STT (Speech-to-Text) Result Types
 */

/**
 * Word-level timing information
 */
export interface WordTiming {
  /** The word text */
  word: string;
  /** Start time in seconds */
  start: number;
  /** End time in seconds */
  end: number;
  /** Confidence score (0.0 to 1.0) */
  confidence: number;
}

/**
 * Speaker information for diarization
 */
export interface SpeakerInfo {
  /** Speaker identifier (e.g., "0", "1", "speaker_0") */
  id: string;
  /** Confidence score for speaker identification */
  confidence?: number;
}

/**
 * STT transcription result
 */
export interface STTResult {
  /** Transcribed text */
  transcript: string;
  /** Whether this is a final result (not interim) */
  isFinal: boolean;
  /** Whether speech has ended (end of utterance) */
  isSpeechFinal: boolean;
  /** Overall confidence score (0.0 to 1.0) */
  confidence: number;
  /** Word-level timing information (if word_timestamps enabled) */
  words?: WordTiming[];
  /** Speaker information (if speaker_diarization enabled) */
  speaker?: SpeakerInfo;
  /** Timestamp when result was received (ms since epoch) */
  timestamp: number;
  /** Duration of audio processed in seconds */
  audioDuration?: number;
}

/**
 * STT transcript event emitted by session
 */
export interface TranscriptEvent {
  /** The transcription result */
  result: STTResult;
  /** Whether this is an interim (partial) result */
  isInterim: boolean;
  /** Unique ID for this utterance */
  utteranceId?: string;
}

/**
 * STT connection options
 */
export interface STTConnectOptions {
  /** Provider name (e.g., "deepgram", "google", "elevenlabs", "microsoft-azure", "cartesia") */
  provider: string;
  /** Language code (e.g., "en-US", "es-ES", "fr-FR") */
  language: string;
  /** Model name (e.g., "nova-2", "nova-3", "latest_long") */
  model: string;
  /** Sample rate of input audio in Hz (default: 16000) */
  sampleRate?: number;
  /** Number of audio channels (default: 1) */
  channels?: number;
  /** Audio encoding (default: "linear16") */
  encoding?: string;
  /** Feature flags for additional processing */
  features?: STTFeatures;
}

/**
 * STT feature flags
 */
export interface STTFeatures {
  /** Enable Voice Activity Detection (default: true) */
  vad?: boolean;
  /** Enable speaker diarization (Deepgram only) */
  speakerDiarization?: boolean;
  /** Enable interim/partial results (default: true) */
  interimResults?: boolean;
  /** Enable auto-punctuation (default: true) */
  punctuation?: boolean;
  /** Enable word-level timestamps */
  wordTimestamps?: boolean;
  /** Enable profanity filter (Deepgram, Azure) */
  profanityFilter?: boolean;
  /** Enable smart formatting (Deepgram) */
  smartFormat?: boolean;
  /** Include filler words (um, uh) (Deepgram) */
  fillerWords?: boolean;
}

/**
 * Default STT features
 */
export const DEFAULT_STT_FEATURES: STTFeatures = {
  vad: true,
  interimResults: true,
  punctuation: true,
};

/**
 * Parse an STT result from server message
 */
export function parseSTTResult(msg: {
  transcript: string;
  is_final: boolean;
  is_speech_final: boolean;
  confidence: number;
}): STTResult {
  return {
    transcript: msg.transcript,
    isFinal: msg.is_final,
    isSpeechFinal: msg.is_speech_final,
    confidence: msg.confidence,
    timestamp: Date.now(),
  };
}
