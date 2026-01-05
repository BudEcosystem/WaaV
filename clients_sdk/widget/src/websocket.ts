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

  constructor(config: WidgetConfig, handlers: MessageHandler) {
    this.config = config;
    this.handlers = handlers;
  }

  get metrics(): WidgetMetrics {
    return { ...this._metrics };
  }

  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
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
          this.handleMessage(event, resolve);
        };

        this.ws.onerror = (event) => {
          const error = new Error('WebSocket error');
          reject(error);
        };

        this.ws.onclose = () => {
          this.handlers.onClose();
          this.attemptReconnect();
        };
      } catch (error) {
        reject(error);
      }
    });
  }

  disconnect(): void {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  private sendConfig(): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;

    const config: Record<string, unknown> = {
      type: 'config',
      audio: true,
    };

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

    if (this.config.tts) {
      config.tts_config = {
        provider: this.config.tts.provider,
        voice_id: this.config.tts.voiceId || this.config.tts.voice,
        sample_rate: this.config.tts.sampleRate || 24000,
        model: this.config.tts.model,
      };
    }

    this.configSentTime = performance.now();
    this.send(config);
  }

  private handleMessage(event: MessageEvent, resolveConnect?: (value: void) => void): void {
    if (event.data instanceof Blob) {
      // Binary audio data
      event.data.arrayBuffer().then((buffer) => {
        if (this.speakStartTime !== null) {
          this._metrics.ttsTtfb = performance.now() - this.speakStartTime;
          this.speakStartTime = null;
        }
        this.handlers.onAudio(buffer, 'linear16', this.config.tts?.sampleRate || 24000, false);
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
    const binaryString = atob(base64);
    const len = binaryString.length;
    const bytes = new Uint8Array(len);
    for (let i = 0; i < len; i++) {
      bytes[i] = binaryString.charCodeAt(i);
    }
    return bytes.buffer;
  }

  private attemptReconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      this.handlers.onError(new Error('Max reconnection attempts reached'));
      return;
    }

    this.reconnectAttempts++;
    const delay = this.reconnectDelay * Math.pow(1.5, this.reconnectAttempts - 1);
    const jitter = delay * 0.2 * (Math.random() * 2 - 1);

    setTimeout(() => {
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

  speak(text: string, flush = false, allowInterruption = true): void {
    this.speakStartTime = performance.now();
    this.send({
      type: 'speak',
      text,
      flush,
      allow_interruption: allowInterruption,
    });
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
