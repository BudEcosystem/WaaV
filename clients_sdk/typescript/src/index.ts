/**
 * @bud-foundry/sdk
 *
 * TypeScript SDK for Bud Foundry AI Gateway
 * Provides Speech-to-Text, Text-to-Speech, and Voice AI capabilities
 *
 * @example
 * ```typescript
 * import { BudClient } from '@bud-foundry/sdk';
 *
 * const bud = new BudClient({
 *   baseUrl: 'http://localhost:3001',
 *   apiKey: 'your-api-key'
 * });
 *
 * // Speech-to-Text
 * const stt = await bud.stt.connect({ provider: 'deepgram' });
 * stt.on('transcript', (t) => console.log(t.text));
 * await stt.startListening();
 *
 * // Text-to-Speech
 * const tts = await bud.tts.connect({ provider: 'elevenlabs' });
 * await tts.speak('Hello, world!');
 *
 * // Bidirectional Voice
 * const talk = await bud.talk.connect({
 *   stt: { provider: 'deepgram' },
 *   tts: { provider: 'elevenlabs' }
 * });
 * await talk.startListening();
 * ```
 */

// Types (selectively exported to avoid conflicts)
export type {
  STTConfig,
  TTSConfig,
  Pronunciation,
  LiveKitConfig,
} from './types/config.js';
export {
  DEFAULT_STT_CONFIG,
  DEFAULT_TTS_CONFIG,
  createSTTConfig,
  createTTSConfig,
} from './types/config.js';

export type {
  ConfigMessage,
  SpeakMessage,
  IncomingMessage,
  ReadyMessage,
  STTResultMessage,
  ErrorMessage,
  OutgoingMessage,
} from './types/messages.js';

export type {
  WordTiming,
  SpeakerInfo,
  STTResult,
  STTConnectOptions,
  STTFeatures,
} from './types/stt.js';
export { DEFAULT_STT_FEATURES, parseSTTResult } from './types/stt.js';

export type {
  Voice,
  TTSAudioChunk,
  SpeakOptions,
  TTSConnectOptions,
  TTSSynthesisResult,
  TTSPlaybackCompleteEvent,
  VoiceListResponse,
} from './types/tts.js';
export { VOICE_DEFAULTS } from './types/tts.js';

export type {
  LiveKitTokenRequest,
  LiveKitTokenResponse,
  RoomInfo,
  ParticipantInfo,
  TrackInfo,
  RoomListResponse,
  LiveKitConnectOptions,
} from './types/livekit.js';

export type {
  SIPHook,
  SIPHookListResponse,
  SIPHookCreateRequest,
  SIPHookCreateResponse,
  SIPTransferRequest,
  SIPTransferResult,
} from './types/sip.js';

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
} from './types/metrics.js';
export { DEFAULT_SLOS, emptyPercentileStats, emptyMetricsSummary } from './types/metrics.js';

export type { FeatureFlags, FeatureFlagInfo } from './types/features.js';
export {
  FEATURE_FLAGS,
  DEFAULT_FEATURE_FLAGS,
  isFeatureSupported,
  getSupportedFeatures,
  mergeFeatures,
} from './types/features.js';

// Provider types (comprehensive list)
export type {
  STTProvider,
  TTSProvider,
  RealtimeProvider as RealtimeProviderType,
  STTCapabilities,
  TTSCapabilities,
  RealtimeCapabilities,
  ProviderCapabilities,
} from './types/providers.js';
export {
  STT_PROVIDERS,
  TTS_PROVIDERS,
  REALTIME_PROVIDERS,
  isValidSTTProvider,
  isValidTTSProvider,
  isValidRealtimeProvider,
  getProviderCapabilities,
  getProvidersWithFeature,
  getDefaultModel,
  getDefaultVoice,
} from './types/providers.js';

// DAG routing types
export type {
  DAGNodeType,
  DAGNode,
  DAGEdge,
  DAGDefinition,
  DAGConfig,
  DAGValidationResult,
} from './types/dag.js';
export {
  DAG_NODE_TYPES,
  DEFAULT_DAG_CONFIG,
  validateDAGDefinition,
  createDAGConfig,
  serializeDAGConfig,
  deserializeDAGConfig,
  getBuiltinTemplate,
  BUILTIN_TEMPLATES,
} from './types/dag.js';

// Audio features types
export type {
  TurnDetectionConfig,
  NoiseFilterConfig,
  AudioFeatures,
} from './types/audio-features.js';
export {
  DEFAULT_TURN_DETECTION,
  DEFAULT_NOISE_FILTER,
  DEFAULT_VAD,
  createAudioFeatures,
  serializeAudioFeatures,
} from './types/audio-features.js';

// Voice cloning and recording types
export type {
  VoiceCloneFilter,
  RecordingStatus,
  RecordingFormat,
  RecordingInfo,
  RecordingFilter,
  RecordingDownloadOptions,
  RecordingList,
} from './types/voice.js';
export { VOICE_CLONE_PROVIDERS } from './types/voice.js';

// Errors
export * from './errors/index.js';

// Metrics
export * from './metrics/index.js';

// REST Client
export * from './rest/index.js';

// WebSocket (with renamed types to avoid conflicts)
export {
  WebSocketSession,
  WebSocketConnection,
  ReconnectStrategy,
  MessageQueue,
  SessionEventEmitter,
  serializeMessage,
  deserializeMessage,
  createConfigMessage,
  createSpeakMessage,
} from './ws/index.js';
export type {
  SessionConfig as WSSessionConfig,
  SessionState,
  WebSocketConnectionOptions,
  ConnectionState,
  ReconnectConfig,
  ReconnectState,
  MessageQueueConfig,
  SessionEventMap,
  SessionEventHandler,
  TranscriptEvent,
  AudioEvent,
  ReadyEvent,
  SessionErrorEvent,
  ConnectionStateEvent,
  MetricsEvent,
  ReconnectEvent,
  SpeakingEvent,
  ListeningEvent,
} from './ws/index.js';

// Audio Utilities
export * from './audio/index.js';

// Pipelines
export * from './pipelines/index.js';

// Main Client
export { BudClient, createBudClient } from './bud.js';
export type { BudClientConfig } from './bud.js';

// Version
export const VERSION = '0.1.0';
