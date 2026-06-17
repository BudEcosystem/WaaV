/**
 * BudClient - Main entry point for @bud-foundry/sdk
 */

import { RestClient, type RestClientOptions } from './rest/client.js';
import { BudSTT, type BudSTTConfig } from './pipelines/stt.js';
import { BudTTS, type BudTTSConfig } from './pipelines/tts.js';
import { BudTalk, type BudTalkConfig } from './pipelines/talk.js';
import { BudTranscribe, type BudTranscribeConfig } from './pipelines/transcribe.js';
import { AgentSession, type AgentConfig } from './pipelines/agent.js';
import { WebSocketSession, type SessionConfig } from './ws/session.js';
import type { FeatureFlags, DEFAULT_FEATURE_FLAGS } from './types/features.js';
import type { MetricsSummary } from './types/metrics.js';
import { getMetricsCollector, resetMetricsCollector } from './metrics/collector.js';
import { SLOTracker } from './metrics/slo.js';
import type { SLOThreshold, SLOStatus } from './types/metrics.js';

/** Default gateway URL when neither config nor env provides one. */
export const DEFAULT_GATEWAY_URL = 'http://127.0.0.1:3009';

/** Env var the SDK reads for the gateway URL (e.g. ws://127.0.0.1:3009). */
export const GATEWAY_URL_ENV = 'WAAV_GATEWAY_URL';

/**
 * Read the gateway URL from the environment (`WAAV_GATEWAY_URL`) if available.
 * Safe in browser bundles where `process` is undefined.
 */
function readGatewayEnvUrl(): string | undefined {
  const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env;
  const value = env?.[GATEWAY_URL_ENV];
  return value && value.trim() !== '' ? value.trim() : undefined;
}

/** Normalise an ws/wss URL to http/https (REST uses http); pass others through. */
function normalizeHttpUrl(url: string): string {
  if (url.startsWith('ws://')) return 'http://' + url.slice('ws://'.length);
  if (url.startsWith('wss://')) return 'https://' + url.slice('wss://'.length);
  return url;
}

/**
 * BudClient configuration
 */
export interface BudClientConfig {
  /**
   * Base URL for the gateway REST API (e.g., "http://127.0.0.1:3009").
   * Optional: defaults to the `WAAV_GATEWAY_URL` env var when set, otherwise
   * "http://127.0.0.1:3009". An ws/wss URL is also accepted and normalised to
   * http/https for REST.
   */
  baseUrl?: string;
  /** WebSocket URL (e.g., "ws://127.0.0.1:3009/ws"). Defaults to baseUrl with /ws path */
  wsUrl?: string;
  /** API key for authentication */
  apiKey?: string;
  /** Request timeout in milliseconds (default: 30000) */
  timeout?: number;
  /** Custom fetch implementation (for Node.js compatibility) */
  fetch?: typeof fetch;
  /** Custom WebSocket implementation (for Node.js compatibility) */
  WebSocket?: typeof WebSocket;
  /** Custom headers to include in all requests */
  headers?: Record<string, string>;
  /** Default feature flags for all pipelines */
  features?: FeatureFlags;
}

/**
 * Pipeline factory configuration (extends base config with specific options)
 */
type PipelineFactoryConfig<T> = Omit<T, 'url' | 'apiKey' | 'features'>;

/**
 * BudClient - Unified client for Bud Foundry gateway
 *
 * @example
 * ```typescript
 * const bud = new BudClient({
 *   baseUrl: 'http://localhost:3001',
 *   apiKey: 'your-api-key'
 * });
 *
 * // Use STT pipeline
 * const stt = bud.stt.create({ provider: 'deepgram' });
 * await stt.connect();
 * stt.on('transcript', (t) => console.log(t.text));
 *
 * // Use TTS pipeline
 * const tts = bud.tts.create({ provider: 'elevenlabs', voice: 'rachel' });
 * await tts.speak('Hello, world!');
 *
 * // Use Talk pipeline (bidirectional)
 * const talk = bud.talk.create({
 *   stt: { provider: 'deepgram' },
 *   tts: { provider: 'elevenlabs', voice: 'rachel' }
 * });
 * await talk.connect();
 * await talk.startListening();
 * ```
 */
export class BudClient {
  private config: BudClientConfig;
  private restClient: RestClient;
  private wsUrl: string;
  /** Resolved gateway base URL (http/https). */
  private baseUrl: string;
  private sloTracker: SLOTracker;
  private activePipelines: Set<BudSTT | BudTTS | BudTalk | BudTranscribe> = new Set();

  /**
   * STT (Speech-to-Text) pipeline factory
   */
  readonly stt: {
    /** Create a new STT pipeline */
    create: (config?: PipelineFactoryConfig<BudSTTConfig>) => BudSTT;
    /** Create and connect to STT pipeline */
    connect: (config?: PipelineFactoryConfig<BudSTTConfig>) => Promise<BudSTT>;
  };

  /**
   * TTS (Text-to-Speech) pipeline factory
   */
  readonly tts: {
    /** Create a new TTS pipeline */
    create: (config?: PipelineFactoryConfig<BudTTSConfig>) => BudTTS;
    /** Create and connect to TTS pipeline */
    connect: (config?: PipelineFactoryConfig<BudTTSConfig>) => Promise<BudTTS>;
    /** One-shot synthesis using REST API */
    synthesize: (text: string, options?: {
      provider?: string;
      voice?: string;
      model?: string;
      sampleRate?: number;
      format?: string;
    }) => Promise<ArrayBuffer>;
  };

  /**
   * Talk (bidirectional voice) pipeline factory
   */
  readonly talk: {
    /** Create a new Talk pipeline */
    create: (config?: PipelineFactoryConfig<BudTalkConfig>) => BudTalk;
    /** Create and connect to Talk pipeline */
    connect: (config?: PipelineFactoryConfig<BudTalkConfig>) => Promise<BudTalk>;
  };

  /**
   * Transcribe (batch transcription) pipeline factory
   */
  readonly transcribe: {
    /** Create a new Transcribe pipeline */
    create: (config?: PipelineFactoryConfig<BudTranscribeConfig>) => BudTranscribe;
    /** Transcribe a file */
    file: (file: File | Blob, config?: PipelineFactoryConfig<BudTranscribeConfig>) => Promise<import('./pipelines/transcribe.js').TranscriptionResult>;
  };

  /**
   * REST client for direct API access
   */
  readonly rest: RestClient;

  constructor(config: BudClientConfig = {}) {
    // Resolve the gateway base URL: explicit config wins, then the
    // WAAV_GATEWAY_URL env var, then the local default. An ws/wss URL is
    // normalised to http/https so REST calls work.
    const baseUrl = normalizeHttpUrl(config.baseUrl ?? readGatewayEnvUrl() ?? DEFAULT_GATEWAY_URL);
    this.baseUrl = baseUrl;
    this.config = { ...config, baseUrl };

    // Derive WebSocket URL from base URL if not provided
    this.wsUrl = config.wsUrl ?? this.deriveWsUrl(baseUrl);

    // Initialize REST client
    this.restClient = new RestClient({
      baseUrl,
      apiKey: config.apiKey,
      timeout: config.timeout,
      fetch: config.fetch,
      headers: config.headers,
    });
    this.rest = this.restClient;

    // Initialize SLO tracker
    this.sloTracker = new SLOTracker();

    // Setup pipeline factories
    this.stt = this.createSTTFactory();
    this.tts = this.createTTSFactory();
    this.talk = this.createTalkFactory();
    this.transcribe = this.createTranscribeFactory();
  }

  /**
   * Create the flagship agent loop (STT → built-in LLM → TTS) in ~5 lines.
   *
   * This is the ONLY entry point that reaches the gateway's built-in
   * conversation loop with reasoning + barge-in + latency masking. It
   * serializes `conversation_config` and nests turn detection into
   * `stt_config.turn_detection`. Returns an UNCONNECTED {@link AgentSession};
   * subscribe to events, then `await agent.connect()`.
   *
   * @example
   * ```typescript
   * const agent = bud.agent({
   *   stt: { provider: 'deepgram', language: 'en-US' },
   *   tts: { provider: 'deepgram', voiceId: 'aura-asteria-en' },
   *   llm: {
   *     baseUrl: 'http://localhost:11434/v1',
   *     model: 'llama3.2:1b',
   *     reasoningEffort: 'minimal',
   *     latencyFiller: 'auto',
   *   },
   *   turn: { eagerEot: true },
   * });
   * agent.on('transcript', (t) => console.log('user:', t.text));
   * agent.on('audio', (a) => playback(a.audio));     // bot speech
   * agent.on('configWarning', (w) => console.warn(w.code, w.message));
   * await agent.connect();
   * agent.sendAudio(pcm);
   * ```
   */
  agent(config: AgentConfig): AgentSession {
    // Thread the client's custom WebSocket impl (Node) unless the caller set one.
    const merged: AgentConfig = { ...config };
    if (merged.WebSocket === undefined && this.config.WebSocket !== undefined) {
      merged.WebSocket = this.config.WebSocket;
    }
    return new AgentSession(merged, this.wsUrl, this.config.apiKey);
  }

  /**
   * Derive WebSocket URL from REST URL
   */
  private deriveWsUrl(baseUrl: string): string {
    const url = new URL(baseUrl);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    url.pathname = url.pathname.replace(/\/$/, '') + '/ws';
    return url.toString();
  }

  /**
   * Create STT pipeline factory
   */
  private createSTTFactory() {
    return {
      create: (config?: PipelineFactoryConfig<BudSTTConfig>): BudSTT => {
        const pipeline = new BudSTT({
          url: this.wsUrl,
          apiKey: this.config.apiKey,
          features: this.config.features,
          ...config,
        });
        this.activePipelines.add(pipeline);
        return pipeline;
      },
      connect: async (config?: PipelineFactoryConfig<BudSTTConfig>): Promise<BudSTT> => {
        const pipeline = this.stt.create(config);
        await pipeline.connect();
        return pipeline;
      },
    };
  }

  /**
   * Create TTS pipeline factory
   */
  private createTTSFactory() {
    return {
      create: (config?: PipelineFactoryConfig<BudTTSConfig>): BudTTS => {
        const pipeline = new BudTTS({
          url: this.wsUrl,
          apiKey: this.config.apiKey,
          features: this.config.features,
          restBaseUrl: this.baseUrl,
          ...config,
        });
        this.activePipelines.add(pipeline);
        return pipeline;
      },
      connect: async (config?: PipelineFactoryConfig<BudTTSConfig>): Promise<BudTTS> => {
        const pipeline = this.tts.create(config);
        await pipeline.connect();
        return pipeline;
      },
      synthesize: async (text: string, options?: {
        provider?: string;
        voice?: string;
        model?: string;
        sampleRate?: number;
        format?: string;
      }): Promise<ArrayBuffer> => {
        const result = await this.restClient.speak(text, options);
        return result.audio;
      },
    };
  }

  /**
   * Create Talk pipeline factory
   */
  private createTalkFactory() {
    return {
      create: (config?: PipelineFactoryConfig<BudTalkConfig>): BudTalk => {
        const pipeline = new BudTalk({
          url: this.wsUrl,
          apiKey: this.config.apiKey,
          features: this.config.features,
          ...config,
        });
        this.activePipelines.add(pipeline);
        return pipeline;
      },
      connect: async (config?: PipelineFactoryConfig<BudTalkConfig>): Promise<BudTalk> => {
        const pipeline = this.talk.create(config);
        await pipeline.connect();
        return pipeline;
      },
    };
  }

  /**
   * Create Transcribe pipeline factory
   */
  private createTranscribeFactory() {
    return {
      create: (config?: PipelineFactoryConfig<BudTranscribeConfig>): BudTranscribe => {
        const pipeline = new BudTranscribe({
          url: this.wsUrl,
          apiKey: this.config.apiKey,
          features: this.config.features,
          ...config,
        });
        this.activePipelines.add(pipeline);
        return pipeline;
      },
      file: async (file: File | Blob, config?: PipelineFactoryConfig<BudTranscribeConfig>) => {
        const pipeline = this.transcribe.create(config);
        try {
          return await pipeline.transcribeFile(file);
        } finally {
          await pipeline.disconnect();
          this.activePipelines.delete(pipeline);
        }
      },
    };
  }

  // REST API shortcuts

  /**
   * Check gateway health
   */
  async health(): Promise<{ status: string }> {
    return this.restClient.health();
  }

  /**
   * List available TTS voices
   */
  async listVoices(provider?: string) {
    return this.restClient.listVoices(provider);
  }

  /**
   * Generate LiveKit token
   */
  async createLiveKitToken(request: Parameters<RestClient['createLiveKitToken']>[0]) {
    return this.restClient.createLiveKitToken(request);
  }

  /**
   * List active LiveKit rooms
   */
  async listRooms() {
    return this.restClient.listRooms();
  }

  /**
   * Get LiveKit room info
   */
  async getRoomInfo(roomName: string) {
    return this.restClient.getRoomInfo(roomName);
  }

  /**
   * List SIP hooks
   */
  async listSIPHooks() {
    return this.restClient.listSIPHooks();
  }

  /**
   * Create SIP hook
   */
  async createSIPHook(request: Parameters<RestClient['createSIPHook']>[0]) {
    return this.restClient.createSIPHook(request);
  }

  /**
   * Delete SIP hook
   */
  async deleteSIPHook(host: string) {
    return this.restClient.deleteSIPHook(host);
  }

  /**
   * Get recording by stream ID
   */
  async getRecording(streamId: string) {
    return this.restClient.getRecording(streamId);
  }

  // Metrics & SLO

  /**
   * Get aggregated metrics from all active pipelines
   */
  getMetrics(): MetricsSummary {
    return getMetricsCollector().getMetrics();
  }

  /**
   * Reset all metrics
   */
  resetMetrics(): void {
    resetMetricsCollector();
  }

  /**
   * Add SLO threshold
   */
  addSLO(slo: SLOThreshold): void {
    this.sloTracker.addSLO(slo);
  }

  /**
   * Remove SLO by metric name
   */
  removeSLO(metric: string): void {
    this.sloTracker.removeSLO(metric);
  }

  /**
   * Check all SLOs against current metrics
   */
  checkSLOs(): SLOStatus[] {
    return this.sloTracker.check(this.getMetrics());
  }

  /**
   * Get SLO health (percentage of SLOs met)
   */
  getSLOHealth(): { percentage: number; met: number; total: number } {
    return this.sloTracker.getHealth(this.getMetrics());
  }

  /**
   * Get currently violated SLOs
   */
  getSLOViolations(): SLOStatus[] {
    return this.sloTracker.getViolations(this.getMetrics());
  }

  // Lifecycle

  /**
   * Get count of active pipelines
   */
  getActivePipelineCount(): number {
    return this.activePipelines.size;
  }

  /**
   * Disconnect all active pipelines
   */
  async disconnectAll(): Promise<void> {
    const promises = Array.from(this.activePipelines).map(async (pipeline) => {
      try {
        await pipeline.disconnect();
      } catch {
        // Ignore disconnect errors
      }
    });

    await Promise.all(promises);
    this.activePipelines.clear();
  }

  /**
   * Dispose client and all resources
   */
  async dispose(): Promise<void> {
    await this.disconnectAll();
    this.sloTracker.reset();
  }
}

/**
 * Create a BudClient instance
 */
export function createBudClient(config: BudClientConfig): BudClient {
  return new BudClient(config);
}
