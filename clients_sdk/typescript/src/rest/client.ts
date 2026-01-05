/**
 * REST Client for Bud Foundry Gateway
 */

import { APIError, TimeoutError } from '../errors/index.js';
import { getMetricsCollector } from '../metrics/collector.js';
import type { Voice, VoiceListResponse, TTSSynthesisResult } from '../types/tts.js';
import type { LiveKitTokenRequest, LiveKitTokenResponse, RoomInfo, RoomListResponse } from '../types/livekit.js';
import type { SIPHook, SIPHookListResponse, SIPHookCreateRequest, SIPHookCreateResponse } from '../types/sip.js';

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

  constructor(options: RestClientOptions) {
    // Remove trailing slash from base URL
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.apiKey = options.apiKey;
    this.timeout = options.timeout ?? 30000;
    this.fetchFn = options.fetch ?? globalThis.fetch;
    this.customHeaders = options.headers ?? {};
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
      });

      clearTimeout(timeoutId);

      const duration = Date.now() - startTime;
      this.metrics.record('ws.connect', duration); // Reusing metric for REST timing

      if (!response.ok) {
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

      if (err instanceof APIError) {
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
    const body = {
      room_name: request.roomName,
      identity: request.identity,
      name: request.name,
      ttl: request.ttl,
      metadata: request.metadata,
      room_options: request.roomOptions
        ? {
            auto_create: request.roomOptions.autoCreate,
            empty_timeout: request.roomOptions.emptyTimeout,
            max_participants: request.roomOptions.maxParticipants,
          }
        : undefined,
      permissions: request.permissions
        ? {
            can_publish: request.permissions.canPublish,
            can_subscribe: request.permissions.canSubscribe,
            can_publish_data: request.permissions.canPublishData,
            can_publish_sources: request.permissions.canPublishSources,
            hidden: request.permissions.hidden,
            recorder: request.permissions.recorder,
          }
        : undefined,
    };

    return this.request<LiveKitTokenResponse>('POST', '/livekit/token', { body });
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
}
