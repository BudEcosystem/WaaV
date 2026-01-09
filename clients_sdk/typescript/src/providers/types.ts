/**
 * Provider types for plugin discovery.
 *
 * These types represent the provider metadata returned by the gateway's
 * plugin discovery endpoints.
 */

/**
 * Language information with human-readable name.
 */
export interface LanguageInfo {
  /** ISO 639-1 language code (e.g., 'en') */
  code: string;
  /** Human-readable name (e.g., 'English') */
  name: string;
}

/**
 * Provider usage metrics.
 */
export interface ProviderMetrics {
  /** Total number of calls */
  callCount: number;
  /** Number of errors */
  errorCount: number;
  /** Error rate (0.0 to 1.0) */
  errorRate: number;
  /** Uptime in seconds */
  uptimeSeconds: number;
}

/**
 * Health status details for a provider.
 */
export interface ProviderHealthDetails {
  /** Total number of calls */
  callCount: number;
  /** Number of errors */
  errorCount: number;
  /** Error rate (0.0 to 1.0) */
  errorRate: number;
  /** Last error message (if any) */
  lastError?: string;
  /** Uptime in seconds */
  uptimeSeconds: number;
  /** Time since last activity in seconds */
  idleSeconds: number;
}

/**
 * Provider health status response.
 */
export interface ProviderHealth {
  /** Provider identifier */
  id: string;
  /** Health status: 'healthy', 'degraded', or 'unhealthy' */
  health: 'healthy' | 'degraded' | 'unhealthy';
  /** Health details */
  details: ProviderHealthDetails;
}

/**
 * Provider type enumeration.
 */
export type ProviderType = 'stt' | 'tts' | 'realtime';

/**
 * Provider information for SDK discovery.
 *
 * Contains all metadata about a provider including its capabilities,
 * supported languages, models, and features.
 *
 * @example
 * ```typescript
 * const provider = await bud.providers.get('deepgram');
 * console.log(provider?.displayName);
 * console.log(provider?.features);
 * console.log(provider?.languages);
 * ```
 */
export interface ProviderInfo {
  /** Provider identifier (e.g., 'deepgram') */
  id: string;
  /** Human-readable name (e.g., 'Deepgram Nova-3') */
  displayName: string;
  /** Provider type: 'stt', 'tts', or 'realtime' */
  providerType: ProviderType;
  /** Brief description */
  description: string;
  /** Provider version */
  version: string;
  /** Provider features (e.g., ['streaming', 'word-timestamps']) */
  features: string[];
  /** Supported languages with human-readable names */
  languages: LanguageInfo[];
  /** Supported models */
  models: string[];
  /** Provider aliases (e.g., ['dg', 'deepgram-nova']) */
  aliases: string[];
  /** Required configuration keys */
  requiredConfig: string[];
  /** Optional configuration keys */
  optionalConfig: string[];
  /** Health status */
  health: 'healthy' | 'degraded' | 'unhealthy';
  /** Usage metrics (if available) */
  metrics?: ProviderMetrics;
}

/**
 * Audio processor information.
 */
export interface ProcessorInfo {
  /** Processor identifier */
  id: string;
  /** Display name */
  name: string;
  /** Description */
  description: string;
  /** Supported audio formats */
  supportedFormats: string[];
}

/**
 * Response from the /plugins endpoint.
 */
export interface PluginListResponse {
  /** STT providers */
  stt: ProviderInfo[];
  /** TTS providers */
  tts: ProviderInfo[];
  /** Realtime providers */
  realtime: ProviderInfo[];
  /** Audio processors */
  processors: ProcessorInfo[];
  /** Total count of all plugins */
  totalCount: number;
}

/**
 * Raw provider info response from API (snake_case).
 */
export interface RawProviderInfo {
  id: string;
  display_name: string;
  provider_type: string;
  description: string;
  version: string;
  features: string[];
  languages: { code: string; name: string }[];
  models: string[];
  aliases: string[];
  required_config: string[];
  optional_config: string[];
  health: string;
  metrics?: {
    call_count: number;
    error_count: number;
    error_rate: number;
    uptime_seconds: number;
  };
}

/**
 * Raw processor info response from API (snake_case).
 */
export interface RawProcessorInfo {
  id: string;
  name: string;
  description: string;
  supported_formats: string[];
}

/**
 * Raw plugin list response from API (snake_case).
 */
export interface RawPluginListResponse {
  stt: RawProviderInfo[];
  tts: RawProviderInfo[];
  realtime: RawProviderInfo[];
  processors: RawProcessorInfo[];
  total_count: number;
}

/**
 * Raw health response from API (snake_case).
 */
export interface RawProviderHealth {
  id: string;
  health: string;
  details: {
    call_count: number;
    error_count: number;
    error_rate: number;
    last_error?: string;
    uptime_seconds: number;
    idle_seconds: number;
  };
}

/**
 * Convert raw provider info from API to typed ProviderInfo.
 */
export function parseProviderInfo(raw: RawProviderInfo): ProviderInfo {
  return {
    id: raw.id,
    displayName: raw.display_name,
    providerType: raw.provider_type as ProviderType,
    description: raw.description,
    version: raw.version,
    features: raw.features,
    languages: raw.languages,
    models: raw.models,
    aliases: raw.aliases,
    requiredConfig: raw.required_config,
    optionalConfig: raw.optional_config,
    health: raw.health as 'healthy' | 'degraded' | 'unhealthy',
    metrics: raw.metrics
      ? {
          callCount: raw.metrics.call_count,
          errorCount: raw.metrics.error_count,
          errorRate: raw.metrics.error_rate,
          uptimeSeconds: raw.metrics.uptime_seconds,
        }
      : undefined,
  };
}

/**
 * Convert raw processor info from API to typed ProcessorInfo.
 */
export function parseProcessorInfo(raw: RawProcessorInfo): ProcessorInfo {
  return {
    id: raw.id,
    name: raw.name,
    description: raw.description,
    supportedFormats: raw.supported_formats,
  };
}

/**
 * Convert raw plugin list response from API to typed PluginListResponse.
 */
export function parsePluginListResponse(raw: RawPluginListResponse): PluginListResponse {
  return {
    stt: raw.stt.map(parseProviderInfo),
    tts: raw.tts.map(parseProviderInfo),
    realtime: raw.realtime.map(parseProviderInfo),
    processors: raw.processors.map(parseProcessorInfo),
    totalCount: raw.total_count,
  };
}

/**
 * Convert raw health response from API to typed ProviderHealth.
 */
export function parseProviderHealth(raw: RawProviderHealth): ProviderHealth {
  return {
    id: raw.id,
    health: raw.health as 'healthy' | 'degraded' | 'unhealthy',
    details: {
      callCount: raw.details.call_count,
      errorCount: raw.details.error_count,
      errorRate: raw.details.error_rate,
      lastError: raw.details.last_error,
      uptimeSeconds: raw.details.uptime_seconds,
      idleSeconds: raw.details.idle_seconds,
    },
  };
}

/**
 * Check if a provider has a specific feature.
 */
export function hasFeature(provider: ProviderInfo, feature: string): boolean {
  const featureLower = feature.toLowerCase();
  return provider.features.some((f) => f.toLowerCase() === featureLower);
}

/**
 * Check if a provider supports a specific language.
 */
export function supportsLanguage(provider: ProviderInfo, language: string): boolean {
  const languageLower = language.toLowerCase();
  return provider.languages.some(
    (lang) =>
      lang.code.toLowerCase() === languageLower ||
      lang.name.toLowerCase() === languageLower ||
      lang.name.toLowerCase().includes(languageLower)
  );
}

/**
 * Check if a provider supports a specific model.
 */
export function supportsModel(provider: ProviderInfo, model: string): boolean {
  const modelLower = model.toLowerCase();
  return provider.models.some((m) => m.toLowerCase().includes(modelLower));
}

/**
 * Check if a provider is available (healthy or degraded).
 */
export function isAvailable(provider: ProviderInfo): boolean {
  return provider.health === 'healthy' || provider.health === 'degraded';
}
