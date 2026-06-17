/**
 * REST Client for Bud Foundry Gateway
 */

import { APIError, TimeoutError, RateLimitError } from '../errors/index.js';
import { getMetricsCollector } from '../metrics/collector.js';
import { buildKeepAliveInit, type KeepAliveInit, type KeepAliveAgentOptions } from './keepalive-agent.js';
import { cloneVoice, cloneVoiceAndWait, getClonedVoiceStatus } from './voice.js';
import type { VoiceCloneRequest, VoiceCloneResponse } from '../types/voice.js';
import type { Voice, VoiceListResponse, TTSSynthesisResult } from '../types/tts.js';
import type { LiveKitTokenRequest, LiveKitTokenResponse, RoomInfo, RoomListResponse } from '../types/livekit.js';
import type { SIPHook, SIPHookListResponse, SIPHookCreateRequest, SIPHookCreateResponse } from '../types/sip.js';
import type { STTConfig, TranslationConfig } from '../types/config.js';

/**
 * Batch-only STT knobs that the streaming path drops but ARE batch-capable on
 * Deepgram prerecorded / AssemblyAI async / OpenAI (P5). Mirrors the gateway
 * `BatchFeatures` (gateway/src/core/stt/batch.rs).
 */
export interface BatchFeatures {
  /** Paragraph formatting (Deepgram `paragraphs` / AAI). */
  paragraphs?: boolean;
  /** Utterance segmentation (Deepgram / AAI `utterances`). */
  utterances?: boolean;
  /** Summary (Deepgram `summarize=v2` / AAI `summarization`). */
  summarize?: boolean;
  /** Topic detection (Deepgram `topics` / AAI `iab_categories`). */
  topics?: boolean;
  /** Intent detection (Deepgram `intents`; no AAI equiv → warn). */
  intents?: boolean;
  /** Language detection (Deepgram / AAI / OpenAI verbose_json). STREAMING GAP. */
  detectLanguage?: boolean;
  /** N-best alternatives (Deepgram prerecorded). STREAMING GAP. */
  alternatives?: number;
}

/** Canonical batch job status (AssemblyAI-aligned superset). */
export type BatchStatus = 'queued' | 'processing' | 'completed' | 'error';

/** Canonical, provider-agnostic batch job (gateway `BatchJob`). */
export interface BatchJob {
  /** Gateway job id (maps to provider request_id / transcript id). */
  job_id: string;
  /** `queued | processing | completed | error`. */
  status: BatchStatus;
  /** Canonical transcript + features once `completed`. */
  result?: unknown;
  /** Error message when `status === 'error'`. */
  error?: string;
}

/** Options for {@link RestClient.transcribeBatch}. */
export interface BatchTranscribeOptions {
  /** Audio URL (Deepgram `{url}` / AssemblyAI `audio_url`). */
  url?: string;
  /** Inline base64 audio bytes (mutually exclusive with `url`). */
  audioBase64?: string;
  /** MIME type for inline bytes (e.g. `audio/wav`). */
  contentType?: string;
  /** `StandardSTTConfig` reused VERBATIM (provider/language/model + features + extras). */
  config?: STTConfig;
  /** Batch-only knobs (the streaming-drop set). */
  batch?: BatchFeatures;
  /** Canonical translation block (batch enables AssemblyAI/Speechmatics/Gladia). */
  translation?: TranslationConfig;
  /** Async webhook for result delivery. */
  callbackUrl?: string;
  /** Callback verb: `POST` (default) or `PUT`. */
  callbackMethod?: string;
  /** Poll until terminal when no `callbackUrl` (default true). */
  poll?: boolean;
  /** Seconds between polls (default 2). */
  pollIntervalSeconds?: number;
  /** Max seconds to poll before throwing (default 300). */
  timeoutSeconds?: number;
}

/**
 * REST client options
 */
export interface RestClientOptions {
  /** Base URL of the gateway (e.g., "http://localhost:3001") */
  baseUrl: string;
  /** API key for authentication */
  apiKey?: string;
  /** Request timeout in milliseconds (default: 30000) */
  timeout?: number;
  /** Custom fetch implementation (for Node.js compatibility) */
  fetch?: typeof fetch;
  /** Custom headers to include in all requests */
  headers?: Record<string, string>;
  /**
   * D7 connection pooling. Node's built-in `fetch` (undici) already pools per
   * origin by default; this installs a scheme-matched `node:http(s).Agent` so
   * `node-fetch`-style consumers (which do NOT pool on their own) also reuse one
   * warm TCP/TLS connection, and lets you tune undici's pool when available.
   * `true` (default) installs it with defaults; pass an object to tune
   * sockets/idle-TTL, or `false` to disable (e.g. when you supply your own
   * pre-configured `fetch`). No-op in the browser, where `fetch` already pools.
   */
  keepAlive?: boolean | KeepAliveAgentOptions;
}

/**
 * REST Client for Bud Foundry Gateway
 */
export class RestClient {
  private baseUrl: string;
  private apiKey?: string;
  private timeout: number;
  private fetchFn: typeof fetch;
  private customHeaders: Record<string, string>;
  private metrics = getMetricsCollector();
  /**
   * D7: per-request init carrying the Node keep-alive agent (undici `dispatcher`
   * and/or node:http(s) `agent`). Built once and spread into every fetch so
   * sequential REST calls reuse one pooled TCP/TLS connection. `{}` in browsers.
   */
  private keepAliveInit: KeepAliveInit;

  constructor(options: RestClientOptions) {
    // Remove trailing slash from base URL
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.apiKey = options.apiKey;
    this.timeout = options.timeout ?? 30000;
    this.fetchFn = options.fetch ?? globalThis.fetch;
    this.customHeaders = options.headers ?? {};
    // D7: install the keep-alive agent under Node (no-op in the browser). Pass
    // the (slash-trimmed) baseUrl so the agent matches the gateway scheme — an
    // https.Agent on an http:// origin would throw ERR_INVALID_PROTOCOL.
    this.keepAliveInit =
      options.keepAlive === false
        ? {}
        : buildKeepAliveInit(this.baseUrl, typeof options.keepAlive === 'object' ? options.keepAlive : {});
  }

  /**
   * Make an authenticated request
   */
  private async request<T>(
    method: string,
    path: string,
    options?: {
      body?: unknown;
      headers?: Record<string, string>;
      timeout?: number;
    }
  ): Promise<T> {
    const url = `${this.baseUrl}${path}`;
    const timeout = options?.timeout ?? this.timeout;

    const headers: Record<string, string> = {
      ...this.customHeaders,
      ...options?.headers,
    };

    if (this.apiKey) {
      headers['Authorization'] = `Bearer ${this.apiKey}`;
    }

    if (options?.body && !headers['Content-Type']) {
      headers['Content-Type'] = 'application/json';
    }

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), timeout);

    const startTime = Date.now();

    try {
      const response = await this.fetchFn(url, {
        method,
        headers,
        body: options?.body ? JSON.stringify(options.body) : undefined,
        signal: controller.signal,
        // D7: attach the Node keep-alive agent (undici dispatcher / http(s)
        // agent). Empty object in the browser, so the spread is a no-op there.
        ...(this.keepAliveInit as RequestInit),
      });

      clearTimeout(timeoutId);

      const duration = Date.now() - startTime;
      this.metrics.record('ws.connect', duration); // Reusing metric for REST timing

      if (!response.ok) {
        // Surface the per-IP rate limit as a typed, retryable error carrying
        // the parsed Retry-After delay.
        if (response.status === 429) {
          throw RateLimitError.fromResponse(response, { url });
        }
        // D7: the gateway returns 503 "Server at capacity" when its global
        // connection cap is hit (connection_limit.rs). Like a 429 this is a
        // back-off-and-retry signal, NOT a fatal error — surface it as a
        // RateLimitError (honouring any Retry-After) so callers/retry loops
        // treat it the same.
        if (response.status === 503) {
          this.metrics.increment('rest.serverAtCapacity');
          throw RateLimitError.fromResponse(response, { url });
        }
        throw await APIError.fromResponse(response, { method });
      }

      // Handle empty responses
      const contentType = response.headers.get('content-type');
      if (contentType?.includes('application/json')) {
        return (await response.json()) as T;
      }

      // For audio/binary responses
      if (contentType?.includes('audio/') || contentType?.includes('application/octet-stream')) {
        return (await response.arrayBuffer()) as unknown as T;
      }

      // For text responses
      const text = await response.text();
      if (text === '') {
        return {} as T;
      }

      try {
        return JSON.parse(text) as T;
      } catch {
        return text as unknown as T;
      }
    } catch (err) {
      clearTimeout(timeoutId);

      // Preserve already-typed SDK errors (APIError, RateLimitError, etc.).
      if (err instanceof APIError || err instanceof RateLimitError) {
        throw err;
      }

      if (err instanceof Error) {
        if (err.name === 'AbortError') {
          throw new TimeoutError(`Request to ${path} timed out after ${timeout}ms`, timeout, {
            operation: `${method} ${path}`,
          });
        }
        throw new APIError(err.message, 0, { cause: err, url, method });
      }

      throw new APIError('Unknown error occurred', 0, { url, method });
    }
  }

  // ============================================================================
  // Health
  // ============================================================================

  /**
   * Check gateway health
   */
  async health(): Promise<{ status: string }> {
    return this.request<{ status: string }>('GET', '/');
  }

  // ============================================================================
  // Voices
  // ============================================================================

  /**
   * List available TTS voices
   */
  async listVoices(provider?: string): Promise<Voice[]> {
    const path = provider ? `/voices?provider=${encodeURIComponent(provider)}` : '/voices';
    const response = await this.request<VoiceListResponse | Voice[]>('GET', path);

    // Handle both array and wrapped response formats
    if (Array.isArray(response)) {
      return response;
    }
    return response.voices;
  }

  /**
   * Voice-cloning helpers (P4): `client.voices.clone({...})` + poll-until-ready.
   *
   * Clone a voice from samples and get back a usable TTS `voiceId`:
   * ```ts
   * const { voiceId, status } = await client.voices.clone({
   *   provider: 'elevenlabs', name: 'My Voice', audioFiles: [wavBuffer],
   * });
   * // const ready = await client.voices.cloneAndWait({ ... });
   * ```
   * `clone` POSTs the canonical gateway `VoiceCloneRequest` body; the returned
   * `voiceId` is directly usable as a TTS `voiceId` once `status === 'ready'`.
   */
  get voices() {
    const baseUrl = this.baseUrl;
    const apiKey = this.apiKey;
    return {
      /** Clone a voice (`POST /voices/clone`) → {@link VoiceCloneResponse}. */
      clone: (request: VoiceCloneRequest): Promise<VoiceCloneResponse> =>
        cloneVoice(baseUrl, request, apiKey),
      /** Clone and resolve once terminal (`ready`/`failed`); see {@link cloneVoiceAndWait}. */
      cloneAndWait: (request: VoiceCloneRequest): Promise<VoiceCloneResponse> =>
        cloneVoiceAndWait(baseUrl, request, apiKey),
      /**
       * Get a cloned voice's status. NOTE: the gateway exposes no `GET /voices/{id}`
       * status endpoint this phase, so this 404s until added — use the `status`
       * from `clone(...)` (or the provider's API) meanwhile.
       */
      getCloneStatus: (voiceId: string): Promise<VoiceCloneResponse> =>
        getClonedVoiceStatus(baseUrl, voiceId, apiKey),
    };
  }

  // ============================================================================
  // TTS
  // ============================================================================

  /**
   * Synthesize text to speech (one-shot)
   */
  async speak(
    text: string,
    options?: {
      provider?: string;
      voice?: string;
      model?: string;
      sampleRate?: number;
      format?: string;
    }
  ): Promise<TTSSynthesisResult> {
    const body = {
      text,
      provider: options?.provider,
      voice_id: options?.voice,
      model: options?.model,
      sample_rate: options?.sampleRate,
      audio_format: options?.format,
    };

    const startTime = Date.now();
    const audio = await this.request<ArrayBuffer>('POST', '/speak', { body });
    const duration = Date.now() - startTime;

    this.metrics.record('tts.ttfb', duration);
    this.metrics.increment('tts.speaks');
    this.metrics.increment('tts.characters', text.length);

    return {
      audio,
      format: options?.format ?? 'linear16',
      sampleRate: options?.sampleRate ?? 24000,
      duration: 0, // Would need to calculate from audio
      characters: text.length,
    };
  }

  // ============================================================================
  // LiveKit
  // ============================================================================

  /**
   * Generate a LiveKit participant token
   */
  async createLiveKitToken(request: LiveKitTokenRequest): Promise<LiveKitTokenResponse> {
    // Gateway TokenRequest (handlers/livekit/token.rs) accepts exactly:
    //   { room_name, participant_name, participant_identity }
    // The previous body sent `identity`/`name`, so the required
    // `participant_identity`/`participant_name` were missing and the token
    // call 422'd. Send the correct field names.
    const body = {
      room_name: request.roomName,
      participant_identity: request.participantIdentity,
      participant_name: request.participantName,
    };

    const wire = await this.request<{
      token: string;
      room_name: string;
      participant_identity: string;
      livekit_url: string;
    }>('POST', '/livekit/token', { body });

    return {
      token: wire.token,
      roomName: wire.room_name,
      participantIdentity: wire.participant_identity,
      livekitUrl: wire.livekit_url,
    };
  }

  /**
   * Get room information
   */
  async getRoomInfo(roomName: string): Promise<RoomInfo> {
    return this.request<RoomInfo>('GET', `/livekit/rooms/${encodeURIComponent(roomName)}`);
  }

  /**
   * List active rooms
   */
  async listRooms(): Promise<RoomInfo[]> {
    const response = await this.request<RoomListResponse>('GET', '/livekit/rooms');
    return response.rooms;
  }

  // ============================================================================
  // SIP
  // ============================================================================

  /**
   * List SIP webhook hooks
   */
  async listSIPHooks(): Promise<SIPHook[]> {
    const response = await this.request<SIPHookListResponse>('GET', '/sip/hooks');
    return response.hooks;
  }

  /**
   * Create or update a SIP webhook hook
   */
  async createSIPHook(request: SIPHookCreateRequest): Promise<SIPHookCreateResponse> {
    return this.request<SIPHookCreateResponse>('POST', '/sip/hooks', { body: request });
  }

  /**
   * Delete a SIP webhook hook
   */
  async deleteSIPHook(host: string): Promise<void> {
    await this.request<void>('DELETE', `/sip/hooks/${encodeURIComponent(host)}`);
  }

  // ============================================================================
  // Recording
  // ============================================================================

  /**
   * Get a recording by stream ID
   */
  async getRecording(streamId: string): Promise<Blob> {
    const buffer = await this.request<ArrayBuffer>('GET', `/recording/${encodeURIComponent(streamId)}`);
    return new Blob([buffer], { type: 'audio/wav' });
  }

  // ===========================================================================
  // Batched / Async STT (P5)
  // ===========================================================================

  /**
   * Submit a batched/async transcription job (`POST /transcribe/batch`) and, by
   * default, poll `GET /transcribe/batch/{job_id}` until terminal.
   *
   * The canonical batch envelope WRAPS the streaming {@link STTConfig} VERBATIM
   * (so the full typed `SttFeatures` — diarization/redaction/etc. — are reused)
   * plus the batch-only knobs the streaming path drops but ARE batch-capable
   * (`alternatives`/`detectLanguage`/`summarize`/`topics`/`intents`/`paragraphs`/
   * `utterances`). Supply audio as EITHER `url` OR `audioBase64` (+ optional
   * `contentType`). When `callbackUrl` is set the gateway returns
   * `{job_id, status:"queued"}` immediately and POSTs the result to the webhook.
   *
   * @returns The terminal {@link BatchJob} when polled, else the submit ack.
   */
  async transcribeBatch(options: BatchTranscribeOptions): Promise<BatchJob> {
    if ((options.url === undefined) === (options.audioBase64 === undefined)) {
      throw new Error('Provide exactly one of `url` or `audioBase64`');
    }

    const submit = await this.submitBatch(options);

    // Webhook mode or caller opted out → return the submit ack.
    if (options.callbackUrl !== undefined || options.poll === false) {
      return submit;
    }
    // Some providers run synchronously and return a terminal job immediately.
    if (submit.status === 'completed' || submit.status === 'error' || submit.result !== undefined) {
      return submit;
    }
    if (!submit.job_id) return submit;

    const intervalMs = (options.pollIntervalSeconds ?? 2) * 1000;
    const deadline = Date.now() + (options.timeoutSeconds ?? 300) * 1000;
    for (;;) {
      const job = await this.getBatchJob(submit.job_id);
      if (job.status === 'completed' || job.status === 'error') return job;
      if (Date.now() >= deadline) {
        const timeoutSec = options.timeoutSeconds ?? 300;
        throw new TimeoutError(
          `Batch job ${submit.job_id} did not finish within ${timeoutSec}s ` +
            `(last status: ${job.status})`,
          timeoutSec * 1000,
          { operation: 'transcribeBatch' },
        );
      }
      await new Promise((r) => setTimeout(r, intervalMs));
    }
  }

  /** Submit a batch job without polling (`POST /transcribe/batch`). */
  async submitBatch(options: BatchTranscribeOptions): Promise<BatchJob> {
    const body: Record<string, unknown> = {};
    if (options.url !== undefined) {
      body.url = options.url;
    } else if (options.audioBase64 !== undefined) {
      body.audio_base64 = options.audioBase64;
      if (options.contentType !== undefined) body.content_type = options.contentType;
    }

    // StandardSTTConfig is flattened onto the envelope (gateway #[serde(flatten)]).
    if (options.config) {
      Object.assign(body, sttConfigToBatchWire(options.config));
    }
    if (options.batch) body.batch = batchFeaturesToWire(options.batch);
    if (options.translation) {
      const t = options.translation;
      const tWire: Record<string, unknown> = {};
      if (t.targetLanguages && t.targetLanguages.length > 0) tWire.target_languages = t.targetLanguages;
      if (t.translateToEnglish !== undefined) tWire.translate_to_english = t.translateToEnglish;
      if (t.partials !== undefined) tWire.partials = t.partials;
      if (Object.keys(tWire).length > 0) body.translation = tWire;
    }
    if (options.callbackUrl !== undefined) body.callback_url = options.callbackUrl;
    if (options.callbackMethod !== undefined) body.callback_method = options.callbackMethod;

    return this.request<BatchJob>('POST', '/transcribe/batch', { body });
  }

  /** Poll a batch job (`GET /transcribe/batch/{job_id}`). */
  async getBatchJob(jobId: string): Promise<BatchJob> {
    return this.request<BatchJob>('GET', `/transcribe/batch/${encodeURIComponent(jobId)}`);
  }
}

/**
 * Flatten an {@link STTConfig} to the `StandardSTTConfig` wire fields the batch
 * envelope expects (provider/language/model/sample_rate/channels/encoding +
 * nested `features` + open `extras` + `translation` + `api_key`). Streaming-only
 * concerns (turn_detection) are intentionally excluded.
 */
function sttConfigToBatchWire(config: STTConfig): Record<string, unknown> {
  const wire: Record<string, unknown> = { provider: config.provider };
  const set = (k: string, v: unknown) => {
    if (v !== undefined && v !== null) wire[k] = v;
  };
  set('language', config.language);
  set('sample_rate', config.sampleRate);
  set('channels', config.channels);
  set('punctuation', config.punctuation ?? config.punctuate);
  set('encoding', config.encoding);
  set('model', config.model);
  set('api_key', config.apiKey);

  // Nested canonical SttFeatures the batch wire reuses verbatim.
  const features: Record<string, unknown> = {};
  const setF = (k: string, v: unknown) => {
    if (v !== undefined && v !== null) features[k] = v;
  };
  setF('interim_results', config.interimResults);
  setF('diarization', config.diarize);
  setF('smart_format', config.smartFormat);
  setF('profanity_filter', config.profanityFilter);
  setF('word_timestamps', config.wordTimestamps);
  setF('filler_words', config.fillerWords);
  setF('vad_events', config.vadEvents);
  setF('redaction', config.redaction);
  setF('language_detection', config.languageDetection);
  setF('entity_detection', config.entityDetection);
  setF('numerals', config.numerals);
  setF('multichannel', config.multichannel);
  setF('alternatives', config.alternatives);
  setF('sentiment', config.sentiment);
  if (config.keyterms) setF('keyterms', config.keyterms);
  else if (config.keywords) setF('keyterms', config.keywords);
  if (Object.keys(features).length > 0) wire.features = features;

  // Open passthrough.
  const extras: Record<string, unknown> = { ...(config.extras ?? {}) };
  if (config.customVocabulary !== undefined && extras.custom_vocabulary === undefined) {
    extras.custom_vocabulary = config.customVocabulary;
  }
  if (Object.keys(extras).length > 0) wire.extras = extras;

  // Translation block (P5).
  if (config.translation) {
    const t = config.translation;
    const tWire: Record<string, unknown> = {};
    if (t.targetLanguages && t.targetLanguages.length > 0) tWire.target_languages = t.targetLanguages;
    if (t.translateToEnglish !== undefined) tWire.translate_to_english = t.translateToEnglish;
    if (t.partials !== undefined) tWire.partials = t.partials;
    if (Object.keys(tWire).length > 0) wire.translation = tWire;
  }

  return wire;
}

/** Map {@link BatchFeatures} (camelCase) to the gateway snake_case wire. */
function batchFeaturesToWire(batch: BatchFeatures): Record<string, unknown> {
  const wire: Record<string, unknown> = {};
  if (batch.paragraphs !== undefined) wire.paragraphs = batch.paragraphs;
  if (batch.utterances !== undefined) wire.utterances = batch.utterances;
  if (batch.summarize !== undefined) wire.summarize = batch.summarize;
  if (batch.topics !== undefined) wire.topics = batch.topics;
  if (batch.intents !== undefined) wire.intents = batch.intents;
  if (batch.detectLanguage !== undefined) wire.detect_language = batch.detectLanguage;
  if (batch.alternatives !== undefined) wire.alternatives = batch.alternatives;
  return wire;
}
