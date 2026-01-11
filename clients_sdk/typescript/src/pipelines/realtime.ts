// =============================================================================
// BudRealtime Pipeline
// Real-time bidirectional audio with LLM integration
// =============================================================================

import { EventEmitter } from 'events';

// =============================================================================
// Types
// =============================================================================

/**
 * Supported realtime providers.
 */
export type RealtimeProvider = 'openai-realtime' | 'hume-evi';

/**
 * Connection state.
 */
export type RealtimeState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'error';

/**
 * Configuration for BudRealtime.
 */
export interface RealtimeConfig {
  /** Realtime provider to use */
  provider: RealtimeProvider;
  /** API key for the provider */
  apiKey: string;
  /** Model to use (OpenAI) */
  model?: string;
  /** EVI version (Hume) */
  eviVersion?: string;
  /** Voice ID for TTS */
  voiceId?: string;
  /** System prompt */
  systemPrompt?: string;
  /** Enable verbose transcription (Hume) */
  verboseTranscription?: boolean;
  /** Resume from previous chat group (Hume) */
  resumedChatGroupId?: string;
  /** Temperature for LLM */
  temperature?: number;
  /** Maximum response tokens */
  maxTokens?: number;
  /** Turn detection settings */
  turnDetection?: {
    enabled: boolean;
    threshold?: number;
    silenceMs?: number;
    prefixPaddingMs?: number;
    createResponseMs?: number;
  };
}

/**
 * Tool/function definition for LLM.
 */
export interface ToolDefinition {
  /** Type of tool (always 'function') */
  type: 'function';
  /** Function name */
  name: string;
  /** Function description */
  description: string;
  /** JSON Schema for parameters */
  parameters: {
    type: 'object';
    properties: Record<string, unknown>;
    required?: string[];
  };
}

/**
 * Function call event from LLM.
 */
export interface FunctionCallEvent {
  /** Function name */
  name: string;
  /** Parsed arguments */
  arguments: Record<string, unknown>;
  /** Call ID for submitting result */
  callId: string;
}

/**
 * Transcript event.
 */
export interface TranscriptEvent {
  /** Transcribed text */
  text: string;
  /** Whether this is the final transcript */
  isFinal: boolean;
  /** Speaker role */
  role?: 'user' | 'assistant';
}

/**
 * Audio event.
 */
export interface AudioEvent {
  /** Raw audio data */
  audio: ArrayBuffer;
  /** Sample rate */
  sampleRate?: number;
}

/**
 * Emotion event (Hume EVI).
 */
export interface EmotionEvent {
  /** Emotion scores */
  emotions: Record<string, number>;
  /** Dominant emotion */
  dominant: string;
  /** Confidence score */
  confidence?: number;
}

/**
 * State change event.
 */
export interface StateChangeEvent {
  /** Previous state */
  previousState: RealtimeState;
  /** Current state */
  currentState: RealtimeState;
}

/**
 * Realtime events interface.
 */
export interface RealtimeEvents {
  audio: (event: AudioEvent) => void;
  transcript: (event: TranscriptEvent) => void;
  functionCall: (event: FunctionCallEvent) => void;
  emotion: (event: EmotionEvent) => void;
  connected: () => void;
  disconnected: () => void;
  stateChange: (event: StateChangeEvent) => void;
  error: (error: Error) => void;
}

// =============================================================================
// Default Configuration
// =============================================================================

const DEFAULT_OPENAI_MODEL = 'gpt-4o-realtime-preview';
const DEFAULT_EVI_VERSION = '3';

// =============================================================================
// BudRealtime Class
// =============================================================================

/**
 * Real-time bidirectional audio pipeline with LLM integration.
 * Supports OpenAI Realtime API and Hume EVI.
 */
export class BudRealtime extends EventEmitter {
  private ws: WebSocket | null = null;
  private _state: RealtimeState = 'disconnected';
  private _tools: ToolDefinition[] = [];
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 3;
  private reconnectDelay = 1000;
  private _lastUrl: string | null = null;
  private _reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  public readonly config: Required<
    Pick<RealtimeConfig, 'provider' | 'apiKey'> &
      Partial<Omit<RealtimeConfig, 'provider' | 'apiKey'>>
  >;

  /**
   * Create a new BudRealtime instance.
   *
   * @param config - Realtime configuration
   * @throws Error if provider is invalid or missing
   */
  constructor(config: RealtimeConfig) {
    super();

    // Validate required fields
    if (!config.provider) {
      throw new Error('Provider is required');
    }

    if (!['openai-realtime', 'hume-evi'].includes(config.provider)) {
      throw new Error(`Invalid provider: ${config.provider}`);
    }

    // Apply defaults based on provider
    this.config = {
      ...config,
      model: config.model || (config.provider === 'openai-realtime' ? DEFAULT_OPENAI_MODEL : undefined),
      eviVersion: config.eviVersion || (config.provider === 'hume-evi' ? DEFAULT_EVI_VERSION : undefined),
    };
  }

  /**
   * Get the current provider.
   */
  get provider(): RealtimeProvider {
    return this.config.provider;
  }

  /**
   * Get the current connection state.
   */
  get state(): RealtimeState {
    return this._state;
  }

  /**
   * Get registered tools.
   */
  get tools(): ToolDefinition[] {
    return [...this._tools];
  }

  /**
   * Get listener count for an event.
   */
  listenerCount(event: string): number {
    return super.listenerCount(event);
  }

  /**
   * Connect to the realtime gateway.
   *
   * @param url - WebSocket URL to connect to
   * @returns Promise that resolves when connected
   */
  async connect(url: string): Promise<void> {
    if (this._state === 'connected' || this._state === 'connecting') {
      return;
    }

    // Store URL for reconnection
    this._lastUrl = url;
    this.setState('connecting');

    return new Promise((resolve, reject) => {
      try {
        this.ws = new WebSocket(url);

        const onOpen = () => {
          this.setState('connected');
          this.reconnectAttempts = 0;
          this.emit('connected');

          // Send initial session config
          this.sendSessionConfig();

          resolve();
        };

        const onClose = (event: CloseEvent) => {
          this.cleanupHandlers();
          this.handleClose(event);
        };

        const onError = (error: Event) => {
          this.emit('error', error instanceof Error ? error : new Error('WebSocket error'));
          if (this._state === 'connecting') {
            this.cleanupHandlers();
            reject(error);
          }
        };

        const onMessage = (event: MessageEvent) => {
          this.handleMessage(event);
        };

        this.ws.onopen = onOpen;
        this.ws.onclose = onClose;
        this.ws.onerror = onError;
        this.ws.onmessage = onMessage;
      } catch (error) {
        this.setState('error');
        reject(error);
      }
    });
  }

  /**
   * Clean up WebSocket event handlers.
   */
  private cleanupHandlers(): void {
    if (this.ws) {
      this.ws.onopen = null;
      this.ws.onclose = null;
      this.ws.onerror = null;
      this.ws.onmessage = null;
    }
  }

  /**
   * Disconnect from the gateway.
   */
  async disconnect(): Promise<void> {
    // Cancel any pending reconnect
    if (this._reconnectTimer) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = null;
    }

    // Clean up WebSocket
    if (this.ws) {
      this.cleanupHandlers();
      this.ws.close(1000, 'Normal closure');
      this.ws = null;
    }

    this._lastUrl = null;
    this.reconnectAttempts = 0;
    this.setState('disconnected');
    this.emit('disconnected');
  }

  /**
   * Send audio data to the gateway.
   *
   * @param audio - Raw audio data (PCM)
   */
  sendAudio(audio: ArrayBuffer): void {
    if (this._state !== 'connected' || !this.ws) {
      throw new Error('Not connected');
    }

    // For OpenAI Realtime, wrap in message format
    if (this.config.provider === 'openai-realtime') {
      const base64Audio = this.arrayBufferToBase64(audio);
      this.ws.send(
        JSON.stringify({
          type: 'input_audio_buffer.append',
          audio: base64Audio,
        })
      );
    } else {
      // For Hume EVI, send raw binary
      this.ws.send(audio);
    }
  }

  /**
   * Send text message to the LLM.
   *
   * @param text - Text message to send
   */
  sendText(text: string): void {
    if (this._state !== 'connected' || !this.ws) {
      throw new Error('Not connected');
    }

    if (this.config.provider === 'openai-realtime') {
      this.ws.send(
        JSON.stringify({
          type: 'conversation.item.create',
          item: {
            type: 'message',
            role: 'user',
            content: [
              {
                type: 'input_text',
                text,
              },
            ],
          },
        })
      );

      // Trigger response
      this.ws.send(
        JSON.stringify({
          type: 'response.create',
        })
      );
    } else {
      // Hume EVI text message
      this.ws.send(
        JSON.stringify({
          type: 'user_message',
          text,
        })
      );
    }
  }

  /**
   * Add a tool/function for the LLM to use.
   *
   * @param tool - Tool definition
   */
  async addTool(tool: ToolDefinition): Promise<void> {
    this._tools.push(tool);

    // If connected, update session with new tool
    if (this._state === 'connected' && this.ws) {
      this.sendSessionConfig();
    }
  }

  /**
   * Remove a tool by name.
   *
   * @param name - Tool name to remove
   */
  removeTool(name: string): void {
    this._tools = this._tools.filter((t) => t.name !== name);

    if (this._state === 'connected' && this.ws) {
      this.sendSessionConfig();
    }
  }

  /**
   * Submit function call result to the LLM.
   *
   * @param callId - Call ID from the function call event
   * @param result - Result to return to the LLM
   */
  submitFunctionResult(callId: string, result: unknown): void {
    if (this._state !== 'connected' || !this.ws) {
      throw new Error('Not connected');
    }

    if (this.config.provider === 'openai-realtime') {
      this.ws.send(
        JSON.stringify({
          type: 'conversation.item.create',
          item: {
            type: 'function_call_output',
            call_id: callId,
            output: JSON.stringify(result),
          },
        })
      );

      // Trigger response
      this.ws.send(
        JSON.stringify({
          type: 'response.create',
        })
      );
    } else {
      // Hume EVI tool result
      this.ws.send(
        JSON.stringify({
          type: 'tool_response',
          tool_call_id: callId,
          content: JSON.stringify(result),
        })
      );
    }
  }

  /**
   * Interrupt/cancel the current response.
   */
  interrupt(): void {
    if (this._state !== 'connected' || !this.ws) {
      return;
    }

    if (this.config.provider === 'openai-realtime') {
      this.ws.send(
        JSON.stringify({
          type: 'response.cancel',
        })
      );
    } else {
      // Hume EVI interrupt
      this.ws.send(
        JSON.stringify({
          type: 'user_interruption',
        })
      );
    }
  }

  /**
   * Commit the audio buffer (OpenAI Realtime).
   */
  commitAudioBuffer(): void {
    if (this._state !== 'connected' || !this.ws) {
      return;
    }

    if (this.config.provider === 'openai-realtime') {
      this.ws.send(
        JSON.stringify({
          type: 'input_audio_buffer.commit',
        })
      );
    }
  }

  // ===========================================================================
  // Private Methods
  // ===========================================================================

  private setState(state: RealtimeState): void {
    const previousState = this._state;
    this._state = state;
    this.emit('stateChange', { previousState, currentState: state });
  }

  private sendSessionConfig(): void {
    if (!this.ws || this._state !== 'connected') return;

    if (this.config.provider === 'openai-realtime') {
      const sessionConfig: Record<string, unknown> = {
        type: 'session.update',
        session: {
          modalities: ['text', 'audio'],
          instructions: this.config.systemPrompt,
          voice: this.config.voiceId || 'alloy',
          input_audio_format: 'pcm16',
          output_audio_format: 'pcm16',
          tools: this._tools.map((t) => ({
            type: 'function',
            name: t.name,
            description: t.description,
            parameters: t.parameters,
          })),
          tool_choice: this._tools.length > 0 ? 'auto' : 'none',
        },
      };

      if (this.config.turnDetection?.enabled) {
        (sessionConfig.session as Record<string, unknown>).turn_detection = {
          type: 'server_vad',
          threshold: this.config.turnDetection.threshold,
          silence_duration_ms: this.config.turnDetection.silenceMs,
          prefix_padding_ms: this.config.turnDetection.prefixPaddingMs,
        };
      }

      if (this.config.temperature !== undefined) {
        (sessionConfig.session as Record<string, unknown>).temperature = this.config.temperature;
      }

      if (this.config.maxTokens !== undefined) {
        (sessionConfig.session as Record<string, unknown>).max_response_output_tokens = this.config.maxTokens;
      }

      this.ws.send(JSON.stringify(sessionConfig));
    } else {
      // Hume EVI session setup
      const sessionConfig: Record<string, unknown> = {
        type: 'session_settings',
        system_prompt: this.config.systemPrompt,
        evi_version: this.config.eviVersion,
        verbose_transcription: this.config.verboseTranscription,
      };

      if (this.config.resumedChatGroupId) {
        sessionConfig.resumed_chat_group_id = this.config.resumedChatGroupId;
      }

      if (this.config.voiceId) {
        sessionConfig.voice_id = this.config.voiceId;
      }

      if (this._tools.length > 0) {
        sessionConfig.tools = this._tools.map((t) => ({
          type: 'function',
          name: t.name,
          description: t.description,
          parameters: t.parameters,
        }));
      }

      this.ws.send(JSON.stringify(sessionConfig));
    }
  }

  private handleMessage(event: MessageEvent): void {
    // Handle binary audio data
    if (event.data instanceof ArrayBuffer) {
      this.emit('audio', { audio: event.data });
      return;
    }

    // Handle JSON messages
    try {
      const message = JSON.parse(event.data as string);
      this.routeMessage(message);
    } catch (error) {
      console.error('Failed to parse message:', error);
    }
  }

  private routeMessage(message: Record<string, unknown>): void {
    const type = message.type as string;

    if (this.config.provider === 'openai-realtime') {
      this.handleOpenAIMessage(type, message);
    } else {
      this.handleHumeMessage(type, message);
    }
  }

  private handleOpenAIMessage(type: string, message: Record<string, unknown>): void {
    switch (type) {
      case 'response.audio.delta': {
        const base64Audio = message.delta as string;
        const audio = this.base64ToArrayBuffer(base64Audio);
        this.emit('audio', { audio });
        break;
      }

      case 'response.audio_transcript.delta': {
        this.emit('transcript', {
          text: message.delta as string,
          isFinal: false,
          role: 'assistant',
        });
        break;
      }

      case 'response.audio_transcript.done': {
        this.emit('transcript', {
          text: message.transcript as string,
          isFinal: true,
          role: 'assistant',
        });
        break;
      }

      case 'conversation.item.input_audio_transcription.completed': {
        this.emit('transcript', {
          text: message.transcript as string,
          isFinal: true,
          role: 'user',
        });
        break;
      }

      case 'response.function_call_arguments.done': {
        this.emit('functionCall', {
          name: message.name as string,
          arguments: JSON.parse(message.arguments as string),
          callId: message.call_id as string,
        });
        break;
      }

      case 'error': {
        const errorInfo = message.error as Record<string, unknown>;
        this.emit('error', new Error(errorInfo.message as string));
        break;
      }
    }
  }

  private handleHumeMessage(type: string, message: Record<string, unknown>): void {
    switch (type) {
      case 'audio': {
        const base64Audio = message.data as string;
        const audio = this.base64ToArrayBuffer(base64Audio);
        this.emit('audio', { audio });
        break;
      }

      case 'user_message':
      case 'assistant_message': {
        const role = type === 'user_message' ? 'user' : 'assistant';
        const content = message.message as Record<string, unknown>;

        this.emit('transcript', {
          text: content.content as string,
          isFinal: true,
          role,
        });

        // Handle emotions from Hume
        const models = message.models as Record<string, unknown> | undefined;
        if (models?.prosody) {
          const prosody = models.prosody as Record<string, unknown>;
          const scores = prosody.scores as Record<string, number>;

          // Find dominant emotion
          let dominant = 'neutral';
          let maxScore = 0;
          for (const [emotion, score] of Object.entries(scores)) {
            if (score > maxScore) {
              maxScore = score;
              dominant = emotion;
            }
          }

          this.emit('emotion', {
            emotions: scores,
            dominant,
            confidence: maxScore,
          });
        }
        break;
      }

      case 'tool_call': {
        this.emit('functionCall', {
          name: message.name as string,
          arguments: JSON.parse(message.parameters as string),
          callId: message.tool_call_id as string,
        });
        break;
      }

      case 'error': {
        this.emit('error', new Error(message.message as string));
        break;
      }
    }
  }

  private handleClose(event: CloseEvent): void {
    this.ws = null;

    if (event.code !== 1000 && this._lastUrl) {
      // Abnormal closure, attempt reconnect
      if (this.reconnectAttempts < this.maxReconnectAttempts) {
        this.setState('reconnecting');
        this.reconnectAttempts++;
        const delay = this.reconnectDelay * this.reconnectAttempts;

        this._reconnectTimer = setTimeout(() => {
          this._reconnectTimer = null;
          if (this._lastUrl && this._state === 'reconnecting') {
            this.connect(this._lastUrl).catch((error) => {
              this.emit('error', error instanceof Error ? error : new Error('Reconnection failed'));
              if (this.reconnectAttempts >= this.maxReconnectAttempts) {
                this.setState('error');
              }
            });
          }
        }, delay);
      } else {
        this.setState('error');
        this.emit('disconnected');
      }
    } else {
      this.setState('disconnected');
      this.emit('disconnected');
    }
  }

  private arrayBufferToBase64(buffer: ArrayBuffer): string {
    const bytes = new Uint8Array(buffer);
    // Use smaller chunks to avoid stack overflow with spread operator
    // 8KB is safe for all JS engines (stack limit is typically 65536 args)
    const chunkSize = 8192;
    const chunks: string[] = [];

    for (let i = 0; i < bytes.length; i += chunkSize) {
      const chunk = bytes.subarray(i, Math.min(i + chunkSize, bytes.length));
      // Use spread operator which is safer than apply() for large arrays
      chunks.push(String.fromCharCode(...chunk));
    }

    return btoa(chunks.join(''));
  }

  private base64ToArrayBuffer(base64: string): ArrayBuffer {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
      bytes[i] = binary.charCodeAt(i);
    }
    return bytes.buffer;
  }
}

// Types and class are exported inline above
