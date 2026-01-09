/**
 * Provider discovery module for Bud Foundry SDK.
 *
 * This module provides a user-friendly API for discovering available providers
 * (STT, TTS, Realtime) from the gateway.
 *
 * @example
 * ```typescript
 * const bud = new BudClient({ baseUrl: 'http://localhost:3001' });
 *
 * // Find all STT providers
 * const sttProviders = await bud.providers.stt.all();
 *
 * // Find providers by language
 * const spanishSTT = await bud.providers.stt.withLanguage('Spanish');
 *
 * // Find providers by feature
 * const streamingSTT = await bud.providers.stt.withFeature('streaming');
 *
 * // Get specific provider info
 * const deepgram = await bud.providers.get('deepgram');
 * console.log(deepgram?.description);
 * console.log(deepgram?.features);
 * ```
 */

import type { RestClient } from '../rest/client.js';
import {
  hasFeature,
  isAvailable,
  parsePluginListResponse,
  parseProviderHealth,
  parseProviderInfo,
  supportsLanguage,
  supportsModel,
} from './types.js';
import type {
  LanguageInfo,
  PluginListResponse,
  ProcessorInfo,
  ProviderHealth,
  ProviderInfo,
  ProviderMetrics,
  ProviderType,
  RawPluginListResponse,
  RawProviderHealth,
  RawProviderInfo,
} from './types.js';

// Re-export types
export type {
  LanguageInfo,
  PluginListResponse,
  ProcessorInfo,
  ProviderHealth,
  ProviderInfo,
  ProviderMetrics,
  ProviderType,
};

// Re-export utility functions
export {
  hasFeature,
  isAvailable,
  supportsLanguage,
  supportsModel,
};

/**
 * Filter parameters for provider search.
 */
export interface ProviderFilterParams {
  /** Filter by language (ISO 639-1 code or name) */
  language?: string;
  /** Filter by feature */
  feature?: string;
  /** Filter by model */
  model?: string;
}

/**
 * Category-specific provider discovery.
 *
 * Provides fluent API for finding providers within a category (STT, TTS, Realtime).
 *
 * @example
 * ```typescript
 * // All STT providers
 * const providers = await bud.providers.stt.all();
 *
 * // Filter by language
 * const spanish = await bud.providers.stt.withLanguage('Spanish');
 *
 * // Filter by feature
 * const streaming = await bud.providers.stt.withFeature('streaming');
 * ```
 */
export class ProviderCategory {
  private category: ProviderType;
  private rest: RestClient;

  constructor(category: ProviderType, restClient: RestClient) {
    this.category = category;
    this.rest = restClient;
  }

  /**
   * Get all providers in this category.
   */
  async all(): Promise<ProviderInfo[]> {
    const response = await this.rest['request']<RawProviderInfo[]>('GET', `/plugins/${this.category}`);
    return response.map(parseProviderInfo);
  }

  /**
   * Find providers that support a specific language.
   *
   * @param language - Language code (e.g., 'en') or name (e.g., 'English', 'Spanish')
   */
  async withLanguage(language: string): Promise<ProviderInfo[]> {
    const response = await this.rest['request']<RawProviderInfo[]>(
      'GET',
      `/plugins/${this.category}?language=${encodeURIComponent(language)}`
    );
    return response.map(parseProviderInfo);
  }

  /**
   * Find providers that have a specific feature.
   *
   * @param feature - Feature name (e.g., 'streaming', 'word-timestamps')
   */
  async withFeature(feature: string): Promise<ProviderInfo[]> {
    const response = await this.rest['request']<RawProviderInfo[]>(
      'GET',
      `/plugins/${this.category}?feature=${encodeURIComponent(feature)}`
    );
    return response.map(parseProviderInfo);
  }

  /**
   * Find providers that support a specific model.
   *
   * @param model - Model name or partial match
   */
  async withModel(model: string): Promise<ProviderInfo[]> {
    const response = await this.rest['request']<RawProviderInfo[]>(
      'GET',
      `/plugins/${this.category}?model=${encodeURIComponent(model)}`
    );
    return response.map(parseProviderInfo);
  }

  /**
   * Get the recommended provider for this category.
   *
   * Returns the first healthy provider with streaming support (if applicable).
   */
  async recommended(): Promise<ProviderInfo | undefined> {
    const providers = await this.all();

    // Prefer healthy providers with streaming support
    for (const provider of providers) {
      if (provider.health === 'healthy' && hasFeature(provider, 'streaming')) {
        return provider;
      }
    }

    // Fall back to any healthy provider
    for (const provider of providers) {
      if (provider.health === 'healthy') {
        return provider;
      }
    }

    return providers[0];
  }
}

/**
 * Provider discovery registry.
 *
 * Provides a user-friendly API for discovering available providers from the gateway.
 * All complexity of REST endpoints is abstracted away.
 *
 * @example
 * ```typescript
 * // Access via BudClient
 * const bud = new BudClient({ baseUrl: 'http://localhost:3001' });
 *
 * // Get all plugins grouped by type
 * const allPlugins = await bud.providers.discover();
 * console.log(`Found ${allPlugins.totalCount} plugins`);
 *
 * // Find providers by type
 * const sttProviders = await bud.providers.stt.all();
 * const ttsProviders = await bud.providers.tts.all();
 *
 * // Find by language (human-readable)
 * const spanishSTT = await bud.providers.stt.withLanguage('Spanish');
 *
 * // Find by feature
 * const streamingSTT = await bud.providers.stt.withFeature('streaming');
 *
 * // Get specific provider info
 * const deepgram = await bud.providers.get('deepgram');
 * console.log(deepgram?.displayName);
 * console.log(deepgram?.features);
 *
 * // Check provider health
 * const health = await bud.providers.health('deepgram');
 * console.log(health?.health); // 'healthy', 'degraded', or 'unhealthy'
 * ```
 */
export class ProviderRegistry {
  private rest: RestClient;
  private _stt: ProviderCategory;
  private _tts: ProviderCategory;
  private _realtime: ProviderCategory;

  constructor(restClient: RestClient) {
    this.rest = restClient;
    this._stt = new ProviderCategory('stt', restClient);
    this._tts = new ProviderCategory('tts', restClient);
    this._realtime = new ProviderCategory('realtime', restClient);
  }

  /**
   * STT (Speech-to-Text) provider category.
   *
   * @example
   * ```typescript
   * const providers = await bud.providers.stt.all();
   * const spanish = await bud.providers.stt.withLanguage('Spanish');
   * ```
   */
  get stt(): ProviderCategory {
    return this._stt;
  }

  /**
   * TTS (Text-to-Speech) provider category.
   *
   * @example
   * ```typescript
   * const providers = await bud.providers.tts.all();
   * const withSSML = await bud.providers.tts.withFeature('ssml');
   * ```
   */
  get tts(): ProviderCategory {
    return this._tts;
  }

  /**
   * Realtime (Audio-to-Audio) provider category.
   *
   * @example
   * ```typescript
   * const providers = await bud.providers.realtime.all();
   * ```
   */
  get realtime(): ProviderCategory {
    return this._realtime;
  }

  /**
   * Discover all available plugins.
   *
   * Returns all plugins grouped by type (STT, TTS, Realtime, Processors).
   */
  async discover(): Promise<PluginListResponse> {
    const response = await this.rest['request']<RawPluginListResponse>('GET', '/plugins');
    return parsePluginListResponse(response);
  }

  /**
   * Get specific provider information by ID.
   *
   * Searches across all provider types (STT, TTS, Realtime).
   *
   * @param providerId - Provider identifier (e.g., 'deepgram', 'elevenlabs')
   */
  async get(providerId: string): Promise<ProviderInfo | undefined> {
    try {
      const response = await this.rest['request']<RawProviderInfo>('GET', `/plugins/${encodeURIComponent(providerId)}`);
      return parseProviderInfo(response);
    } catch {
      return undefined;
    }
  }

  /**
   * Get provider health status.
   *
   * @param providerId - Provider identifier
   */
  async health(providerId: string): Promise<ProviderHealth | undefined> {
    try {
      const response = await this.rest['request']<RawProviderHealth>(
        'GET',
        `/plugins/${encodeURIComponent(providerId)}/health`
      );
      return parseProviderHealth(response);
    } catch {
      return undefined;
    }
  }

  /**
   * Filter providers across all types.
   *
   * @param params - Filter parameters
   */
  async filter(params: ProviderFilterParams & { type?: ProviderType }): Promise<ProviderInfo[]> {
    let providers: ProviderInfo[];

    if (params.type) {
      // Filter within specific type
      const category = this[params.type];
      providers = await category.all();
    } else {
      // Get all providers
      const plugins = await this.discover();
      providers = [...plugins.stt, ...plugins.tts, ...plugins.realtime];
    }

    // Apply filters
    if (params.language) {
      providers = providers.filter((p) => supportsLanguage(p, params.language!));
    }
    if (params.feature) {
      providers = providers.filter((p) => hasFeature(p, params.feature!));
    }
    if (params.model) {
      providers = providers.filter((p) => supportsModel(p, params.model!));
    }

    return providers;
  }

  /**
   * Get all available audio processors.
   */
  async processors(): Promise<ProcessorInfo[]> {
    const response = await this.rest['request']<
      Array<{
        id: string;
        name: string;
        description: string;
        supported_formats: string[];
      }>
    >('GET', '/plugins/processors');

    return response.map((p) => ({
      id: p.id,
      name: p.name,
      description: p.description,
      supportedFormats: p.supported_formats,
    }));
  }
}
