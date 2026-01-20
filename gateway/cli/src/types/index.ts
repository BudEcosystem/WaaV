/**
 * WaaV CLI Type Definitions
 *
 * Re-exports all types used throughout the CLI
 */

// Configuration types
export * from './config.js';

// Provider types
export * from './provider.js';

// DAG pipeline types
export * from './dag.js';

// Navigation types (SPA architecture)
export * from './navigation.js';

// Gateway message types (aligned with TypeScript SDK)
export interface GatewayMessage {
  type: string;
  [key: string]: unknown;
}

// Config message sent to gateway
export interface ConfigMessage extends GatewayMessage {
  type: 'config';
  stream_id?: string;
  stt_config?: STTConfigMessage;
  tts_config?: TTSConfigMessage;
  features?: FeatureFlags;
}

export interface STTConfigMessage {
  provider: string;
  language?: string;
  model?: string;
  interim_results?: boolean;
  punctuation?: boolean;
  profanity_filter?: boolean;
  [key: string]: unknown;
}

export interface TTSConfigMessage {
  provider: string;
  voice_id?: string;
  model?: string;
  speed?: number;
  pitch?: number;
  [key: string]: unknown;
}

export interface FeatureFlags {
  vad?: boolean;
  noise_filter?: boolean;
  turn_detection?: boolean;
  emotion_detection?: boolean;
}

// Ready message from gateway
export interface ReadyMessage extends GatewayMessage {
  type: 'ready';
  stream_id: string;
  livekit_room?: string;
  livekit_token?: string;
}

// STT result from gateway
export interface STTResultMessage extends GatewayMessage {
  type: 'stt_result';
  transcript: string;
  is_final: boolean;
  confidence?: number;
  language?: string;
  timings?: {
    start_ms: number;
    end_ms: number;
  };
  words?: Array<{
    word: string;
    start_ms: number;
    end_ms: number;
    confidence: number;
  }>;
  speaker?: string;
  emotions?: Record<string, number>;
}

// TTS audio chunk from gateway
export interface TTSAudioMessage extends GatewayMessage {
  type: 'tts_audio';
  audio: Uint8Array | string; // Base64 or raw bytes
  is_final: boolean;
  sample_rate?: number;
  duration_ms?: number;
}

// Error message from gateway
export interface ErrorMessage extends GatewayMessage {
  type: 'error';
  code: string;
  message: string;
  recoverable: boolean;
  details?: Record<string, unknown>;
}

// Speak command to gateway
export interface SpeakMessage extends GatewayMessage {
  type: 'speak';
  text: string;
  voice_id?: string;
  speed?: number;
  emotion?: string;
}

// Audio data to gateway
export interface AudioMessage extends GatewayMessage {
  type: 'audio';
  audio: Uint8Array | string; // Raw PCM or Base64
  sample_rate?: number;
  channels?: number;
  encoding?: 'pcm_s16le' | 'pcm_f32le' | 'opus' | 'mp3';
}

// Interrupt command
export interface InterruptMessage extends GatewayMessage {
  type: 'interrupt';
}

// Flush command
export interface FlushMessage extends GatewayMessage {
  type: 'flush';
}

// Ping/Pong for keepalive
export interface PingMessage extends GatewayMessage {
  type: 'ping';
  timestamp: number;
}

export interface PongMessage extends GatewayMessage {
  type: 'pong';
  timestamp: number;
}

// Gateway status for health checks
export interface GatewayStatus {
  healthy: boolean;
  version: string;
  uptime_seconds: number;
  connections: number;
  providers: {
    stt: string[];
    tts: string[];
    realtime: string[];
  };
  features: string[];
  livekit_enabled: boolean;
  // Extended status fields (optional, for dashboard)
  stt_provider?: string;
  tts_provider?: string;
  requests_total?: number;
  requests_per_minute?: number;
  avg_latency_ms?: number;
  stt_requests?: number;
  tts_requests?: number;
  stt_latency_ms?: number;
  tts_latency_ms?: number;
  error_rate?: number;
}

// Voice info from gateway /voices endpoint
export interface GatewayVoice {
  id: string;
  name: string;
  provider: string;
  language: string;
  language_code: string;
  gender?: string;
  style?: string;
  preview_url?: string;
}

// CLI-specific types
export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

export interface CLIContext {
  config: import('./config.js').WaavConfig;
  configPath: string;
  profile: string;
  verbose: boolean;
  quiet: boolean;
  noColor: boolean;
  noAnimation: boolean;
  jsonOutput: boolean;
}

// Note: ScreenName and NavigationState types are now in navigation.ts
// They are re-exported from './navigation.js' above

// Audio device info
export interface AudioDevice {
  id: string;
  name: string;
  isDefault: boolean;
  isInput: boolean;
  isOutput: boolean;
  sampleRate?: number;
  channels?: number;
}

// Metrics types
export interface CLIMetrics {
  session_start: Date;
  commands_executed: number;
  voice_sessions: number;
  total_audio_seconds: number;
  stt_requests: number;
  tts_requests: number;
  errors: number;
  latencies: {
    stt_p50_ms: number;
    stt_p99_ms: number;
    tts_p50_ms: number;
    tts_p99_ms: number;
  };
}
