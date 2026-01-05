/**
 * WebSocket module for @bud-foundry/sdk
 */

export { WebSocketConnection, type WebSocketConnectionOptions, type ConnectionState, type ConnectionEventHandlers } from './connection.js';
export { ReconnectStrategy, type ReconnectConfig, type ReconnectState, type ReconnectHandlers, DEFAULT_RECONNECT_CONFIG } from './reconnect.js';
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
  createPingMessage,
  createAudioMessage,
  createStopMessage,
  createFlushMessage,
  createInterruptMessage,
} from './messages.js';
