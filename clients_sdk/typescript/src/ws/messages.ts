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
  ResolvedAlias,
  STTResultMessage,
  TTSAudioMessage,
  ErrorMessage,
  PongMessage,
  SessionUpdateMessage,
  ConfigWarningMessage,
  MessageType,
} from '../types/messages.js';
import type { STTConfig, TTSConfig, LiveKitConfig, DAGConfig, ConversationConfig, TurnDetectionConfig } from '../types/config.js';
import { conversationConfigToWire } from '../types/conversation.js';
import { serializeDAGConfig } from '../types/dag.js';
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
 *
 * 1:1 with the gateway `/ws` config envelope: stt/tts/livekit/dag/conversation,
 * plus turn detection (nested into `stt_config.turn_detection` on the wire).
 */
export interface SDKConfigMessage {
  type: 'config';
  streamId?: string;
  audio?: boolean;
  stt?: STTConfig;
  tts?: TTSConfig;
  livekit?: LiveKitConfig;
  /** DAG routing configuration. */
  dag?: DAGConfig;
  /** Built-in conversation/agent loop (serialized into `conversation_config`). */
  conversation?: ConversationConfig;
  /** ML turn detection (nested into `stt_config.turn_detection`). */
  turnDetection?: TurnDetectionConfig;
  /**
   * Proxy/alias name (P3): a single server-defined name the gateway resolves to a
   * full {stt,tts,llm,dag} bundle BEFORE provider construction. Composes with the
   * rest — an explicit `stt`/`tts`/`conversation` here overrides the alias default.
   * The resolved concrete providers come back on `ready` as `resolved_alias`.
   */
  alias?: string;
  /**
   * Legacy client feature flags. These are folded into the nested
   * `stt_config.features{}` block (NOT emitted as a top-level `features` key,
   * which the gateway does not recognize and silently drops).
   */
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
 * Convert config message to wire format.
 *
 * Mirrors the gateway `/ws` config envelope. The dead top-level `features` key
 * (silently dropped by the gateway) is GONE: advanced STT/TTS features nest
 * under `stt_config.features{}` / `tts_config.features{}`, turn detection nests
 * under `stt_config.turn_detection`, and the conversation/DAG loops get their
 * own blocks.
 */
function configToWire(message: SDKConfigMessage): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    type: 'config',
  };

  if (message.streamId) {
    wire.stream_id = message.streamId;
  }

  // Proxy/alias name (P3): a single top-level field the gateway resolves to a full
  // {stt,tts,llm,dag} bundle server-side (an explicit config below overrides it).
  if (message.alias) {
    wire.alias = message.alias;
  }

  if (message.audio !== undefined) {
    wire.audio = message.audio;
  }

  if (message.stt) {
    // Turn detection + legacy feature flags fold INTO the STT block (their real
    // wire home), never a top-level key.
    wire.stt_config = sttConfigToWire(message.stt, message.turnDetection, message.features);
  }

  if (message.tts) {
    wire.tts_config = ttsConfigToWire(message.tts);
  }

  if (message.livekit) {
    wire.livekit = livekitConfigToWire(message.livekit);
  }

  if (message.dag) {
    wire.dag_config = serializeDAGConfig(message.dag);
  }

  if (message.conversation) {
    wire.conversation_config = conversationConfigToWire(message.conversation);
  }

  return wire;
}

/** Assign `value` to `target[key]` only when it is not undefined. */
function setIfDefined(target: Record<string, unknown>, key: string, value: unknown): void {
  if (value !== undefined) target[key] = value;
}

/**
 * Build the nested `stt_config.features{}` block from the canonical SttFeatures
 * fields on STTConfig (+ legacy FeatureFlags), omitting anything unset. Returns
 * undefined when no feature is set so the key is omitted entirely.
 */
function sttFeaturesToWire(config: STTConfig, features?: FeatureFlags): Record<string, unknown> | undefined {
  const f: Record<string, unknown> = {};

  // Canonical SttFeatures (gateway/src/core/stt/standard.rs).
  setIfDefined(f, 'interim_results', config.interimResults);
  // STTConfig.diarize → canonical `diarization`.
  setIfDefined(f, 'diarization', config.diarize);
  setIfDefined(f, 'word_timestamps', config.wordTimestamps);
  setIfDefined(f, 'smart_format', config.smartFormat);
  setIfDefined(f, 'profanity_filter', config.profanityFilter);
  setIfDefined(f, 'filler_words', config.fillerWords);
  setIfDefined(f, 'vad_events', config.vadEvents);
  setIfDefined(f, 'endpointing_ms', config.endpointingMs ?? config.endpointing);
  setIfDefined(f, 'utterance_end_ms', config.utteranceEndMsFeature ?? config.utteranceEndMs);
  // STTConfig.keywords / .keyterms → canonical `keyterms`.
  setIfDefined(f, 'keyterms', config.keyterms ?? config.keywords);
  setIfDefined(f, 'redaction', config.redaction);
  setIfDefined(f, 'language_detection', config.languageDetection);
  setIfDefined(f, 'entity_detection', config.entityDetection);
  setIfDefined(f, 'numerals', config.numerals);
  setIfDefined(f, 'multichannel', config.multichannel);
  setIfDefined(f, 'alternatives', config.alternatives);
  setIfDefined(f, 'sentiment', config.sentiment);
  setIfDefined(f, 'speech_begin_event', config.speechBeginEvent);

  // Legacy top-level FeatureFlags map onto canonical SttFeatures, without
  // overriding an explicit per-config value above.
  if (features) {
    if (f.interim_results === undefined && features.interimResults !== undefined) f.interim_results = features.interimResults;
    if (f.diarization === undefined && features.speakerDiarization !== undefined) f.diarization = features.speakerDiarization;
    if (f.smart_format === undefined && features.smartFormat !== undefined) f.smart_format = features.smartFormat;
    if (f.profanity_filter === undefined && features.profanityFilter !== undefined) f.profanity_filter = features.profanityFilter;
    if (f.word_timestamps === undefined && features.wordTimestamps !== undefined) f.word_timestamps = features.wordTimestamps;
    if (f.filler_words === undefined && features.fillerWords !== undefined) f.filler_words = features.fillerWords;
  }

  return Object.keys(f).length > 0 ? f : undefined;
}

/**
 * Convert STT config to wire format (`STTWebSocketConfig`). Advanced features
 * nest under `features{}`, ML turn detection under `turn_detection`, and the
 * no-canonical-field long tail (e.g. customVocabulary) under `extras`.
 */
function sttConfigToWire(
  config: STTConfig,
  turnDetection?: TurnDetectionConfig,
  features?: FeatureFlags
): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    provider: config.provider,
  };
  // Required-ish top-level scalars (omit when unset; the gateway has serde
  // defaults for encoding/model and language is required by the gateway).
  setIfDefined(wire, 'language', config.language);
  setIfDefined(wire, 'sample_rate', config.sampleRate);
  setIfDefined(wire, 'channels', config.channels);
  // STTConfig allows `punctuate` as a Deepgram-style alias of `punctuation`.
  setIfDefined(wire, 'punctuation', config.punctuation ?? config.punctuate);
  setIfDefined(wire, 'encoding', config.encoding);
  setIfDefined(wire, 'model', config.model);
  setIfDefined(wire, 'api_key', config.apiKey);

  // Nested canonical features.
  const featuresWire = sttFeaturesToWire(config, features);
  if (featuresWire) wire.features = featuresWire;

  // ML turn detection → stt_config.turn_detection (config.rs:345). Only the
  // three real TurnDetectionWsConfig fields go on the wire; the silence/padding
  // hints are client-side VAD only.
  if (turnDetection && turnDetection.enabled) {
    const td: Record<string, unknown> = { enabled: true };
    setIfDefined(td, 'threshold', turnDetection.threshold);
    setIfDefined(td, 'eager', turnDetection.eager);
    wire.turn_detection = td;
  }

  // Open passthrough: explicit extras + the no-canonical-field customVocabulary.
  const extras: Record<string, unknown> = { ...(config.extras ?? {}) };
  if (config.customVocabulary !== undefined && extras.custom_vocabulary === undefined) {
    extras.custom_vocabulary = config.customVocabulary;
  }
  if (Object.keys(extras).length > 0) wire.extras = extras;

  return wire;
}

/**
 * Build the nested `tts_config.features{}` block from the canonical TtsFeatures
 * fields on TTSConfig, omitting anything unset.
 */
function ttsFeaturesToWire(config: TTSConfig): Record<string, unknown> | undefined {
  const f: Record<string, unknown> = {};

  // Canonical TtsFeatures (gateway/src/core/tts/standard.rs). speed/pitch/volume
  // /stability/similarity_boost/style/use_speaker_boost are ALSO mirrored here so
  // providers reading TtsFeatures honor them too.
  setIfDefined(f, 'speed', config.speed);
  setIfDefined(f, 'pitch', config.pitch);
  setIfDefined(f, 'volume', config.volume);
  setIfDefined(f, 'stability', config.stability);
  setIfDefined(f, 'similarity_boost', config.similarityBoost);
  setIfDefined(f, 'style', config.style);
  setIfDefined(f, 'use_speaker_boost', config.useSpeakerBoost);
  setIfDefined(f, 'instructions', config.instructions);
  setIfDefined(f, 'ssml', config.ssml);
  setIfDefined(f, 'language', config.language);
  setIfDefined(f, 'word_timestamps', config.wordTimestamps);
  setIfDefined(f, 'streaming', config.streaming);
  setIfDefined(f, 'seed', config.seed);
  setIfDefined(f, 'optimize_streaming_latency', config.optimizeStreamingLatency);
  setIfDefined(f, 'include_timestamp_types', config.includeTimestampTypes);
  setIfDefined(f, 'rate_percentage', config.ratePercentage);
  setIfDefined(f, 'pitch_percentage', config.pitchPercentage);

  return Object.keys(f).length > 0 ? f : undefined;
}

/**
 * Convert TTS config to wire format (`TTSWebSocketConfig`). Advanced knobs nest
 * under `features{}`; Hume-only knobs with no canonical field go under `extras`.
 */
function ttsConfigToWire(config: TTSConfig): Record<string, unknown> {
  const wire: Record<string, unknown> = {
    provider: config.provider,
  };
  // `voice` is a convenience alias for `voice_id`.
  setIfDefined(wire, 'voice_id', config.voiceId ?? config.voice);
  // Abstract voice selection (P4): the gateway resolves a VoiceDescriptor to a
  // concrete provider voice_id server-side. Sent under `voice_descriptor` as a
  // snake_case object; a raw voice_id always wins, so the gateway only uses this
  // when voice_id is absent. Only set fields are emitted.
  if (config.voiceDescriptor !== undefined) {
    const vd = config.voiceDescriptor;
    const vdWire: Record<string, unknown> = {};
    setIfDefined(vdWire, 'gender', vd.gender);
    setIfDefined(vdWire, 'locale', vd.locale);
    setIfDefined(vdWire, 'accent', vd.accent);
    setIfDefined(vdWire, 'age', vd.age);
    setIfDefined(vdWire, 'style', vd.style);
    setIfDefined(vdWire, 'name_hint', vd.nameHint);
    if (Object.keys(vdWire).length > 0) wire.voice_descriptor = vdWire;
  }
  setIfDefined(wire, 'model', config.model);
  setIfDefined(wire, 'sample_rate', config.sampleRate);
  setIfDefined(wire, 'audio_format', config.audioFormat);
  // Top-level egress rate is `speaking_rate` on TTSWebSocketConfig; accept the
  // `speakingRate` field or fall back to the `speed` alias.
  setIfDefined(wire, 'speaking_rate', config.speakingRate ?? config.speed);
  setIfDefined(wire, 'audio_out_chunk_ms', config.audioOutChunkMs);
  setIfDefined(wire, 'client_playback_rate', config.clientPlaybackRate);
  setIfDefined(wire, 'connection_timeout', config.connectionTimeout);
  setIfDefined(wire, 'request_timeout', config.requestTimeout);
  setIfDefined(wire, 'api_key', config.apiKey);

  if (config.pronunciations !== undefined) {
    wire.pronunciations = config.pronunciations.map((p) => ({ from: p.from, to: p.to }));
  }

  // Emotion settings (Unified Emotion System).
  setIfDefined(wire, 'emotion', config.emotion);
  setIfDefined(wire, 'emotion_intensity', config.emotionIntensity);
  setIfDefined(wire, 'delivery_style', config.deliveryStyle);
  setIfDefined(wire, 'emotion_description', config.emotionDescription);

  // Nested canonical features.
  const featuresWire = ttsFeaturesToWire(config);
  if (featuresWire) wire.features = featuresWire;

  // Hume-only knobs with no canonical TtsFeatures field → extras passthrough.
  const extras: Record<string, unknown> = { ...(config.extras ?? {}) };
  if (config.actingInstructions !== undefined && extras.acting_instructions === undefined) extras.acting_instructions = config.actingInstructions;
  if (config.voiceDescription !== undefined && extras.voice_description === undefined) extras.voice_description = config.voiceDescription;
  if (config.trailingSilence !== undefined && extras.trailing_silence === undefined) extras.trailing_silence = config.trailingSilence;
  if (config.instantMode !== undefined && extras.instant_mode === undefined) extras.instant_mode = config.instantMode;
  if (Object.keys(extras).length > 0) wire.extras = extras;

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
    case 'config_warning':
      return configWarningFromWire(wire);
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
  // P3: the resolved-alias echo — the concrete providers the gateway resolved an
  // `alias` to (no secrets), so a developer can SEE what e.g. "support-bot" became.
  if (wire.resolved_alias && typeof wire.resolved_alias === 'object') {
    msg.resolved_alias = wire.resolved_alias as ResolvedAlias;
  }
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
 * Convert config_warning message from wire format. Gateway wire shape
 * (handlers/ws/messages.rs OutgoingMessage::ConfigWarning):
 * { type, code, message, detail? }. `detail` is arbitrary JSON.
 */
function configWarningFromWire(wire: Record<string, unknown>): ConfigWarningMessage {
  const msg: ConfigWarningMessage = {
    type: 'config_warning',
    code: asStringRequired(wire.code, 'code'),
    message: asStringRequired(wire.message, 'message'),
  };
  const detail = asRecord(wire.detail);
  if (detail !== undefined) msg.detail = detail;
  return msg;
}

/**
 * Create a config message.
 *
 * `stt`/`tts`/`livekit`/`features` are positional for backward compatibility;
 * the 1:1-mirror additions (dag / conversation / turnDetection / streamId /
 * audio) are passed via the trailing `extra` options bag.
 */
export function createConfigMessage(
  stt?: STTConfig,
  tts?: TTSConfig,
  livekit?: LiveKitConfig,
  features?: FeatureFlags,
  extra?: {
    dag?: DAGConfig;
    conversation?: ConversationConfig;
    turnDetection?: TurnDetectionConfig;
    alias?: string;
    streamId?: string;
    audio?: boolean;
  }
): SDKConfigMessage {
  const msg: SDKConfigMessage = {
    type: 'config',
    stt,
    tts,
    livekit,
    features,
  };
  if (extra?.dag !== undefined) msg.dag = extra.dag;
  if (extra?.conversation !== undefined) msg.conversation = extra.conversation;
  if (extra?.turnDetection !== undefined) msg.turnDetection = extra.turnDetection;
  if (extra?.alias !== undefined) msg.alias = extra.alias;
  if (extra?.streamId !== undefined) msg.streamId = extra.streamId;
  if (extra?.audio !== undefined) msg.audio = extra.audio;
  return msg;
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
