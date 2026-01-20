/**
 * WaaV CLI Gateway Module
 *
 * Re-exports gateway client and WebSocket utilities
 */

export { GatewayClient, type GatewayClientConfig } from './client.js';
export {
  WebSocketClient,
  type WebSocketClientConfig,
  type WebSocketClientEvents,
  type SessionConfig,
  type ConnectionState,
} from './websocket.js';
