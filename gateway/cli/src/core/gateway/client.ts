/**
 * WaaV CLI Gateway REST Client
 *
 * HTTP client for communicating with the WaaV Gateway REST API
 */

import type {
  GatewayStatus,
  GatewayVoice,
  GatewayConfig,
} from '../../types/index.js';
import { GatewayError, ErrorCode, withRetry } from '../../utils/errors.js';

/**
 * Gateway client configuration
 */
export interface GatewayClientConfig {
  baseUrl: string;
  apiKey?: string;
  jwtToken?: string;
  timeout?: number;
  retryAttempts?: number;
  retryDelay?: number;
}

/**
 * Gateway REST Client
 */
export class GatewayClient {
  private baseUrl: string;
  private apiKey?: string;
  private jwtToken?: string;
  private timeout: number;
  private retryAttempts: number;
  private retryDelay: number;

  constructor(config: GatewayClientConfig) {
    this.baseUrl = config.baseUrl.replace(/\/$/, ''); // Remove trailing slash
    this.apiKey = config.apiKey;
    this.jwtToken = config.jwtToken;
    this.timeout = config.timeout ?? 30000;
    this.retryAttempts = config.retryAttempts ?? 3;
    this.retryDelay = config.retryDelay ?? 1000;
  }

  /**
   * Create client from gateway config
   */
  static fromConfig(config: GatewayConfig): GatewayClient {
    return new GatewayClient({
      baseUrl: config.url,
      apiKey: config.auth?.api_key,
      jwtToken: config.auth?.jwt_token,
      timeout: config.timeout,
      retryAttempts: config.retry?.attempts,
      retryDelay: config.retry?.delay,
    });
  }

  /**
   * Get request headers
   */
  private getHeaders(): Record<string, string> {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      'Accept': 'application/json',
      'User-Agent': 'waav-cli/1.0.0',
    };

    if (this.apiKey) {
      headers['Authorization'] = `Bearer ${this.apiKey}`;
    } else if (this.jwtToken) {
      headers['Authorization'] = `Bearer ${this.jwtToken}`;
    }

    return headers;
  }

  /**
   * Make an HTTP request to the gateway
   */
  private async request<T>(
    method: string,
    path: string,
    options: {
      body?: unknown;
      timeout?: number;
      retry?: boolean;
    } = {}
  ): Promise<T> {
    const url = `${this.baseUrl}${path}`;
    const { body, timeout = this.timeout, retry = true } = options;

    const makeRequest = async (): Promise<T> => {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), timeout);

      try {
        const response = await fetch(url, {
          method,
          headers: this.getHeaders(),
          body: body ? JSON.stringify(body) : undefined,
          signal: controller.signal,
        });

        clearTimeout(timeoutId);

        if (!response.ok) {
          const errorBody = await response.text();
          let errorMessage = `HTTP ${response.status}: ${response.statusText}`;

          try {
            const errorJson = JSON.parse(errorBody);
            errorMessage = errorJson.message || errorJson.error || errorMessage;
          } catch {
            // Use default error message
          }

          throw new GatewayError(errorMessage, {
            code: response.status === 401
              ? ErrorCode.GATEWAY_AUTH_FAILED
              : ErrorCode.GATEWAY_ERROR,
            details: { status: response.status, url },
            recoverable: response.status >= 500,
          });
        }

        // Handle empty responses
        const text = await response.text();
        if (!text) {
          return {} as T;
        }

        return JSON.parse(text) as T;
      } catch (error) {
        clearTimeout(timeoutId);

        if (error instanceof GatewayError) {
          throw error;
        }

        if (error instanceof Error) {
          if (error.name === 'AbortError') {
            throw new GatewayError(`Request timeout after ${timeout}ms`, {
              code: ErrorCode.GATEWAY_TIMEOUT,
              recoverable: true,
            });
          }

          if (error.message.includes('fetch failed') || error.message.includes('ECONNREFUSED')) {
            throw new GatewayError('Cannot connect to gateway', {
              code: ErrorCode.GATEWAY_UNREACHABLE,
              suggestion: `Ensure the gateway is running at ${this.baseUrl}`,
              recoverable: true,
            });
          }
        }

        throw new GatewayError(`Request failed: ${error instanceof Error ? error.message : String(error)}`, {
          code: ErrorCode.GATEWAY_ERROR,
          cause: error instanceof Error ? error : undefined,
        });
      }
    };

    if (retry) {
      return withRetry(makeRequest, {
        maxAttempts: this.retryAttempts,
        delay: this.retryDelay,
        shouldRetry: (e) => e instanceof GatewayError && e.recoverable,
      });
    }

    return makeRequest();
  }

  /**
   * Check if the gateway is reachable
   */
  async ping(): Promise<boolean> {
    try {
      await this.request<unknown>('GET', '/', { timeout: 5000, retry: false });
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Get gateway health/status
   */
  async getStatus(): Promise<GatewayStatus> {
    const response = await this.request<GatewayStatus>('GET', '/');

    // Normalize the response
    return {
      healthy: true,
      version: response.version || '1.0.0',
      uptime_seconds: response.uptime_seconds || 0,
      connections: response.connections || 0,
      providers: response.providers || { stt: [], tts: [], realtime: [] },
      features: response.features || [],
      livekit_enabled: response.livekit_enabled || false,
    };
  }

  /**
   * List available voices from the gateway
   */
  async listVoices(options?: {
    provider?: string;
    language?: string;
    gender?: string;
    limit?: number;
    offset?: number;
  }): Promise<GatewayVoice[]> {
    const params = new URLSearchParams();

    if (options?.provider) {
      params.set('provider', options.provider);
    }
    if (options?.language) {
      params.set('language', options.language);
    }
    if (options?.gender) {
      params.set('gender', options.gender);
    }
    if (options?.limit) {
      params.set('limit', String(options.limit));
    }
    if (options?.offset) {
      params.set('offset', String(options.offset));
    }

    const queryString = params.toString();
    const path = queryString ? `/voices?${queryString}` : '/voices';

    const response = await this.request<{ voices: GatewayVoice[] } | GatewayVoice[]>('GET', path);

    // Handle both array and object response formats
    if (Array.isArray(response)) {
      return response;
    }

    return response.voices || [];
  }

  /**
   * Generate speech from text (TTS)
   */
  async speak(options: {
    text: string;
    provider?: string;
    voice_id?: string;
    model?: string;
    speed?: number;
    format?: 'mp3' | 'wav' | 'pcm' | 'opus';
  }): Promise<ArrayBuffer> {
    const url = `${this.baseUrl}/speak`;

    const response = await fetch(url, {
      method: 'POST',
      headers: {
        ...this.getHeaders(),
        'Accept': 'audio/*',
      },
      body: JSON.stringify(options),
    });

    if (!response.ok) {
      const errorText = await response.text();
      throw new GatewayError(`TTS failed: ${errorText}`, {
        code: ErrorCode.GATEWAY_ERROR,
        details: { status: response.status },
      });
    }

    return response.arrayBuffer();
  }

  /**
   * Get LiveKit room token
   */
  async getLiveKitToken(options: {
    room_name: string;
    participant_name: string;
    can_publish?: boolean;
    can_subscribe?: boolean;
  }): Promise<{ token: string; url: string }> {
    return this.request<{ token: string; url: string }>('POST', '/livekit/token', {
      body: options,
    });
  }

  /**
   * Download a recording
   */
  async getRecording(streamId: string): Promise<ArrayBuffer> {
    const url = `${this.baseUrl}/recording/${streamId}`;

    const response = await fetch(url, {
      method: 'GET',
      headers: this.getHeaders(),
    });

    if (!response.ok) {
      throw new GatewayError(`Failed to download recording: ${response.statusText}`, {
        code: ErrorCode.GATEWAY_ERROR,
        details: { status: response.status, streamId },
      });
    }

    return response.arrayBuffer();
  }

  /**
   * Test provider connectivity
   */
  async testProvider(category: 'stt' | 'tts', provider: string): Promise<{
    success: boolean;
    latency_ms?: number;
    error?: string;
  }> {
    try {
      if (category === 'tts') {
        // Test TTS by generating a short phrase
        const start = Date.now();
        await this.speak({
          text: 'Test.',
          provider,
          format: 'pcm',
        });
        const latency_ms = Date.now() - start;
        return { success: true, latency_ms };
      } else {
        // For STT, we'd need to send audio - just check if provider is available
        const status = await this.getStatus();
        const available = status.providers.stt.includes(provider);
        return {
          success: available,
          error: available ? undefined : `Provider ${provider} not available`,
        };
      }
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }

  /**
   * Get WebSocket URL for streaming
   */
  getWebSocketUrl(): string {
    const wsProtocol = this.baseUrl.startsWith('https') ? 'wss' : 'ws';
    const host = this.baseUrl.replace(/^https?:\/\//, '');
    return `${wsProtocol}://${host}/ws`;
  }

  /**
   * Update configuration
   */
  updateConfig(config: Partial<GatewayClientConfig>): void {
    if (config.baseUrl) {
      this.baseUrl = config.baseUrl.replace(/\/$/, '');
    }
    if (config.apiKey !== undefined) {
      this.apiKey = config.apiKey;
    }
    if (config.jwtToken !== undefined) {
      this.jwtToken = config.jwtToken;
    }
    if (config.timeout !== undefined) {
      this.timeout = config.timeout;
    }
    if (config.retryAttempts !== undefined) {
      this.retryAttempts = config.retryAttempts;
    }
    if (config.retryDelay !== undefined) {
      this.retryDelay = config.retryDelay;
    }
  }

  /**
   * Get current base URL
   */
  getBaseUrl(): string {
    return this.baseUrl;
  }
}
