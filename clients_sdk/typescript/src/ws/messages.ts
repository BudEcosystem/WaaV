/**
 * WebSocket Message Serialization/Deserialization
 * Handles conversion between TypeScript types and wire format
 */

import type {
  IncomingMessage,
  OutgoingMessage,
  SpeakMessage,
  ReadyMessage,
  STTResultMessage,
  TTSAudioMessage,
  ErrorMessage,
  PongMessage,
  SessionUpdateMessage,
  MessageType,
} from '../types/messages.js';
import type { STTConfig, TTSConfig, LiveKitConfig } from '../types/config.js';
import type { FeatureFlags } from '../types/features.js';

// ============================================================================
// Type validation helpers - provide runtime safety for wire data
// ============================================================================

/**
 * Safely get a string value from unknown data
 */
function asString(value: unknown, defaultValue?: string): string | undefined {
  if (typeof value === 'string') return value;
  if (defaultValue !== undefined) return defaultValue;
  return undefined;
}

/**
 * Safely get a required string value from unknown data
 */
function asStringRequired(value: unknown, fieldName: string): string {
  if (typeof value === 'string') return value;
  console.warn(`Expected string for ${fieldName}, got ${typeof value}`);
  return String(value ?? '');
}

/**
 * Safely get a boolean value from unknown data
 */
function asBoolean(value: unknown, defaultValue?: boolean): boolean | undefined {
  if (typeof value === 'boolean') return value;
  if (defaultValue !== undefined) return defaultValue;
  return undefined;
}

/**
 * Safely get a required boolean value from unknown data
 */
function asBooleanRequired(value: unknown, fieldName: string, defaultValue = false): boolean {
  if (typeof value === 'boolean') return value;
  if (value === undefined || value === null) return defaultValue;
  console.warn(`Expected boolean for ${fieldName}, got ${typeof value}`);
  return Boolean(value);
}

/**
 * Safely get a number value from unknown data
 */
function asNumber(value: unknown, defaultValue?: number): number | undefined {
  if (typeof value === 'number' && !Number.isNaN(value)) return value;
  if (defaultValue !== undefined) return defaultValue;
  return undefined;
}

/**
 * Safely get a required number value from unknown data
 */
function asNumberRequired(value: unknown, fieldName: string, defaultValue = 0): number {
  if (typeof value === 'number' && !Number.isNaN(value)) return value;
  if (value === undefined || value === null) return defaultValue;
  console.warn(`Expected number for ${fieldName}, got ${typeof value}`);
  const parsed = Number(value);
  return Number.isNaN(parsed) ? defaultValue : parsed;
}

/**
 * Safely get an array of strings from unknown data
 */
function asStringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) return undefined;
  return value.filter((item): item is string => typeof item === 'string');
}

/**
 * Safely get a record from unknown data
 */
function asRecord(value: unknown): Record<string, unknown> | undefined {
  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return undefined;
}

/**
 * SDK-facing ConfigMessage with camelCase fields.
 * This is different from the wire format which uses snake_case.
 */
export interface SDKConfigMessage {
  type: 'config';
  streamId?: string;
  audio?: boolean;
  stt?: STTConfig;
  tts?: TTSConfig;
  livekit?: LiveKitConfig;
  features?: FeatureFlags;
}

/**
 * Extended OutgoingMessage type that includes SDK-facing types
 */
export type SDKOutgoingMessage = OutgoingMessage | SDKConfigMessage;

/**
 * Serialize outgoing message to JSON string
 */
export function serializeMessage(message: SDKOutgoingMessage): string {
  const wireMessage = toWireFormat(message);
  return JSON.stringify(wireMessage);
}

/**
 * Deserialize incoming JSON string to message
 */
export function deserializeMessage(data: string): IncomingMessage {
  const wireMessage = JSON.parse(data);
  return fromWireFormat(wireMessage);
}

/**
 * Convert outgoing message to wire format (snake_case)
 */
function toWireFormat(message: SDKOutgoingMessage): Record<string, unknown> {
  switch (message.type) {
    case 'config':
      return configToWire(message as SDKConfigMessage);
    case 'speak':
      return speakToWire(message as SpeakMessage);
    case 'ping':
      return { type: 'ping', timestamp: Date.now() };
    case 'audio':
      return { type: 'audio' }; // Binary audio handled separately
    case 'stop':
      return { type: 'stop' };
    case 'flush':
      return { type: 'flush' };
    case 'interrupt':
      return { type: 'interrupt' };
    default:
      return { type: (message as { type: string }).type };
  }
}

/**
 * Convert config message to wire format
 */
function configToWire(message: SDKConfigMessage): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    type: 'config',
  };

  if (message.streamId) {
    wire.stream_id = message.streamId;
  }

  if (message.audio !== undefined) {
    wire.audio = message.audio;
  }

  if (message.stt) {
    wire.stt_config = sttConfigToWire(message.stt);
  }

  if (message.tts) {
    wire.tts_config = ttsConfigToWire(message.tts);
  }

  if (message.livekit) {
    wire.livekit = livekitConfigToWire(message.livekit);
  }

  if (message.features) {
    wire.features = featuresToWire(message.features);
  }

  return wire;
}

/**
 * Convert STT config to wire format
 */
function sttConfigToWire(config: STTConfig): Record<string, unknown> {
  return {
    provider: config.provider,
    language: config.language,
    model: config.model,
    sample_rate: config.sampleRate,
    encoding: config.encoding,
    channels: config.channels,
    interim_results: config.interimResults,
    punctuate: config.punctuate,
    profanity_filter: config.profanityFilter,
    smart_format: config.smartFormat,
    diarize: config.diarize,
    keywords: config.keywords,
    custom_vocabulary: config.customVocabulary,
    endpointing: config.endpointing,
    utterance_end_ms: config.utteranceEndMs,
  };
}

/**
 * Convert TTS config to wire format
 */
function ttsConfigToWire(config: TTSConfig): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    provider: config.provider,
    voice: config.voice,
    voice_id: config.voiceId,
    model: config.model,
    sample_rate: config.sampleRate,
    audio_format: config.audioFormat,
    speed: config.speed,
    pitch: config.pitch,
    volume: config.volume,
    stability: config.stability,
    similarity_boost: config.similarityBoost,
    style: config.style,
    use_speaker_boost: config.useSpeakerBoost,
  };

  // Emotion settings (Unified Emotion System)
  if (config.emotion !== undefined) {
    wire.emotion = config.emotion;
  }
  if (config.emotionIntensity !== undefined) {
    wire.emotion_intensity = config.emotionIntensity;
  }
  if (config.deliveryStyle !== undefined) {
    wire.delivery_style = config.deliveryStyle;
  }
  if (config.emotionDescription !== undefined) {
    wire.emotion_description = config.emotionDescription;
  }

  // Hume-specific settings
  if (config.actingInstructions !== undefined) {
    wire.acting_instructions = config.actingInstructions;
  }
  if (config.voiceDescription !== undefined) {
    wire.voice_description = config.voiceDescription;
  }
  if (config.trailingSilence !== undefined) {
    wire.trailing_silence = config.trailingSilence;
  }
  if (config.instantMode !== undefined) {
    wire.instant_mode = config.instantMode;
  }

  return wire;
}

/**
 * Convert LiveKit config to wire format
 */
function livekitConfigToWire(config: LiveKitConfig): Record<string, unknown> {
  return {
    room_name: config.roomName,
    identity: config.identity,
    name: config.name,
    metadata: config.metadata,
  };
}

/**
 * Convert features to wire format
 */
function featuresToWire(features: FeatureFlags): Record<string, unknown> {
  return {
    vad: features.vad,
    noise_cancellation: features.noiseCancellation,
    speaker_diarization: features.speakerDiarization,
    interim_results: features.interimResults,
    punctuation: features.punctuation,
    profanity_filter: features.profanityFilter,
    smart_format: features.smartFormat,
    word_timestamps: features.wordTimestamps,
    echo_cancellation: features.echoCancellation,
    filler_words: features.fillerWords,
  };
}

/**
 * Convert speak message to wire format
 */
function speakToWire(message: SpeakMessage): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    type: 'speak',
    text: message.text,
  };

  if (message.voice) wire.voice = message.voice;
  if (message.voiceId) wire.voice_id = message.voiceId;
  if (message.provider) wire.provider = message.provider;
  if (message.model) wire.model = message.model;
  if (message.speed !== undefined) wire.speed = message.speed;
  if (message.pitch !== undefined) wire.pitch = message.pitch;
  if (message.flush !== undefined) wire.flush = message.flush;

  // Emotion settings for speak command
  if (message.emotion !== undefined) wire.emotion = message.emotion;
  if (message.emotionIntensity !== undefined) wire.emotion_intensity = message.emotionIntensity;
  if (message.deliveryStyle !== undefined) wire.delivery_style = message.deliveryStyle;
  if (message.emotionDescription !== undefined) wire.emotion_description = message.emotionDescription;

  return wire;
}

/**
 * Convert wire format to incoming message
 */
function fromWireFormat(wire: Record<string, unknown>): IncomingMessage {
  const type = wire.type as MessageType;

  switch (type) {
    case 'ready':
      return readyFromWire(wire);
    case 'stt_result':
      return sttResultFromWire(wire);
    case 'tts_audio':
      return ttsAudioFromWire(wire);
    case 'error':
      return errorFromWire(wire);
    case 'pong':
      return pongFromWire(wire);
    case 'session_update':
      return sessionUpdateFromWire(wire);
    case 'speaking_started':
      return { type: 'speaking_started' };
    case 'speaking_finished':
      return { type: 'speaking_finished' };
    case 'listening_started':
      return { type: 'listening_started' };
    case 'listening_stopped':
      return { type: 'listening_stopped' };
    default:
      // Return generic message for unknown types
      return { type, ...wire } as IncomingMessage;
  }
}

/**
 * Convert ready message from wire format
 */
function readyFromWire(wire: Record<string, unknown>): ReadyMessage {
  return {
    type: 'ready',
    sessionId: asString(wire.session_id),
    sttReady: asBoolean(wire.stt_ready),
    ttsReady: asBoolean(wire.tts_ready),
    livekitConnected: asBoolean(wire.livekit_connected),
    serverVersion: asString(wire.server_version),
    capabilities: asStringArray(wire.capabilities),
  };
}

/**
 * Convert STT result message from wire format
 */
function sttResultFromWire(wire: Record<string, unknown>): STTResultMessage {
  // Safely parse words array with validation
  let words: STTResultMessage['words'];
  if (Array.isArray(wire.words)) {
    words = wire.words
      .filter((w): w is Record<string, unknown> => w !== null && typeof w === 'object')
      .map((w) => ({
        word: asStringRequired(w.word, 'word.word'),
        start: asNumberRequired(w.start, 'word.start'),
        end: asNumberRequired(w.end, 'word.end'),
        confidence: asNumber(w.confidence),
        speakerId: asNumber(w.speaker_id),
      }));
  }

  return {
    type: 'stt_result',
    text: asStringRequired(wire.text, 'text'),
    isFinal: asBooleanRequired(wire.is_final, 'is_final'),
    confidence: asNumber(wire.confidence),
    speakerId: asNumber(wire.speaker_id),
    language: asString(wire.language),
    startTime: asNumber(wire.start_time),
    endTime: asNumber(wire.end_time),
    words,
    channelIndex: asNumber(wire.channel_index),
  };
}

/**
 * Convert TTS audio message from wire format
 */
function ttsAudioFromWire(wire: Record<string, unknown>): TTSAudioMessage {
  return {
    type: 'tts_audio',
    audio: asStringRequired(wire.audio, 'audio'), // Base64 encoded
    format: asString(wire.format),
    sampleRate: asNumber(wire.sample_rate),
    duration: asNumber(wire.duration),
    isFinal: asBoolean(wire.is_final),
    sequence: asNumber(wire.sequence),
  };
}

/**
 * Convert error message from wire format
 */
function errorFromWire(wire: Record<string, unknown>): ErrorMessage {
  return {
    type: 'error',
    code: asStringRequired(wire.code, 'code'),
    message: asStringRequired(wire.message, 'message'),
    details: asRecord(wire.details),
    recoverable: asBoolean(wire.recoverable),
  };
}

/**
 * Convert pong message from wire format
 */
function pongFromWire(wire: Record<string, unknown>): PongMessage {
  return {
    type: 'pong',
    timestamp: asNumberRequired(wire.timestamp, 'timestamp'),
    serverTime: asNumber(wire.server_time),
  };
}

/**
 * Convert session update message from wire format
 */
function sessionUpdateFromWire(wire: Record<string, unknown>): SessionUpdateMessage {
  return {
    type: 'session_update',
    field: asStringRequired(wire.field, 'field'),
    value: wire.value, // Any value type allowed
    previousValue: wire.previous_value, // Any value type allowed
  };
}

/**
 * Create a config message
 */
export function createConfigMessage(
  stt?: STTConfig,
  tts?: TTSConfig,
  livekit?: LiveKitConfig,
  features?: FeatureFlags
): SDKConfigMessage {
  return {
    type: 'config',
    stt,
    tts,
    livekit,
    features,
  };
}

/**
 * Create a speak message
 */
export function createSpeakMessage(text: string, options?: {
  voice?: string;
  voiceId?: string;
  provider?: string;
  model?: string;
  speed?: number;
  pitch?: number;
  flush?: boolean;
}): SpeakMessage {
  return {
    type: 'speak',
    text,
    ...options,
  };
}

/**
 * Create a ping message
 */
export function createPingMessage(): OutgoingMessage {
  return { type: 'ping' };
}

/**
 * Create an audio message marker
 */
export function createAudioMessage(): OutgoingMessage {
  return { type: 'audio' };
}

/**
 * Create a stop message
 */
export function createStopMessage(): OutgoingMessage {
  return { type: 'stop' };
}

/**
 * Create a flush message
 */
export function createFlushMessage(): OutgoingMessage {
  return { type: 'flush' };
}

/**
 * Create an interrupt message
 */
export function createInterruptMessage(): OutgoingMessage {
  return { type: 'interrupt' };
}
