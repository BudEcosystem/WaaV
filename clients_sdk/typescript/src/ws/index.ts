/**
 * WebSocket module for @bud-foundry/sdk
 */

export { WebSocketConnection, type WebSocketConnectionOptions, type ConnectionState, type ConnectionEventHandlers } from './connection.js';
export { ReconnectStrategy, type ReconnectConfig, type ReconnectState, type ReconnectHandlers, DEFAULT_RECONNECT_CONFIG } from './reconnect.js';
export { LivenessWatchdog, type LivenessConfig, type ResolvedLivenessConfig, DEFAULT_LIVENESS_CONFIG } from './liveness.js';
export { UplinkAudioShedder, type UplinkConfig, type ResolvedUplinkConfig, DEFAULT_UPLINK_CONFIG } from './uplink.js';
export { WebSocketSession, type SessionConfig, type SessionState } from './session.js';
export { MessageQueue, type MessageQueueConfig } from './queue.js';
export {
  SessionEventEmitter,
  type SessionEventMap,
  type SessionEventHandler,
  type TranscriptEvent,
  type AudioEvent,
  type ReadyEvent,
  type SessionErrorEvent,
  type ConfigWarningEvent,
  type ConnectionStateEvent,
  type MetricsEvent,
  type ReconnectEvent,
  type SpeakingEvent,
  type ListeningEvent,
} from './events.js';
export {
  serializeMessage,
  deserializeMessage,
  createConfigMessage,
  createSpeakMessage,
  createClearMessage,
  createAudioEndMessage,
  createSendMessageMessage,
} from './messages.js';
export type { SDKConfigMessage, SDKOutgoingMessage } from './messages.js';
