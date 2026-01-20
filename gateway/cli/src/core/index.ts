/**
 * WaaV CLI Core Module
 *
 * Re-exports all core services
 */

// Configuration
export {
  ConfigManager,
  configManager,
  store,
  DEFAULT_PROVIDER_CONFIGS,
  EXAMPLE_CONFIGS,
  PROVIDER_ENV_VARS,
  getProviderEnvVar,
  getProviderEnvVars,
} from './config/index.js';

// Gateway
export {
  GatewayClient,
  WebSocketClient,
  type GatewayClientConfig,
  type WebSocketClientConfig,
  type WebSocketClientEvents,
  type SessionConfig,
  type ConnectionState,
} from './gateway/index.js';

// Audio
export {
  AudioRecorder,
  AudioPlayer,
  VAD,
  type AudioRecorderConfig,
  type AudioRecorderEvents,
  type RecorderState,
  type AudioPlayerConfig,
  type AudioPlayerEvents,
  type PlayerState,
  type VADConfig,
  type VADEvents,
  type VADState,
} from './audio/index.js';
