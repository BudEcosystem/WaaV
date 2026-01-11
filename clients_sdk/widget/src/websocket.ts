/**
 * WebSocket connection handler for widget
 */

import type { WidgetConfig, TranscriptResult, WidgetMetrics } from './types';

export type MessageHandler = {
  onReady: (streamId: string) => void;
  onTranscript: (result: TranscriptResult) => void;
  onAudio: (audio: ArrayBuffer, format: string, sampleRate: number, isFinal: boolean) => void;
  onPlaybackComplete: () => void;
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

    const config: Record<string, unknown> = {
      type: 'config',
      audio: true,
    };

    // STT configuration
    if (this.config.stt) {
      config.stt_config = {
        provider: this.config.stt.provider,
        language: this.config.stt.language || 'en-US',
        sample_rate: this.config.stt.sampleRate || 16000,
        channels: this.config.stt.channels || 1,
        encoding: this.config.stt.encoding || 'linear16',
        model: this.config.stt.model || 'nova-3',
        punctuation: this.config.features?.punctuation ?? true,
      };
    }

    // TTS configuration with emotion support
    if (this.config.tts) {
      const ttsConfig: Record<string, unknown> = {
        provider: this.config.tts.provider,
        voice_id: this.config.tts.voiceId || this.config.tts.voice,
        sample_rate: this.config.tts.sampleRate || 24000,
        model: this.config.tts.model,
      };

      // Add emotion settings if configured
      if (this.config.tts.emotion) {
        if (this.config.tts.emotion.emotion !== undefined) {
          ttsConfig.emotion = this.config.tts.emotion.emotion;
        }
        if (this.config.tts.emotion.intensity !== undefined) {
          ttsConfig.emotion_intensity = this.config.tts.emotion.intensity;
        }
        if (this.config.tts.emotion.deliveryStyle !== undefined) {
          ttsConfig.delivery_style = this.config.tts.emotion.deliveryStyle;
        }
        if (this.config.tts.emotion.description !== undefined) {
          ttsConfig.emotion_description = this.config.tts.emotion.description;
        }
      }

      config.tts_config = ttsConfig;
    }

    // Audio features configuration
    if (this.config.audioFeatures) {
      const audioFeatures: Record<string, unknown> = {};

      // Turn detection
      if (this.config.audioFeatures.turnDetection) {
        audioFeatures.turn_detection = {
          enabled: this.config.audioFeatures.turnDetection.enabled,
          threshold: this.config.audioFeatures.turnDetection.threshold,
          silence_ms: this.config.audioFeatures.turnDetection.silenceMs,
          prefix_padding_ms: this.config.audioFeatures.turnDetection.prefixPaddingMs,
        };
      }

      // Noise filtering (matches Python SDK naming)
      if (this.config.audioFeatures.noiseFilter) {
        audioFeatures.noise_filtering = {
          enabled: this.config.audioFeatures.noiseFilter.enabled,
          strength: this.config.audioFeatures.noiseFilter.strength,
        };
      }

      // VAD (Voice Activity Detection)
      if (this.config.audioFeatures.vad) {
        audioFeatures.vad = {
          enabled: this.config.audioFeatures.vad.enabled,
          threshold: this.config.audioFeatures.vad.threshold,
          silence_ms: this.config.audioFeatures.vad.silenceMs,
        };
      }

      if (Object.keys(audioFeatures).length > 0) {
        config.audio_features = audioFeatures;
      }
    }

    // Realtime configuration (OpenAI Realtime API / Hume EVI)
    if (this.config.realtime) {
      const realtimeConfig: Record<string, unknown> = {
        provider: this.config.realtime.provider,
        model: this.config.realtime.model,
        system_prompt: this.config.realtime.systemPrompt,
        voice_id: this.config.realtime.voiceId,
        temperature: this.config.realtime.temperature,
        max_tokens: this.config.realtime.maxTokens,
      };

      // Hume EVI specific fields
      if (this.config.realtime.eviVersion !== undefined) {
        realtimeConfig.evi_version = this.config.realtime.eviVersion;
      }
      if (this.config.realtime.verboseTranscription !== undefined) {
        realtimeConfig.verbose_transcription = this.config.realtime.verboseTranscription;
      }
      if (this.config.realtime.resumedChatGroupId !== undefined) {
        realtimeConfig.resumed_chat_group_id = this.config.realtime.resumedChatGroupId;
      }

      // OpenAI Realtime specific fields
      if (this.config.realtime.inputAudioTranscription !== undefined) {
        realtimeConfig.input_audio_transcription = {
          model: this.config.realtime.inputAudioTranscription.model,
        };
      }

      // Turn detection for realtime mode
      if (this.config.realtime.turnDetection !== undefined) {
        realtimeConfig.turn_detection = {
          enabled: this.config.realtime.turnDetection.enabled,
          threshold: this.config.realtime.turnDetection.threshold,
          silence_ms: this.config.realtime.turnDetection.silenceMs,
          prefix_padding_ms: this.config.realtime.turnDetection.prefixPaddingMs,
        };
      }

      config.realtime_config = realtimeConfig;
    }

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

        case 'error':
          this.handlers.onError(new Error(data.message || 'Unknown error'));
          break;

        case 'pong':
          // Handle pong for latency measurement
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

  ping(): void {
    this.send({
      type: 'ping',
      timestamp: Date.now(),
    });
  }

  get connected(): boolean {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN;
  }
}
