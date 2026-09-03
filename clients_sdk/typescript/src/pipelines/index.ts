/**
 * Pipelines module for @bud-foundry/sdk
 */

export { BasePipeline, type BasePipelineConfig, type PipelineState } from './base.js';
export { BudSTT, createBudSTT, type BudSTTConfig } from './stt.js';
export { BudTTS, createBudTTS, type BudTTSConfig } from './tts.js';
export { BudTalk, createBudTalk, type BudTalkConfig, type ConversationTurn } from './talk.js';
export { AgentSession, buildAgentSessionConfig, type AgentConfig, type AgentTurnConfig } from './agent.js';
export { BudTranscribe, createBudTranscribe, type BudTranscribeConfig, type TranscriptionResult, type TranscriptionProgress } from './transcribe.js';
export {
  BudRealtime,
  type RealtimeConfig,
  type RealtimeProvider,
  type RealtimeState,
  type ToolDefinition,
  type FunctionCallEvent,
  type TranscriptEvent as RealtimeTranscriptEvent,
  type AudioEvent as RealtimeAudioEvent,
  type EmotionEvent,
  type StateChangeEvent,
  type RealtimeEvents,
} from './realtime.js';
export {
  GatewayRealtime,
  REALTIME_PROVIDERS,
  getSupportedRealtimeProviders,
  gatewayRealtimeConfigToWire,
  type GatewayRealtimeConfig,
  type GatewayRealtimeProvider,
  type GatewayRealtimeState,
  type GatewayTurnDetection,
  type GatewayRealtimeTool,
  type GatewayRealtimeEvent,
  type GatewayRealtimeEvents,
  type GatewayTranscriptEvent,
  type GatewayAudioEvent,
  type GatewayFunctionCallEvent,
  type GatewaySpeechEvent,
  type GatewayErrorEvent,
  type GatewaySessionCreatedEvent,
  type GatewayResponseEvent,
  type GatewayResponseOverride,
  type GatewayRealtimeOptions,
} from './realtime-gateway.js';
