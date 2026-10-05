/**
 * WebSocket Message Types
 *
 * Message types are named from the CLIENT SDK perspective:
 * - OutgoingMessage: Messages the CLIENT sends TO the server
 * - IncomingMessage: Messages the CLIENT receives FROM the server
 *
 * Note: This is the opposite of the gateway (server) naming convention in messages.rs
 * where IncomingMessage = what server receives and OutgoingMessage = what server sends.
 */

import type { STTConfig, TTSConfig, LiveKitConfig, Emotion, DeliveryStyle, EmotionIntensityLevel } from './config.js';

/**
 * The WebSocket wire-protocol version this SDK is built against. The gateway
 * sends its own `protocol_version` on the `ready` message; the SDK asserts the
 * two match so a breaking contract change surfaces as an explicit mismatch
 * warning instead of silent field drift. Mirrors gateway PROTOCOL_VERSION.
 */
export const PROTOCOL_VERSION = '1.0';

// ============================================================================
// Outgoing Messages (Client -> Server)
// These are messages the CLIENT SENDS to the server
// ============================================================================

/**
 * Configuration message to initialize the WebSocket session
 */
export interface ConfigMessage {
  type: 'config';
  /** Optional unique identifier for this WebSocket session */
  stream_id?: string;
  /** Enable audio processing (STT/TTS). Defaults to true */
  audio?: boolean;
  /** STT configuration */
  stt_config?: {
    provider: string;
    language: string;
    sample_rate: number;
    channels: number;
    punctuation: boolean;
    encoding: string;
    model: string;
  };
  /** TTS configuration */
  tts_config?: {
    provider: string;
    voice_id?: string;
    speaking_rate?: number;
    audio_format?: string;
    sample_rate?: number;
    connection_timeout?: number;
    request_timeout?: number;
    model: string;
    pronunciations?: Array<{ from: string; to: string }>;
  };
  /** LiveKit configuration */
  livekit?: {
    room_name: string;
    enable_recording?: boolean;
    waav_participant_identity?: string;
    waav_participant_name?: string;
    listen_participants?: string[];
  };
}

/**
 * Speak message to synthesize text to speech
 */
export interface SpeakMessage {
  type: 'speak';
  /** Text to synthesize */
  text: string;
  /** Flush TTS buffer immediately */
  flush?: boolean;
  /** Allow this TTS to be interrupted */
  allow_interruption?: boolean;
  /** Voice name */
  voice?: string;
  /** Voice ID */
  voiceId?: string;
  /** TTS provider to use */
  provider?: string;
  /** TTS model to use */
  model?: string;
  /** Speed/rate adjustment */
  speed?: number;
  /** Pitch adjustment */
  pitch?: number;
  /** Primary emotion to express */
  emotion?: Emotion;
  /** Emotion intensity (0.0 to 1.0 or preset level) */
  emotionIntensity?: number | EmotionIntensityLevel;
  /** Delivery style */
  deliveryStyle?: DeliveryStyle;
  /** Free-form emotion description */
  emotionDescription?: string;
}

/**
 * Clear message to stop current TTS playback (barge-in / cancel).
 */
export interface ClearMessage {
  type: 'clear';
}

/**
 * Audio-end message: signals the gateway that the inbound audio stream has
 * ended so it finalizes any pending transcript.
 */
export interface AudioEndMessage {
  type: 'audio_end';
}

/**
 * Send a data message to other participants
 */
export interface SendMessageMessage {
  type: 'send_message';
  /** Message content */
  message: string;
  /** Message role (e.g., "user", "assistant") */
  role: string;
  /** Optional topic/channel */
  topic?: string;
  /** Optional debug metadata */
  debug?: Record<string, unknown>;
}

/**
 * SIP transfer message
 */
export interface SIPTransferMessage {
  type: 'sip_transfer';
  /** The destination phone number to transfer the call to */
  transfer_to: string;
}

/**
 * Union type for all outgoing messages (sent by client to server)
 */
export type OutgoingMessage =
  | ConfigMessage
  | SpeakMessage
  | ClearMessage
  | AudioEndMessage
  | SendMessageMessage
  | SIPTransferMessage;

// ============================================================================
// Incoming Messages (Server -> Client)
// These are messages the CLIENT RECEIVES from the server
// ============================================================================

/**
 * Ready message confirming session initialization
 */
export interface ReadyMessage {
  type: 'ready';
  /**
   * WebSocket wire-protocol version (gateway PROTOCOL_VERSION, e.g. "1.0").
   * SDKs assert this on connect so a breaking change to the message contract
   * surfaces as an explicit version mismatch instead of silent field drift.
   */
  protocol_version?: string;
  /** Unique identifier for this WebSocket session */
  stream_id: string;
  /** LiveKit room name that was created */
  livekit_room_name?: string;
  /** LiveKit URL to connect to */
  livekit_url?: string;
  /** Identity of the AI agent participant in the room */
  waav_participant_identity?: string;
  /** Display name of the AI agent participant */
  waav_participant_name?: string;
  /**
   * P3 proxy/alias echo: the concrete providers the gateway resolved an `alias`
   * to (no secrets). Present only when an `alias` was sent and recognized. Lets a
   * developer SEE what e.g. "support-bot" became — and re-point it server-side
   * with zero client change.
   */
  resolved_alias?: ResolvedAlias;
  /** D8 negotiated uplink transport codec in effect (`linear16` | `opus`); present only on request. */
  audio_in_codec?: string;
  /** D8 negotiated downlink transport codec in effect (`linear16` | `opus`); present only on request. */
  audio_out_codec?: string;
  /**
   * What speech-to-text this session got (segmented STT): streaming, segmented or buffered, the
   * kind of interim results, who ends utterances, the expected latency and its basis, and notices.
   * Present only on sessions the gateway's rollout switch covers; absent = no statement.
   */
  stt?: ReadyStt;
}

/** A string union that still accepts values a newer gateway may add. */
type OpenString<T extends string> = T | (string & {});

/** A fact about the session reported on `ready.stt.notices` instead of a message. */
export interface SttNotice {
  /** Stable machine code, e.g. `stt_language_unset`, `stt_capability_assumed`. */
  code: string;
  /** Human-readable explanation. */
  message: string;
  /** Optional structured detail. */
  detail?: Record<string, unknown>;
}

/**
 * The `ready.stt` object (gateway docs/segmented-stt/customer-contract-reference.md section 2).
 *
 * Wire keys stay snake_case, like {@link ResolvedAlias}. Every known key is optional and the object
 * is open: keys a newer gateway adds are kept as-is.
 */
export interface ReadyStt {
  /** Canonical provider id after alias or deployment resolution. */
  provider?: string;
  /** The model that runs (absent when the gateway does not know it). */
  model?: string;
  /** Where `model` came from. */
  model_source?: OpenString<'request' | 'deployment' | 'provider_default' | 'substituted'>;
  /** The Bud deployment name the client used (named-deployment sessions only). */
  deployment?: string;
  /**
   * `streaming`: text while the caller speaks. `segmented`: text after each pause. `buffered`: text
   * only at `audio_end` or hang-up.
   */
  transcription_mode?: OpenString<'streaming' | 'segmented' | 'buffered'>;
  /** The preference that applied; differs from `transcription_mode` when unmet. */
  requested_mode?: OpenString<'auto' | 'streaming' | 'segmented'>;
  /** The level the preference came from. */
  requested_mode_source?: OpenString<'request' | 'deployment' | 'default'>;
  /** `live`: revisable interims during speech. `per_segment`: the whole turn so far, after a pause. */
  interim_results?: OpenString<'live' | 'per_segment' | 'none'>;
  /** Who ends utterances. */
  endpointing?: OpenString<'vendor' | 'gateway' | 'client'>;
  /** Which `vad_event` messages arrive. */
  speech_events?: OpenString<'detector' | 'transcript' | 'none'>;
  /** Sustained speech needed for `turn_start` while audio plays. */
  barge_in_ms?: number;
  /** The voice detector in use (gateway-endpointed sessions). */
  detector?: OpenString<'silero' | 'energy' | 'scripted'>;
  /** Where `confidence` comes from; `none` means it is exactly 1.0, so do not filter on it. */
  confidence_source?: OpenString<'vendor' | 'derived' | 'none' | 'unknown'>;
  /** Latency bucket of the slow figure. */
  latency_class?: OpenString<'realtime' | 'fast' | 'slow' | 'unknown'>;
  /** 50th-percentile end-of-speech to text, in ms. */
  final_latency_typical_ms?: number | null;
  /** Slow-percentile end-of-speech to text, in ms. */
  final_latency_slow_ms?: number | null;
  /** Which percentile the slow figure is (95 or 99). */
  final_latency_slow_percentile?: number;
  /** How the latency figures were obtained. */
  latency_basis?: OpenString<'measured' | 'provisional' | 'seed' | 'none'>;
  /** How long the gateway waits for a segment's text before reporting it lost, in ms. */
  final_deadline_ms?: number | null;
  /** Vendor lifecycle of the model. */
  lifecycle?: OpenString<'ga' | 'preview' | 'deprecated'>;
  /** Shutdown date (`YYYY-MM-DD`) of a deprecated model. */
  shutdown_on?: string;
  /** The capability-map layer that matched. */
  capability_source?: OpenString<
    'deployment_override' | 'exact' | 'glob' | 'model_unset' | 'provider_default' | 'global_default'
  >;
  /** Same-provider models that stream. */
  streaming_alternatives?: string[];
  /** Facts reported without a message. */
  notices?: SttNotice[];
  /** Capability map version. */
  map_version?: string;
  /** Keys a newer gateway adds. */
  [key: string]: unknown;
}

/**
 * The post-merge concrete bundle a server-defined `alias` resolved to (P3),
 * echoed on `ready`. No secrets (system prompts / API keys are omitted).
 */
export interface ResolvedAlias {
  /** The alias name that was resolved. */
  name?: string;
  /** Resolution kind: stt | tts | realtime | agent | dag. */
  kind?: string;
  /** Resolved STT provider/model/language. */
  stt?: Record<string, unknown>;
  /** Resolved TTS provider/voice/emotion. */
  tts?: Record<string, unknown>;
  /** Resolved LLM/conversation settings. */
  llm?: Record<string, unknown>;
  /** Resolved DAG template name, when the alias routes to a DAG. */
  dag_template?: string;
}

/**
 * One translated segment in the uniform gateway `translations` array (P5).
 *
 * The gateway folds Speechmatics `AddTranslation` / Gladia `type:"translation"` /
 * the OpenAI-Groq English fast path into this single `{lang, text}` shape so the
 * SDK reads ONE field regardless of provider. Mirrors gateway `Translation`
 * (gateway/src/core/stt/standard.rs).
 */
export interface Translation {
  /** Canonical target-language BCP-47 string (e.g. `es-ES`). */
  lang: string;
  /** The translated text for this segment. */
  text: string;
  /** `true` if this is a partial (interim) translation, `false`/omitted if final. */
  is_partial?: boolean;
}

/**
 * STT result message containing transcription
 */
export interface STTResultMessage {
  type: 'stt_result';
  /** Transcribed text (gateway field `transcript`, NOT `text`) */
  transcript: string;
  /** Whether this is the final version of the transcript */
  is_final: boolean;
  /** Whether speech has ended */
  is_speech_final: boolean;
  /** Confidence score (0.0 to 1.0) */
  confidence: number;
  /**
   * The FULL accumulated segment text, present only on a speech_final whose
   * segment spans multiple finals (or a forced/timer fire). Clients that DISPLAY
   * per-final text should prefer this when present.
   */
  segment_transcript?: string;
  /**
   * Uniform, provider-agnostic in-stream translations merged onto this transcript
   * (P5). Empty/absent unless a translation-capable provider returned a
   * `translations:[{lang,text}]` array on this stt_result frame.
   */
  translations?: Translation[];
}

/**
 * Unified message from various sources
 */
export interface UnifiedMessage {
  /** Text message content */
  message?: string;
  /** Binary data encoded as base64 */
  data?: string;
  /** Participant/sender identity */
  identity: string;
  /** Topic/channel for the message */
  topic: string;
  /** Room/space identifier */
  room: string;
  /** Timestamp when the message was received */
  timestamp: number;
}

/**
 * Message received from participants
 */
export interface MessageMessage {
  type: 'message';
  /** Unified message structure */
  message: UnifiedMessage;
}

/**
 * Participant disconnection information
 */
export interface ParticipantDisconnectedInfo {
  /** Participant's unique identity */
  identity: string;
  /** Participant's display name */
  name?: string;
  /** Room identifier */
  room: string;
  /** Timestamp when the disconnection occurred */
  timestamp: number;
}

/**
 * Participant disconnected message
 */
export interface ParticipantDisconnectedMessage {
  type: 'participant_disconnected';
  /** Information about the participant who disconnected */
  participant: ParticipantDisconnectedInfo;
}

/**
 * TTS playback completion notification
 */
export interface TTSPlaybackCompleteMessage {
  type: 'tts_playback_complete';
  /** Timestamp when completion occurred (milliseconds since epoch) */
  timestamp: number;
}

/**
 * Error message. An uncoded gateway error is just `{type, message}`; a coded one adds `code`,
 * `recoverable` and `details` (and `message` keeps its `"{code}: "` prefix).
 */
export interface ErrorMessage {
  type: 'error';
  /** Stable machine code (e.g. `stt_live_unsupported`); absent on an uncoded error. */
  code?: string;
  /** Error message */
  message: string;
  /** Structured detail of a coded error. */
  details?: Record<string, unknown>;
  /**
   * `true`: the socket is still usable; a setup refusal accepts a corrected `config`.
   * `false`: the session is over or will never transcribe. Absent on an uncoded error.
   */
  recoverable?: boolean;
}

/**
 * TTS audio chunk message
 */
export interface TTSAudioMessage {
  type: 'tts_audio';
  /** Base64 encoded audio data */
  audio: string;
  /** Audio format */
  format?: string;
  /** Sample rate */
  sample_rate?: number;
  /** Duration in seconds */
  duration?: number;
  /** Whether this is the final chunk */
  is_final?: boolean;
  /** Sequence number */
  sequence?: number;
}

/**
 * Pong response to ping
 */
export interface PongMessage {
  type: 'pong';
  /** Original ping timestamp */
  timestamp: number;
  /** Server time */
  server_time?: number;
}

/**
 * Session update message
 */
export interface SessionUpdateMessage {
  type: 'session_update';
  /** Field that was updated */
  field: string;
  /** New value */
  value: unknown;
  /** Previous value */
  previous_value?: unknown;
}

/**
 * SIP transfer error message
 */
export interface SIPTransferErrorMessage {
  type: 'sip_transfer_error';
  /** Error message describing why the transfer failed */
  message: string;
}

/**
 * Non-fatal gateway config advisory (gateway OutgoingMessage::ConfigWarning,
 * handlers/ws/messages.rs). Emitted when a config is accepted but something was
 * silently degraded (unknown/misnested key, emotion ignored by provider,
 * reasoning model on the voice path, ...). NEVER closes the session.
 */
export interface ConfigWarningMessage {
  type: 'config_warning';
  /** Stable machine code (e.g. "unknown_config_keys"). */
  code: string;
  /** Human-readable explanation + a one-line fix hint. */
  message: string;
  /** Optional free-form JSON detail (e.g. { ignored_keys: [...] }). */
  detail?: Record<string, unknown>;
}

/** The `event` of a {@link VadEventMessage}. */
export type VadEventKind = 'speech_start' | 'speech_end' | 'turn_start' | 'turn_end' | 'turn_closed';

/**
 * Detector-timed speech events and the gateway's turn decisions (segmented STT). Which ones arrive
 * is given by `ready.stt.speech_events`; match them by `turn_id`.
 */
export interface VadEventMessage {
  type: 'vad_event';
  /** `speech_start`, `speech_end`, `turn_start`, `turn_end` or `turn_closed`. */
  event: OpenString<VadEventKind>;
  /** The turn this event belongs to. */
  turn_id: number;
  /** Position in the received audio (first speech sample for starts, last for ends). */
  audio_ms?: number;
  /** Sustained speech before the gateway took the turn (`turn_start`). */
  sustained_ms?: number;
  /** The cut segment was discarded rather than uploaded (`speech_end`). */
  discarded?: boolean;
  /** Whether the turn produced text (`turn_closed`). */
  had_transcript?: boolean;
  /** Why a turn closed without text (`turn_closed`): `no_speech`, `transcription_failed`, `ignored`. */
  reason?: OpenString<'no_speech' | 'transcription_failed' | 'ignored'>;
}

/**
 * A speech-to-text problem that does not end the call (a lost segment, dropped audio, rate
 * limiting). Never sent as `error`, so never treat it as a disconnect.
 */
export interface SttWarningMessage {
  type: 'stt_warning';
  /** Stable machine code (e.g. `stt_segment_failed`). */
  code: string;
  /** Human-readable explanation. */
  message: string;
  /** Optional structured detail (e.g. `{turn_id, segment_seq, ...}`). */
  detail?: Record<string, unknown>;
}

/**
 * Union type for all incoming messages (received by client from server)
 */
export type IncomingMessage =
  | ReadyMessage
  | STTResultMessage
  | MessageMessage
  | ParticipantDisconnectedMessage
  | TTSPlaybackCompleteMessage
  | TTSAudioMessage
  | PongMessage
  | SessionUpdateMessage
  | ErrorMessage
  | SIPTransferErrorMessage
  | ConfigWarningMessage
  | VadEventMessage
  | SttWarningMessage;

/**
 * Common message type identifier
 */
export type MessageType =
  // Outgoing (client -> server)
  | 'config'
  | 'speak'
  | 'clear'
  | 'audio_end'
  | 'send_message'
  | 'sip_transfer'
  // Incoming (server -> client)
  | 'ready'
  | 'stt_result'
  | 'message'
  | 'participant_disconnected'
  | 'tts_playback_complete'
  | 'tts_audio'
  | 'pong'
  | 'session_update'
  | 'error'
  | 'sip_transfer_error'
  | 'config_warning'
  | 'vad_event'
  | 'stt_warning';

// ============================================================================
// Message Serialization Helpers
// ============================================================================

/**
 * Convert SDK config types to wire format
 */
export function toConfigMessage(
  streamId?: string,
  sttConfig?: STTConfig,
  ttsConfig?: TTSConfig,
  livekitConfig?: LiveKitConfig,
  audio = true
): ConfigMessage {
  const msg: ConfigMessage = {
    type: 'config',
    audio,
  };

  if (streamId) {
    msg.stream_id = streamId;
  }

  if (sttConfig) {
    msg.stt_config = {
      provider: sttConfig.provider,
      language: sttConfig.language ?? 'en-US',
      sample_rate: sttConfig.sampleRate ?? 16000,
      channels: sttConfig.channels ?? 1,
      punctuation: sttConfig.punctuation ?? true,
      encoding: sttConfig.encoding ?? 'linear16',
      model: sttConfig.model ?? '',
    };
  }

  if (ttsConfig) {
    msg.tts_config = {
      provider: ttsConfig.provider,
      model: ttsConfig.model ?? '',
      voice_id: ttsConfig.voiceId,
      speaking_rate: ttsConfig.speakingRate,
      audio_format: ttsConfig.audioFormat,
      sample_rate: ttsConfig.sampleRate,
      connection_timeout: ttsConfig.connectionTimeout,
      request_timeout: ttsConfig.requestTimeout,
      pronunciations: ttsConfig.pronunciations?.map((p) => ({ from: p.from, to: p.to })),
    };
  }

  if (livekitConfig) {
    msg.livekit = {
      room_name: livekitConfig.roomName,
      enable_recording: livekitConfig.enableRecording,
      waav_participant_identity: livekitConfig.waavParticipantIdentity,
      waav_participant_name: livekitConfig.waavParticipantName,
      listen_participants: livekitConfig.listenParticipants,
    };
  }

  return msg;
}

/**
 * Create a speak message
 */
export function toSpeakMessage(text: string, flush?: boolean, allowInterruption?: boolean): SpeakMessage {
  return {
    type: 'speak',
    text,
    flush,
    allow_interruption: allowInterruption,
  };
}

/**
 * Create a clear message
 */
export function toClearMessage(): ClearMessage {
  return { type: 'clear' };
}

/**
 * Parse an incoming message from JSON (messages received from server)
 */
export function parseIncomingMessage(json: string): IncomingMessage {
  return JSON.parse(json) as IncomingMessage;
}

/**
 * Serialize an outgoing message to JSON (messages sent to server)
 */
export function serializeOutgoingMessage(message: OutgoingMessage): string {
  return JSON.stringify(message);
}
