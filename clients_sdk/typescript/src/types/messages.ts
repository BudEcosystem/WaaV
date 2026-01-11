/**
 * WebSocket Message Types
 *
 * Message types are named from the CLIENT SDK perspective:
 * - OutgoingMessage: Messages the CLIENT sends TO the server
 * - IncomingMessage: Messages the CLIENT receives FROM the server
 *
 * Note: This is the opposite of Sayna's (server) naming convention in messages.rs
 * where IncomingMessage = what server receives and OutgoingMessage = what server sends.
 */

import type { STTConfig, TTSConfig, LiveKitConfig, Emotion, DeliveryStyle, EmotionIntensityLevel } from './config.js';

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
    sayna_participant_identity?: string;
    sayna_participant_name?: string;
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
 * Clear message to stop current TTS playback
 */
export interface ClearMessage {
  type: 'clear';
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
  /** Unique identifier for this WebSocket session */
  stream_id: string;
  /** LiveKit room name that was created */
  livekit_room_name?: string;
  /** LiveKit URL to connect to */
  livekit_url?: string;
  /** Identity of the AI agent participant in the room */
  sayna_participant_identity?: string;
  /** Display name of the AI agent participant */
  sayna_participant_name?: string;
}

/**
 * STT result message containing transcription
 */
export interface STTResultMessage {
  type: 'stt_result';
  /** Transcribed text */
  transcript: string;
  /** Whether this is the final version of the transcript */
  is_final: boolean;
  /** Whether speech has ended */
  is_speech_final: boolean;
  /** Confidence score (0.0 to 1.0) */
  confidence: number;
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
 * Error message
 */
export interface ErrorMessage {
  type: 'error';
  /** Error code */
  code?: string;
  /** Error message */
  message: string;
  /** Additional error details */
  details?: Record<string, unknown>;
  /** Whether the error is recoverable */
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
  | SIPTransferErrorMessage;

/**
 * Common message type identifier
 */
export type MessageType =
  | 'config'
  | 'speak'
  | 'clear'
  | 'send_message'
  | 'sip_transfer'
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
  | 'ping'
  | 'audio'
  | 'stop'
  | 'flush'
  | 'interrupt'
  | 'speaking_started'
  | 'speaking_finished'
  | 'listening_started'
  | 'listening_stopped';

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
      language: sttConfig.language,
      sample_rate: sttConfig.sampleRate ?? 16000,
      channels: sttConfig.channels ?? 1,
      punctuation: sttConfig.punctuation ?? true,
      encoding: sttConfig.encoding ?? 'linear16',
      model: sttConfig.model,
    };
  }

  if (ttsConfig) {
    msg.tts_config = {
      provider: ttsConfig.provider,
      model: ttsConfig.model,
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
      sayna_participant_identity: livekitConfig.saynaParticipantIdentity,
      sayna_participant_name: livekitConfig.saynaParticipantName,
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
