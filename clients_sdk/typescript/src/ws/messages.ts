/**
 * WebSocket Message Serialization/Deserialization
 * Handles conversion between TypeScript types and wire format
 */

import type {
  IncomingMessage,
  OutgoingMessage,
  SpeakMessage,
  SendMessageMessage,
  ClearMessage,
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
    case 'clear':
      // Barge-in / cancel current TTS playback (gateway op `clear`).
      return { type: 'clear' };
    case 'audio_end':
      // Finalize: tell the gateway the audio stream has ended so it flushes
      // any pending transcript (gateway op `audio_end`).
      return { type: 'audio_end' };
    case 'send_message':
      return sendMessageToWire(message as SendMessageMessage);
    case 'sip_transfer':
      return {
        type: 'sip_transfer',
        transfer_to: (message as { transfer_to: string }).transfer_to,
      };
    default:
      return { type: (message as { type: string }).type };
  }
}

/**
 * Convert a send_message (data channel) message to wire format
 */
function sendMessageToWire(message: SendMessageMessage): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    type: 'send_message',
    message: message.message,
    role: message.role,
  };
  if (message.topic !== undefined) wire.topic = message.topic;
  if (message.debug !== undefined) wire.debug = message.debug;
  return wire;
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
  const wire: Record<string, unknown> = {
    room_name: config.roomName,
  };
  if (config.enableRecording !== undefined) wire.enable_recording = config.enableRecording;
  if (config.waavParticipantIdentity !== undefined)
    wire.waav_participant_identity = config.waavParticipantIdentity;
  if (config.waavParticipantName !== undefined)
    wire.waav_participant_name = config.waavParticipantName;
  if (config.listenParticipants !== undefined)
    wire.listen_participants = config.listenParticipants;
  return wire;
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
    default:
      // Return generic message for unknown types (e.g. tts_playback_complete,
      // message, participant_disconnected, sip_transfer_error). These already
      // arrive in the gateway's snake_case wire shape, so pass them through.
      return { ...wire, type } as IncomingMessage;
  }
}

/**
 * Convert ready message from wire format
 */
function readyFromWire(wire: Record<string, unknown>): ReadyMessage {
  // Gateway wire shape (handlers/ws/messages.rs OutgoingMessage::Ready):
  // { type, protocol_version, stream_id, livekit_room_name?, livekit_url?,
  //   waav_participant_identity?, waav_participant_name? }
  const msg: ReadyMessage = {
    type: 'ready',
    stream_id: asStringRequired(wire.stream_id, 'stream_id'),
    protocol_version: asString(wire.protocol_version),
  };
  const livekitRoomName = asString(wire.livekit_room_name);
  if (livekitRoomName !== undefined) msg.livekit_room_name = livekitRoomName;
  const livekitUrl = asString(wire.livekit_url);
  if (livekitUrl !== undefined) msg.livekit_url = livekitUrl;
  const pId = asString(wire.waav_participant_identity);
  if (pId !== undefined) msg.waav_participant_identity = pId;
  const pName = asString(wire.waav_participant_name);
  if (pName !== undefined) msg.waav_participant_name = pName;
  return msg;
}

/**
 * Convert STT result message from wire format
 */
function sttResultFromWire(wire: Record<string, unknown>): STTResultMessage {
  // Gateway wire shape (handlers/ws/messages.rs OutgoingMessage::STTResult):
  // { type, transcript, is_final, is_speech_final, confidence, segment_transcript? }
  // NOTE: the field is `transcript`, NOT `text` — reading `text` was the bug that
  // made every transcript come back empty.
  const msg: STTResultMessage = {
    type: 'stt_result',
    transcript: asStringRequired(wire.transcript, 'transcript'),
    is_final: asBooleanRequired(wire.is_final, 'is_final'),
    is_speech_final: asBooleanRequired(wire.is_speech_final, 'is_speech_final'),
    confidence: asNumberRequired(wire.confidence, 'confidence'),
  };
  const segment = asString(wire.segment_transcript);
  if (segment !== undefined) msg.segment_transcript = segment;
  return msg;
}

/**
 * Convert TTS audio message from wire format
 */
function ttsAudioFromWire(wire: Record<string, unknown>): TTSAudioMessage {
  const msg: TTSAudioMessage = {
    type: 'tts_audio',
    audio: asStringRequired(wire.audio, 'audio'), // Base64 encoded
  };
  const format = asString(wire.format);
  if (format !== undefined) msg.format = format;
  const sampleRate = asNumber(wire.sample_rate);
  if (sampleRate !== undefined) msg.sample_rate = sampleRate;
  const duration = asNumber(wire.duration);
  if (duration !== undefined) msg.duration = duration;
  const isFinal = asBoolean(wire.is_final);
  if (isFinal !== undefined) msg.is_final = isFinal;
  const sequence = asNumber(wire.sequence);
  if (sequence !== undefined) msg.sequence = sequence;
  return msg;
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
  const msg: PongMessage = {
    type: 'pong',
    timestamp: asNumberRequired(wire.timestamp, 'timestamp'),
  };
  const serverTime = asNumber(wire.server_time);
  if (serverTime !== undefined) msg.server_time = serverTime;
  return msg;
}

/**
 * Convert session update message from wire format
 */
function sessionUpdateFromWire(wire: Record<string, unknown>): SessionUpdateMessage {
  return {
    type: 'session_update',
    field: asStringRequired(wire.field, 'field'),
    value: wire.value, // Any value type allowed
    previous_value: wire.previous_value, // Any value type allowed
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
 * Create a clear message (barge-in / cancel current TTS playback).
 * This is the gateway's `clear` op — the correct way to interrupt the bot.
 */
export function createClearMessage(): ClearMessage {
  return { type: 'clear' };
}

/**
 * Create an audio_end message (finalize).
 * Tells the gateway the audio stream has ended so it flushes any pending
 * transcript. This is the gateway's `audio_end` op.
 */
export function createAudioEndMessage(): OutgoingMessage {
  return { type: 'audio_end' };
}

/**
 * Create a send_message (data-channel) message.
 */
export function createSendMessageMessage(
  message: string,
  role: string,
  options?: { topic?: string; debug?: Record<string, unknown> }
): SendMessageMessage {
  return {
    type: 'send_message',
    message,
    role,
    ...options,
  };
}
