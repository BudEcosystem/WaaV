/**
 * WaaV CLI Configuration Types
 *
 * Defines the schema for ~/.waavrc.yaml configuration file
 */

import { z } from 'zod';

// Provider configuration schemas
export const ProviderConfigSchema = z.object({
  api_key: z.string().optional(),
  model: z.string().optional(),
  voice_id: z.string().optional(),
  language: z.string().optional(),
  region: z.string().optional(),
  custom_endpoint: z.string().url().optional(),
  options: z.record(z.unknown()).optional(),
});

export type ProviderConfig = z.infer<typeof ProviderConfigSchema>;

// Gateway configuration
export const GatewayConfigSchema = z.object({
  url: z.string().url().default('http://localhost:3001'),
  auth: z.object({
    api_key: z.string().optional(),
    jwt_token: z.string().optional(),
  }).optional(),
  timeout: z.number().positive().default(30000),
  retry: z.object({
    attempts: z.number().int().positive().default(3),
    delay: z.number().positive().default(1000),
    backoff_multiplier: z.number().positive().default(2),
  }).optional(),
});

export type GatewayConfig = z.infer<typeof GatewayConfigSchema>;

// Audio configuration
export const AudioConfigSchema = z.object({
  input_device: z.string().default('default'),
  output_device: z.string().default('default'),
  sample_rate: z.number().int().positive().default(16000),
  channels: z.number().int().positive().default(1),
  bit_depth: z.number().int().positive().default(16),
  vad: z.object({
    enabled: z.boolean().default(true),
    threshold: z.number().min(0).max(1).default(0.5),
    speech_pad_ms: z.number().int().positive().default(300),
    silence_duration_ms: z.number().int().positive().default(1000),
  }).default({}),
});

export type AudioConfig = z.infer<typeof AudioConfigSchema>;

// Voice mode configuration
export const VoiceModeConfigSchema = z.object({
  mode: z.enum(['push_to_talk', 'continuous', 'vad']).default('vad'),
  push_to_talk_key: z.string().default('space'),
  auto_play_response: z.boolean().default(true),
  show_transcript: z.boolean().default(true),
  beep_on_listen: z.boolean().default(false),
});

export type VoiceModeConfig = z.infer<typeof VoiceModeConfigSchema>;

// UI configuration
export const UIConfigSchema = z.object({
  theme: z.enum(['dark', 'light', 'auto']).default('dark'),
  animations: z.boolean().default(true),
  sounds: z.boolean().default(false),
  show_tips: z.boolean().default(true),
  verbose: z.boolean().default(false),
});

export type UIConfig = z.infer<typeof UIConfigSchema>;

// Default providers
export const DefaultProvidersSchema = z.object({
  stt_provider: z.string().default('deepgram'),
  tts_provider: z.string().default('elevenlabs'),
  realtime_provider: z.string().optional(),
});

export type DefaultProviders = z.infer<typeof DefaultProvidersSchema>;

// Pipeline reference
export const PipelineRefSchema = z.object({
  name: z.string(),
  file: z.string(),
  description: z.string().optional(),
});

export type PipelineRef = z.infer<typeof PipelineRefSchema>;

// Profile configuration
export const ProfileConfigSchema = z.object({
  gateway: GatewayConfigSchema.optional(),
  defaults: DefaultProvidersSchema.optional(),
  providers: z.record(ProviderConfigSchema).optional(),
});

export type ProfileConfig = z.infer<typeof ProfileConfigSchema>;

// Main configuration schema
export const WaavConfigSchema = z.object({
  version: z.number().int().positive().default(1),
  profile: z.string().default('default'),

  gateway: GatewayConfigSchema.default({}),
  defaults: DefaultProvidersSchema.default({}),
  providers: z.record(ProviderConfigSchema).default({}),

  audio: AudioConfigSchema.default({}),
  voice: VoiceModeConfigSchema.default({}),
  ui: UIConfigSchema.default({}),

  pipelines: z.record(PipelineRefSchema).default({}),
  profiles: z.record(ProfileConfigSchema).default({}),
});

export type WaavConfig = z.infer<typeof WaavConfigSchema>;

// Configuration file locations
export const CONFIG_FILE_NAME = '.waavrc.yaml';
export const CONFIG_DIR_NAME = '.waav';

// Environment variable prefixes
export const ENV_PREFIX = 'WAAV_';
export const PROVIDER_ENV_PREFIX = 'WAAV_PROVIDER_';

// Default configuration
export const DEFAULT_CONFIG: WaavConfig = WaavConfigSchema.parse({});
