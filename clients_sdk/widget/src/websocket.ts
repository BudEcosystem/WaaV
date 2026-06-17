/**
 * WebSocket connection handler for widget
 */

import type {
  WidgetConfig,
  TranscriptResult,
  WidgetMetrics,
  ConfigWarning,
  ConversationConfig,
} from './types';

/**
 * Build the `/ws` `config` envelope from a WidgetConfig.
 *
 * This is the single source of truth for what the widget puts on the wire and is
 * exported (pure, side-effect-free) so it can be unit-tested without a live socket.
 *
 * Wire contract (gateway handlers/ws/config.rs + messages.rs):
 *  - The envelope is FLAT: stt_config / tts_config / conversation_config (no
 *    top-level audio_features or realtime_config — those are NON-KEYS on /ws and
 *    are silently dropped by the gateway).
 *  - ML turn detection nests under `stt_config.turn_detection`
 *    ({enabled, threshold?, eager?}); silence_ms/prefix_padding_ms are not /ws keys.
 *  - The built-in LLM loop (so the bot talks back) is driven by `conversation_config`
 *    ({base_url, model, ...}); without it the gateway only does raw STT/TTS.
 *  - Realtime/S2S is a SEPARATE /realtime endpoint and is intentionally not sent here.
 */
export function buildConfigMessage(config: WidgetConfig): Record<string, unknown> {
  const message: Record<string, unknown> = {
    type: 'config',
    audio: true,
  };

  // Proxy/alias name (P3): a single server-defined name the gateway resolves to a
  // full {stt,tts,llm,dag} bundle. `<bud-widget data-alias="support-bot">` is a
  // complete agent; ops re-point what "support-bot" means with zero client change.
  // An explicit stt/tts/conversation below overrides the alias default.
  if (config.alias) {
    message.alias = config.alias;
  }

  // STT configuration (turn_detection nests HERE, per gateway config.rs:345).
  if (config.stt) {
    const sttConfig: Record<string, unknown> = {
      provider: config.stt.provider,
      language: config.stt.language || 'en-US',
      sample_rate: config.stt.sampleRate || 16000,
      channels: config.stt.channels || 1,
      encoding: config.stt.encoding || 'linear16',
      // Empty model => the gateway picks the provider's recommended default
      // (gateway config.rs: "each provider maps an empty model to its
      // recommended default"). Provider-agnostic: a hardcoded deepgram model
      // like "nova-3" would be wrong for openai/google/etc.
      model: config.stt.model ?? '',
      punctuation: config.features?.punctuation ?? true,
    };

    // ML turn detection -> stt_config.turn_detection (gateway TurnDetectionWsConfig:
    // only enabled/threshold/eager are wire keys).
    const td = config.audioFeatures?.turnDetection;
    if (td) {
      const turnDetection: Record<string, unknown> = { enabled: td.enabled };
      if (td.threshold !== undefined) {
        turnDetection.threshold = td.threshold;
      }
      if (td.eager !== undefined) {
        turnDetection.eager = td.eager;
      }
      sttConfig.turn_detection = turnDetection;
    }

    message.stt_config = sttConfig;
  }

  // TTS configuration with emotion support.
  if (config.tts) {
    const ttsConfig: Record<string, unknown> = {
      provider: config.tts.provider,
      voice_id: config.tts.voiceId || config.tts.voice,
      sample_rate: config.tts.sampleRate || 24000,
      // `model` is REQUIRED on the wire (gateway TTSWebSocketConfig has no serde
      // default). Omitting it makes the gateway HARD-REJECT the whole config
      // message ("missing field `model`") — which would silently break the bare
      // `data-tts-voice` flagship path. An empty string is the gateway-blessed
      // "use the provider's recommended default model" (gateway config.rs).
      model: config.tts.model ?? '',
    };

    // Abstract voice selection (P4): the gateway resolves a VoiceDescriptor to a
    // concrete provider voice_id server-side. Sent under `voice_descriptor` as a
    // snake_case object; a raw voice_id always wins, so the gateway only uses this
    // when voice_id is absent. Only set fields are emitted.
    const vd = config.tts.voiceDescriptor;
    if (vd) {
      const vdWire: Record<string, unknown> = {};
      if (vd.gender !== undefined) vdWire.gender = vd.gender;
      if (vd.locale !== undefined) vdWire.locale = vd.locale;
      if (vd.accent !== undefined) vdWire.accent = vd.accent;
      if (vd.age !== undefined) vdWire.age = vd.age;
      if (vd.style !== undefined) vdWire.style = vd.style;
      if (vd.nameHint !== undefined) vdWire.name_hint = vd.nameHint;
      if (Object.keys(vdWire).length > 0) ttsConfig.voice_descriptor = vdWire;
    }

    if (config.tts.emotion) {
      if (config.tts.emotion.emotion !== undefined) {
        ttsConfig.emotion = config.tts.emotion.emotion;
      }
      if (config.tts.emotion.intensity !== undefined) {
        ttsConfig.emotion_intensity = config.tts.emotion.intensity;
      }
      if (config.tts.emotion.deliveryStyle !== undefined) {
        ttsConfig.delivery_style = config.tts.emotion.deliveryStyle;
      }
      if (config.tts.emotion.description !== undefined) {
        ttsConfig.emotion_description = config.tts.emotion.description;
      }
    }

    message.tts_config = ttsConfig;
  }

  // Conversation-loop (LLM) configuration -> conversation_config. This is what
  // makes the bot TALK BACK (STT -> LLM -> TTS). base_url + model are required.
  // Every optional field maps 1:1 to a gateway conversation_config key (snake
  // case); the widget drift guard asserts this mapping stays complete.
  if (config.conversation) {
    message.conversation_config = buildConversationConfig(config.conversation);
  }

  return message;
}

/**
 * Serialize a {@link ConversationConfig} to the gateway `conversation_config`
 * envelope (camelCase -> snake_case). Pure; exported for unit tests.
 *
 * Only `base_url` + `model` are always present (gateway-required); every other
 * field is emitted only when set, so an unspecified dial takes the gateway
 * default. This is the 1:1 mirror of `ConversationWebSocketConfig`.
 */
export function buildConversationConfig(conv: ConversationConfig): Record<string, unknown> {
  const out: Record<string, unknown> = {
    base_url: conv.baseUrl,
    model: conv.model,
  };
  // Core LLM
  if (conv.systemPrompt !== undefined) out.system_prompt = conv.systemPrompt;
  if (conv.apiKey !== undefined) out.api_key = conv.apiKey;
  if (conv.temperature !== undefined) out.temperature = conv.temperature;
  if (conv.maxTokens !== undefined) out.max_tokens = conv.maxTokens;
  if (conv.streaming !== undefined) out.streaming = conv.streaming;
  if (conv.maxHistory !== undefined) out.max_history = conv.maxHistory;
  if (conv.allowInterruption !== undefined) out.allow_interruption = conv.allowInterruption;
  if (conv.providerKind !== undefined) out.provider_kind = conv.providerKind;
  if (conv.stripMarkdown !== undefined) out.strip_markdown = conv.stripMarkdown;
  // Barge-in / mute
  if (conv.eagerEot !== undefined) out.eager_eot = conv.eagerEot;
  if (conv.bargeInMinWords !== undefined) out.barge_in_min_words = conv.bargeInMinWords;
  if (conv.muteStrategy !== undefined) out.mute_strategy = conv.muteStrategy;
  if (conv.userIdleTimeoutMs !== undefined) out.user_idle_timeout_ms = conv.userIdleTimeoutMs;
  if (conv.summarizeTargetTokens !== undefined)
    out.summarize_target_tokens = conv.summarizeTargetTokens;
  // Latency filler
  if (conv.latencyFiller !== undefined) out.latency_filler = conv.latencyFiller;
  if (conv.latencyFillerAfterMs !== undefined)
    out.latency_filler_after_ms = conv.latencyFillerAfterMs;
  if (conv.latencyFillerPhrases !== undefined)
    out.latency_filler_phrases = conv.latencyFillerPhrases;
  // Reasoning tier
  if (conv.reasoningEffort !== undefined) out.reasoning_effort = conv.reasoningEffort;
  if (conv.reasoningModel !== undefined) out.reasoning_model = conv.reasoningModel;
  if (conv.reasoningBaseUrl !== undefined) out.reasoning_base_url = conv.reasoningBaseUrl;
  if (conv.reasoningApiKey !== undefined) out.reasoning_api_key = conv.reasoningApiKey;
  if (conv.reasoningProviderKind !== undefined)
    out.reasoning_provider_kind = conv.reasoningProviderKind;
  if (conv.reasoningRoute !== undefined) out.reasoning_route = conv.reasoningRoute;
  if (conv.reasoningBudgetMs !== undefined) out.reasoning_budget_ms = conv.reasoningBudgetMs;
  if (conv.degradationMessage !== undefined) out.degradation_message = conv.degradationMessage;
  if (conv.maxLlmCallsPerTurn !== undefined) out.max_llm_calls_per_turn = conv.maxLlmCallsPerTurn;
  if (conv.maxReasoningTokens !== undefined) out.max_reasoning_tokens = conv.maxReasoningTokens;
  return out;
}

export type MessageHandler = {
  onReady: (streamId: string) => void;
  onTranscript: (result: TranscriptResult) => void;
  onAudio: (audio: ArrayBuffer, format: string, sampleRate: number, isFinal: boolean) => void;
  onPlaybackComplete: () => void;
  /** Non-fatal gateway advisory (config_warning): a key was ignored / a feature no-ops. */
  onConfigWarning: (warning: ConfigWarning) => void;
  onError: (error: Error) => void;
  onClose: () => void;
};

export class WidgetWebSocket {
  private ws: WebSocket | null = null;
  private config: WidgetConfig;
  private handlers: MessageHandler;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 5;
  private reconnectDelay = 1000;
  private configSentTime: number | null = null;
  private speakStartTime: number | null = null;
  private _metrics: WidgetMetrics = {
    messagesReceived: 0,
    messagesSent: 0,
  };
  private reconnectTimeoutId: ReturnType<typeof setTimeout> | null = null;

  constructor(config: WidgetConfig, handlers: MessageHandler) {
    this.config = config;
    this.handlers = handlers;
  }

  get metrics(): WidgetMetrics {
    return { ...this._metrics };
  }

  /**
   * Reset metrics counters to prevent unbounded growth
   */
  resetMetrics(): void {
    this._metrics = {
      messagesReceived: 0,
      messagesSent: 0,
    };
  }

  connect(timeout = 10000): Promise<void> {
    return new Promise((resolve, reject) => {
      let settled = false;
      let timeoutId: ReturnType<typeof setTimeout> | null = null;

      const settle = (error?: Error) => {
        if (settled) return;
        settled = true;
        if (timeoutId !== null) {
          clearTimeout(timeoutId);
          timeoutId = null;
        }
        if (error) {
          reject(error);
        } else {
          resolve();
        }
      };

      // Set connection timeout
      timeoutId = setTimeout(() => {
        if (!settled) {
          // Close WebSocket if still connecting
          if (this.ws) {
            this.ws.close();
            this.ws = null;
          }
          settle(new Error(`Connection timeout after ${timeout}ms - no ready message received`));
        }
      }, timeout);

      try {
        const url = new URL(this.config.gatewayUrl);

        // Add auth token as query param if provided
        if (this.config.apiKey) {
          url.searchParams.set('token', this.config.apiKey);
        }

        this.ws = new WebSocket(url.toString());

        this.ws.onopen = () => {
          this.reconnectAttempts = 0;
          this.sendConfig();
          // We'll resolve when we get the ready message
        };

        this.ws.onmessage = (event) => {
          this._metrics.messagesReceived++;
          this.handleMessage(event, () => settle());
        };

        this.ws.onerror = () => {
          settle(new Error('WebSocket error'));
        };

        this.ws.onclose = () => {
          this.handlers.onClose();
          if (!settled) {
            // Only attempt reconnect if we haven't settled yet (connection dropped before ready)
            this.attemptReconnect();
          }
        };
      } catch (error) {
        settle(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  disconnect(): void {
    // Cancel any pending reconnect attempt
    if (this.reconnectTimeoutId !== null) {
      clearTimeout(this.reconnectTimeoutId);
      this.reconnectTimeoutId = null;
    }

    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }

    // Reset metrics on disconnect to prevent unbounded growth
    this.resetMetrics();
  }

  private sendConfig(): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;

    // Single source of truth for the /ws config envelope (see buildConfigMessage).
    // turn_detection nests in stt_config; conversation_config drives the LLM loop;
    // NO top-level audio_features/realtime_config (non-keys, silently dropped).
    const config = buildConfigMessage(this.config);

    this.configSentTime = performance.now();
    this.send(config);
  }

  private handleMessage(event: MessageEvent, resolveConnect?: (value: void) => void): void {
    if (event.data instanceof Blob) {
      // Binary audio data - handle async conversion with proper error handling
      // Capture speakStartTime synchronously to avoid race conditions
      const capturedSpeakStartTime = this.speakStartTime;
      if (capturedSpeakStartTime !== null) {
        this._metrics.ttsTtfb = performance.now() - capturedSpeakStartTime;
        this.speakStartTime = null;
      }

      event.data
        .arrayBuffer()
        .then((buffer) => {
          this.handlers.onAudio(buffer, 'linear16', this.config.tts?.sampleRate || 24000, false);
        })
        .catch((error) => {
          console.error('Failed to convert Blob to ArrayBuffer:', error);
          this.handlers.onError(new Error('Failed to process binary audio data'));
        });
      return;
    }

    try {
      const data = JSON.parse(event.data);
      const type = data.type;

      switch (type) {
        case 'ready':
          if (resolveConnect) {
            resolveConnect();
          }
          this.handlers.onReady(data.stream_id);
          break;

        case 'stt_result':
          if (this.configSentTime !== null) {
            this._metrics.sttTtft = performance.now() - this.configSentTime;
            this.configSentTime = null;
          }
          this.handlers.onTranscript({
            text: data.transcript || '',
            isFinal: data.is_final || false,
            confidence: data.confidence,
            speakerId: data.speaker_id,
          });
          break;

        case 'tts_audio':
          // Base64 encoded audio
          const audioData = this.base64ToArrayBuffer(data.audio);
          if (this.speakStartTime !== null) {
            this._metrics.ttsTtfb = performance.now() - this.speakStartTime;
            this.speakStartTime = null;
          }
          this.handlers.onAudio(
            audioData,
            data.format || 'linear16',
            data.sample_rate || 24000,
            data.is_final || false
          );
          break;

        case 'tts_playback_complete':
          this.handlers.onPlaybackComplete();
          break;

        case 'config_warning': {
          // Non-fatal gateway advisory (gateway handlers/ws/config_lint.rs):
          // a config key was ignored (typo / wrong nesting) or a feature no-ops.
          // Surface it so a developer learns the config was partially ignored
          // instead of debugging a silent no-op. NEVER closes the session.
          const warning: ConfigWarning = {
            code: typeof data.code === 'string' ? data.code : 'unknown',
            message: typeof data.message === 'string' ? data.message : '',
            detail:
              data.detail && typeof data.detail === 'object' ? data.detail : undefined,
          };
          // Loud-but-non-fatal: also log so it shows up in the console for the
          // common case where no listener is attached.
          console.warn(
            `[bud-widget] gateway config_warning (${warning.code}): ${warning.message}`,
            warning.detail ?? ''
          );
          this.handlers.onConfigWarning(warning);
          break;
        }

        case 'error':
          this.handlers.onError(new Error(data.message || 'Unknown error'));
          break;
      }
    } catch (e) {
      console.error('Failed to parse WebSocket message:', e);
    }
  }

  private base64ToArrayBuffer(base64: string): ArrayBuffer {
    if (!base64 || base64.length === 0) {
      return new ArrayBuffer(0);
    }

    try {
      const binaryString = atob(base64);
      const len = binaryString.length;
      const bytes = new Uint8Array(len);
      for (let i = 0; i < len; i++) {
        bytes[i] = binaryString.charCodeAt(i);
      }
      return bytes.buffer;
    } catch (error) {
      console.error('Failed to decode base64 audio data:', error);
      return new ArrayBuffer(0);
    }
  }

  private attemptReconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      this.handlers.onError(new Error('Max reconnection attempts reached'));
      return;
    }

    this.reconnectAttempts++;
    const delay = this.reconnectDelay * Math.pow(1.5, this.reconnectAttempts - 1);
    const jitter = delay * 0.2 * (Math.random() * 2 - 1);

    // Store timeout ID so it can be cancelled on disconnect
    this.reconnectTimeoutId = setTimeout(() => {
      this.reconnectTimeoutId = null;
      this.connect().catch((error) => {
        this.handlers.onError(error);
      });
    }, delay + jitter);
  }

  send(data: Record<string, unknown>): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      console.warn('WebSocket not connected');
      return;
    }

    this.ws.send(JSON.stringify(data));
    this._metrics.messagesSent++;
  }

  sendAudio(audio: Int16Array | ArrayBuffer): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      return;
    }

    if (audio instanceof Int16Array) {
      this.ws.send(audio.buffer);
    } else {
      this.ws.send(audio);
    }
  }

  speak(
    text: string,
    options?: {
      flush?: boolean;
      allowInterruption?: boolean;
      emotion?: string;
      emotionIntensity?: number | string;
      deliveryStyle?: string;
      emotionDescription?: string;
    }
  ): void {
    this.speakStartTime = performance.now();
    const message: Record<string, unknown> = {
      type: 'speak',
      text,
      flush: options?.flush ?? false,
      allow_interruption: options?.allowInterruption ?? true,
    };

    // Add emotion settings for per-message control
    if (options?.emotion !== undefined) {
      message.emotion = options.emotion;
    }
    if (options?.emotionIntensity !== undefined) {
      message.emotion_intensity = options.emotionIntensity;
    }
    if (options?.deliveryStyle !== undefined) {
      message.delivery_style = options.deliveryStyle;
    }
    if (options?.emotionDescription !== undefined) {
      message.emotion_description = options.emotionDescription;
    }

    // Use default emotion from config if not overridden
    if (
      options?.emotion === undefined &&
      options?.emotionIntensity === undefined &&
      options?.deliveryStyle === undefined &&
      this.config.tts?.emotion
    ) {
      if (this.config.tts.emotion.emotion !== undefined) {
        message.emotion = this.config.tts.emotion.emotion;
      }
      if (this.config.tts.emotion.intensity !== undefined) {
        message.emotion_intensity = this.config.tts.emotion.intensity;
      }
      if (this.config.tts.emotion.deliveryStyle !== undefined) {
        message.delivery_style = this.config.tts.emotion.deliveryStyle;
      }
      if (this.config.tts.emotion.description !== undefined) {
        message.emotion_description = this.config.tts.emotion.description;
      }
    }

    this.send(message);
  }

  clear(): void {
    this.send({ type: 'clear' });
  }

  get connected(): boolean {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN;
  }
}
