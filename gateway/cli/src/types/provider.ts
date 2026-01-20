/**
 * WaaV CLI Provider Types
 *
 * Defines provider metadata, capabilities, and configuration schemas
 */

import { z } from 'zod';

// Provider categories
export type ProviderCategory = 'stt' | 'tts' | 'realtime';

// Provider status in CLI
export type ProviderStatus = 'configured' | 'available' | 'unavailable' | 'error';

// STT provider capabilities
export const STTCapabilitiesSchema = z.object({
  streaming: z.boolean().default(false),
  interim_results: z.boolean().default(false),
  word_timestamps: z.boolean().default(false),
  speaker_diarization: z.boolean().default(false),
  punctuation: z.boolean().default(true),
  profanity_filter: z.boolean().default(false),
  custom_vocabulary: z.boolean().default(false),
  language_detection: z.boolean().default(false),
  emotion_detection: z.boolean().default(false),
  sentiment_analysis: z.boolean().default(false),
});

export type STTCapabilities = z.infer<typeof STTCapabilitiesSchema>;

// TTS provider capabilities
export const TTSCapabilitiesSchema = z.object({
  streaming: z.boolean().default(false),
  voice_cloning: z.boolean().default(false),
  ssml: z.boolean().default(false),
  emotion_control: z.boolean().default(false),
  speed_control: z.boolean().default(true),
  pitch_control: z.boolean().default(false),
  custom_voices: z.boolean().default(false),
  multilingual: z.boolean().default(false),
  word_timestamps: z.boolean().default(false),
});

export type TTSCapabilities = z.infer<typeof TTSCapabilitiesSchema>;

// Realtime provider capabilities
export const RealtimeCapabilitiesSchema = z.object({
  audio_to_audio: z.boolean().default(true),
  text_to_audio: z.boolean().default(false),
  audio_to_text: z.boolean().default(false),
  function_calling: z.boolean().default(false),
  turn_detection: z.boolean().default(true),
  voice_selection: z.boolean().default(true),
  system_prompt: z.boolean().default(true),
});

export type RealtimeCapabilities = z.infer<typeof RealtimeCapabilitiesSchema>;

// Provider configuration field definition
export interface ProviderConfigField {
  name: string;
  label: string;
  type: 'string' | 'password' | 'number' | 'boolean' | 'select' | 'multiselect';
  required: boolean;
  default?: unknown;
  description?: string;
  placeholder?: string;
  options?: { value: string; label: string }[];
  validation?: {
    min?: number;
    max?: number;
    pattern?: string;
    message?: string;
  };
  env_var?: string;
}

// Provider metadata
export interface ProviderInfo {
  id: string;
  name: string;
  category: ProviderCategory;
  description: string;
  website: string;
  docs_url?: string;
  logo_url?: string;
  pricing_url?: string;

  // Supported features
  capabilities: STTCapabilities | TTSCapabilities | RealtimeCapabilities;

  // Languages supported
  languages: string[];

  // Models available
  models: {
    id: string;
    name: string;
    description?: string;
    default?: boolean;
  }[];

  // Configuration fields
  config_fields: ProviderConfigField[];

  // Environment variables for API keys
  env_vars: {
    api_key?: string;
    additional?: Record<string, string>;
  };

  // Regional availability
  regions?: string[];

  // Tags for filtering
  tags: string[];

  // Is this a cloud provider or local?
  type: 'cloud' | 'local' | 'hybrid';

  // Status in the system
  status?: ProviderStatus;

  // Is this provider configured with valid credentials?
  is_configured?: boolean;
}

// STT-specific provider info
export interface STTProviderInfo extends ProviderInfo {
  category: 'stt';
  capabilities: STTCapabilities;
}

// TTS-specific provider info
export interface TTSProviderInfo extends ProviderInfo {
  category: 'tts';
  capabilities: TTSCapabilities;
  voices?: VoiceInfo[];
}

// Realtime-specific provider info
export interface RealtimeProviderInfo extends ProviderInfo {
  category: 'realtime';
  capabilities: RealtimeCapabilities;
}

// Voice information for TTS providers
export interface VoiceInfo {
  id: string;
  name: string;
  provider: string;
  language: string;
  language_code: string;
  gender?: 'male' | 'female' | 'neutral' | 'unknown';
  age?: 'child' | 'young' | 'adult' | 'senior';
  accent?: string;
  style?: string;
  description?: string;
  preview_url?: string;
  is_custom?: boolean;
  is_cloned?: boolean;
}

// Provider test result
export interface ProviderTestResult {
  provider_id: string;
  success: boolean;
  latency_ms?: number;
  error?: string;
  details?: {
    api_reachable: boolean;
    auth_valid: boolean;
    quota_available?: boolean;
    rate_limit_status?: string;
  };
  tested_at: Date;
}

// Provider health status from gateway
export interface ProviderHealthStatus {
  provider_id: string;
  category: ProviderCategory;
  status: 'healthy' | 'degraded' | 'unhealthy' | 'unknown';
  latency_p50_ms?: number;
  latency_p99_ms?: number;
  error_rate?: number;
  last_error?: string;
  last_success?: Date;
  uptime_percent?: number;
}

// Comprehensive provider list (subset of 70+ providers)
export const STT_PROVIDERS: readonly string[] = [
  'deepgram',
  'google',
  'azure',
  'aws_transcribe',
  'openai',
  'assemblyai',
  'rev_ai',
  'speechmatics',
  'whisper',
  'groq',
  'cartesia',
  'soniox',
  'symbl',
  'vosk',
  'picovoice',
  'alibaba',
  'baidu',
  'tencent',
  'iflytek',
  'sarvam',
  'bhashini',
  'krutrim',
  'vakyansh',
  'gladia',
  'lmnt',
  'eleven_labs',
  'hume',
  // Indian & Regional
  'gnani',
  'reverie',
  // Voice Biometrics & Enterprise
  'phonexia',
  // Russian Providers
  'yandex',
  'tinkoff',
  'sberdevices',
] as const;

export const TTS_PROVIDERS: readonly string[] = [
  'elevenlabs',
  'google',
  'azure',
  'aws_polly',
  'openai',
  'cartesia',
  'deepgram',
  'play_ht',
  'murf',
  'lovo',
  'resemble',
  'wellsaid',
  'coqui',
  'bark',
  'tortoise',
  'piper',
  'xtts',
  'lmnt',
  'unrealspeech',
  'fish',
  'neets',
  'styletts2',
  'hume',
  'alibaba',
  'baidu',
  'tencent',
  'iflytek',
  'microsoft_edge',
  'voicevox',
  'silero',
  'espeak',
  'festival',
  // New additions
  'speechify',
  'smallest',
  // Russian Providers
  'yandex',
  'sberdevices',
] as const;

export const REALTIME_PROVIDERS: readonly string[] = [
  'openai_realtime',
  'hume',
  'deepgram_voice_agent',
  'livekit_agents',
  'vapi',
  'retell',
  'bland',
] as const;

export type STTProviderID = (typeof STT_PROVIDERS)[number];
export type TTSProviderID = (typeof TTS_PROVIDERS)[number];
export type RealtimeProviderID = (typeof REALTIME_PROVIDERS)[number];
export type ProviderID = STTProviderID | TTSProviderID | RealtimeProviderID;
