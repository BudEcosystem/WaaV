/**
 * Type definitions for @bud-foundry/sdk
 */

// Configuration types
export type {
  STTConfig,
  TTSConfig,
  Pronunciation,
  LiveKitConfig,
  SessionConfig,
  // Emotion types (Unified Emotion System)
  Emotion,
  DeliveryStyle,
  EmotionIntensityLevel,
  EmotionConfig,
  // Voice cloning types
  VoiceCloneProvider,
  VoiceCloneRequest,
  VoiceCloneResponse,
  // Hume EVI types
  HumeEVIVersion,
  HumeEVIConfig,
  ProsodyScores,
} from './config.js';
export {
  DEFAULT_STT_CONFIG,
  DEFAULT_TTS_CONFIG,
  createSTTConfig,
  createTTSConfig,
  // Emotion helper functions
  getTopEmotions,
  getDominantEmotion,
  intensityToNumber,
} from './config.js';

// Message types
export type {
  ConfigMessage,
  SpeakMessage,
  ClearMessage,
  SendMessageMessage,
  SIPTransferMessage,
  IncomingMessage,
  ReadyMessage,
  STTResultMessage,
  UnifiedMessage,
  MessageMessage,
  ParticipantDisconnectedInfo,
  ParticipantDisconnectedMessage,
  TTSPlaybackCompleteMessage,
  ErrorMessage,
  SIPTransferErrorMessage,
  OutgoingMessage,
} from './messages.js';
export {
  toConfigMessage,
  toSpeakMessage,
  toClearMessage,
  parseOutgoingMessage,
  serializeIncomingMessage,
} from './messages.js';

// STT types
export type {
  WordTiming,
  SpeakerInfo,
  STTResult,
  TranscriptEvent,
  STTConnectOptions,
  STTFeatures,
} from './stt.js';
export { DEFAULT_STT_FEATURES, parseSTTResult } from './stt.js';

// TTS types
export type {
  Voice,
  TTSAudioChunk,
  SpeakOptions,
  TTSConnectOptions,
  TTSSynthesisResult,
  TTSPlaybackCompleteEvent,
  VoiceListResponse,
} from './tts.js';
export { VOICE_DEFAULTS } from './tts.js';

// LiveKit types
export type {
  LiveKitTokenRequest,
  LiveKitTokenResponse,
  RoomInfo,
  ParticipantInfo,
  TrackInfo,
  RoomListResponse,
  LiveKitConnectOptions,
} from './livekit.js';

// SIP types
export type {
  SIPHook,
  SIPHookListResponse,
  SIPHookCreateRequest,
  SIPHookCreateResponse,
  SIPTransferRequest,
  SIPTransferResult,
  SIPCallInfo,
  SIPWebhookEvent,
} from './sip.js';
export { isValidPhoneNumber, normalizePhoneNumber } from './sip.js';

// Metrics types
export type {
  PercentileStats,
  MetricPoint,
  STTMetrics,
  TTSMetrics,
  WebSocketMetrics,
  E2EMetrics,
  AudioMetrics,
  ResourceMetrics,
  MetricsSummary,
  SLOThreshold,
  SLOStatus,
} from './metrics.js';
export { DEFAULT_SLOS, emptyPercentileStats, emptyMetricsSummary } from './metrics.js';

// Feature flags
export type { FeatureFlags, FeatureFlagInfo } from './features.js';
export {
  FEATURE_FLAGS,
  DEFAULT_FEATURE_FLAGS,
  isFeatureSupported,
  getSupportedFeatures,
  mergeFeatures,
} from './features.js';

// Realtime (Audio-to-Audio) types
export type {
  RealtimeProvider,
  OpenAIRealtimeModel,
  OpenAIRealtimeVoice,
  VADConfig,
  InputTranscriptionConfig,
  RealtimeSessionConfig,
  RealtimeTranscript,
  SpeechEvent,
  RealtimeAudioChunk,
  RealtimeSessionState,
  RealtimeSessionEvents,
  IRealtimeSession,
} from './realtime.js';
export { REALTIME_DEFAULTS, createRealtimeConfig } from './realtime.js';
