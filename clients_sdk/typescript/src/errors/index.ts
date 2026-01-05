/**
 * Error classes for @bud-foundry/sdk
 */

// Base error
export {
  BudError,
  BudErrorCode,
  isBudError,
  getErrorCodeName,
} from './base.js';

// Connection errors
export {
  ConnectionError,
  TimeoutError,
  ReconnectError,
  ConnectionClosedError,
} from './connection.js';

// API errors
export {
  APIError,
  ConfigurationError,
} from './api.js';

// STT errors
export {
  STTError,
  STTProviderError,
  TranscriptionError,
  AudioFormatError,
} from './stt.js';

// TTS errors
export {
  TTSError,
  TTSProviderError,
  SynthesisError,
  VoiceNotFoundError,
} from './tts.js';
