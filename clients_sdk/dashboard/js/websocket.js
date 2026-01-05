/**
 * WebSocket Manager for Dashboard
 */

export class WebSocketManager {
  constructor(url, apiKey) {
    this.url = url;
    this.apiKey = apiKey;
    this.ws = null;
    this.onOpen = null;
    this.onMessage = null;
    this.onClose = null;
    this.onError = null;
  }

  connect() {
    return new Promise((resolve, reject) => {
      try {
        const wsUrl = new URL(this.url);
        if (this.apiKey) {
          wsUrl.searchParams.set('token', this.apiKey);
        }

        this.ws = new WebSocket(wsUrl.toString());

        this.ws.onopen = () => {
          if (this.onOpen) this.onOpen();
          resolve();
        };

        this.ws.onmessage = (event) => {
          if (event.data instanceof Blob) {
            // Binary audio data
            event.data.arrayBuffer().then((buffer) => {
              if (this.onMessage) {
                this.onMessage({ type: 'audio', audio: buffer });
              }
            });
          } else {
            try {
              const data = JSON.parse(event.data);
              if (this.onMessage) this.onMessage(data);
            } catch (e) {
              console.error('Failed to parse message:', e);
            }
          }
        };

        this.ws.onclose = () => {
          if (this.onClose) this.onClose();
        };

        this.ws.onerror = (error) => {
          if (this.onError) this.onError(error);
          reject(error);
        };
      } catch (error) {
        reject(error);
      }
    });
  }

  disconnect() {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  send(data) {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(data));
    }
  }

  sendAudio(audioData) {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      if (audioData instanceof Int16Array) {
        this.ws.send(audioData.buffer);
      } else {
        this.ws.send(audioData);
      }
    }
  }

  sendConfig(config) {
    const message = {
      type: 'config',
      audio: true,
    };

    if (config.stt) {
      message.stt_config = {
        provider: config.stt.provider || 'deepgram',
        language: config.stt.language || 'en-US',
        sample_rate: config.stt.sample_rate || 16000,
        channels: config.stt.channels || 1,
        encoding: config.stt.encoding || 'linear16',
        model: config.stt.model || 'nova-3',
        punctuation: config.stt.punctuation ?? true,
      };
      // Include provider API key if provided
      if (config.stt.api_key) {
        message.stt_config.api_key = config.stt.api_key;
        console.log('[WS] STT API key set for provider:', message.stt_config.provider);
      }
    }

    if (config.tts) {
      message.tts_config = {
        provider: config.tts.provider || 'deepgram',
        voice_id: config.tts.voice_id,
        sample_rate: config.tts.sample_rate || 24000,
        model: config.tts.model,
      };
      // Include provider API key if provided
      if (config.tts.api_key) {
        message.tts_config.api_key = config.tts.api_key;
        console.log('[WS] TTS API key set for provider:', message.tts_config.provider);
      }
      // If TTS and STT use the same provider and TTS has no key, use STT's key
      else if (config.stt && config.stt.api_key &&
               message.stt_config.provider === message.tts_config.provider) {
        message.tts_config.api_key = config.stt.api_key;
        console.log('[WS] TTS API key shared from STT for provider:', message.tts_config.provider);
      }
    }

    console.log('[WS] Sending config:', JSON.stringify(message, null, 2));
    this.send(message);
  }

  get connected() {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN;
  }
}
