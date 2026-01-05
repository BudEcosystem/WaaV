/**
 * WebSocket Message Serialization/Deserialization
 * Handles conversion between TypeScript types and wire format
 */

import type {
  IncomingMessage,
  OutgoingMessage,
  ConfigMessage,
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

/**
 * Serialize outgoing message to JSON string
 */
export function serializeMessage(message: OutgoingMessage): string {
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
function toWireFormat(message: OutgoingMessage): Record<string, unknown> {
  switch (message.type) {
    case 'config':
      return configToWire(message as ConfigMessage);
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
function configToWire(message: ConfigMessage): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    type: 'config',
  };

  if (message.stt) {
    wire.stt = sttConfigToWire(message.stt);
  }

  if (message.tts) {
    wire.tts = ttsConfigToWire(message.tts);
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
  return {
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
    sessionId: wire.session_id as string | undefined,
    sttReady: wire.stt_ready as boolean | undefined,
    ttsReady: wire.tts_ready as boolean | undefined,
    livekitConnected: wire.livekit_connected as boolean | undefined,
    serverVersion: wire.server_version as string | undefined,
    capabilities: wire.capabilities as string[] | undefined,
  };
}

/**
 * Convert STT result message from wire format
 */
function sttResultFromWire(wire: Record<string, unknown>): STTResultMessage {
  const words = wire.words as Array<Record<string, unknown>> | undefined;

  return {
    type: 'stt_result',
    text: wire.text as string,
    isFinal: wire.is_final as boolean,
    confidence: wire.confidence as number | undefined,
    speakerId: wire.speaker_id as number | undefined,
    language: wire.language as string | undefined,
    startTime: wire.start_time as number | undefined,
    endTime: wire.end_time as number | undefined,
    words: words?.map((w) => ({
      word: w.word as string,
      start: w.start as number,
      end: w.end as number,
      confidence: w.confidence as number | undefined,
      speakerId: w.speaker_id as number | undefined,
    })),
    channelIndex: wire.channel_index as number | undefined,
  };
}

/**
 * Convert TTS audio message from wire format
 */
function ttsAudioFromWire(wire: Record<string, unknown>): TTSAudioMessage {
  return {
    type: 'tts_audio',
    audio: wire.audio as string, // Base64 encoded
    format: wire.format as string | undefined,
    sampleRate: wire.sample_rate as number | undefined,
    duration: wire.duration as number | undefined,
    isFinal: wire.is_final as boolean | undefined,
    sequence: wire.sequence as number | undefined,
  };
}

/**
 * Convert error message from wire format
 */
function errorFromWire(wire: Record<string, unknown>): ErrorMessage {
  return {
    type: 'error',
    code: wire.code as string,
    message: wire.message as string,
    details: wire.details as Record<string, unknown> | undefined,
    recoverable: wire.recoverable as boolean | undefined,
  };
}

/**
 * Convert pong message from wire format
 */
function pongFromWire(wire: Record<string, unknown>): PongMessage {
  return {
    type: 'pong',
    timestamp: wire.timestamp as number,
    serverTime: wire.server_time as number | undefined,
  };
}

/**
 * Convert session update message from wire format
 */
function sessionUpdateFromWire(wire: Record<string, unknown>): SessionUpdateMessage {
  return {
    type: 'session_update',
    field: wire.field as string,
    value: wire.value,
    previousValue: wire.previous_value,
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
): ConfigMessage {
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
