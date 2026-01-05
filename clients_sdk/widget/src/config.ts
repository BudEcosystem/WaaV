/**
 * Widget configuration parsing
 */

import type { WidgetConfig, STTConfig, TTSConfig, FeatureFlags } from './types';

/**
 * Parse configuration from data attributes
 */
export function parseConfigFromAttributes(element: HTMLElement): WidgetConfig {
  const config: WidgetConfig = {
    gatewayUrl: element.dataset.gatewayUrl || 'ws://localhost:3001/ws',
    apiKey: element.dataset.apiKey || element.dataset.authToken,
    theme: parseTheme(element.dataset.theme),
    position: parsePosition(element.dataset.position),
    mode: parseMode(element.dataset.mode),
    showMetrics: element.dataset.showMetrics === 'true',
    customCss: element.dataset.customCss,
  };

  // Parse STT config
  const sttProvider = element.dataset.sttProvider;
  if (sttProvider) {
    config.stt = {
      provider: sttProvider,
      language: element.dataset.sttLanguage || 'en-US',
      model: element.dataset.sttModel,
      sampleRate: parseInt(element.dataset.sttSampleRate || '16000', 10),
      channels: parseInt(element.dataset.sttChannels || '1', 10),
      encoding: element.dataset.sttEncoding || 'linear16',
    };
  }

  // Parse TTS config
  const ttsProvider = element.dataset.ttsProvider;
  if (ttsProvider) {
    config.tts = {
      provider: ttsProvider,
      voice: element.dataset.ttsVoice,
      voiceId: element.dataset.ttsVoiceId,
      model: element.dataset.ttsModel,
      sampleRate: parseInt(element.dataset.ttsSampleRate || '24000', 10),
    };
  }

  // Parse feature flags
  config.features = {
    vad: element.dataset.vad !== 'false',
    noiseCancellation: element.dataset.noiseCancellation === 'true',
    speakerDiarization: element.dataset.speakerDiarization === 'true',
    interimResults: element.dataset.interimResults !== 'false',
    punctuation: element.dataset.punctuation !== 'false',
    profanityFilter: element.dataset.profanityFilter === 'true',
    smartFormat: element.dataset.smartFormat !== 'false',
    echoCancellation: element.dataset.echoCancellation !== 'false',
  };

  return config;
}

function parseTheme(value: string | undefined): 'light' | 'dark' | 'auto' {
  if (value === 'light' || value === 'dark' || value === 'auto') {
    return value;
  }
  return 'auto';
}

function parsePosition(
  value: string | undefined
): 'bottom-right' | 'bottom-left' | 'top-right' | 'top-left' {
  if (
    value === 'bottom-right' ||
    value === 'bottom-left' ||
    value === 'top-right' ||
    value === 'top-left'
  ) {
    return value;
  }
  return 'bottom-right';
}

function parseMode(value: string | undefined): 'push-to-talk' | 'vad' {
  if (value === 'push-to-talk' || value === 'vad') {
    return value;
  }
  return 'vad';
}

/**
 * Merge configs with defaults
 */
export function mergeConfig(config: Partial<WidgetConfig>): WidgetConfig {
  return {
    gatewayUrl: config.gatewayUrl || 'ws://localhost:3001/ws',
    apiKey: config.apiKey,
    theme: config.theme || 'auto',
    position: config.position || 'bottom-right',
    mode: config.mode || 'vad',
    showMetrics: config.showMetrics ?? false,
    customCss: config.customCss,
    stt: config.stt || {
      provider: 'deepgram',
      language: 'en-US',
      sampleRate: 16000,
      channels: 1,
      encoding: 'linear16',
    },
    tts: config.tts || {
      provider: 'deepgram',
      sampleRate: 24000,
    },
    features: {
      vad: config.features?.vad ?? true,
      noiseCancellation: config.features?.noiseCancellation ?? false,
      speakerDiarization: config.features?.speakerDiarization ?? false,
      interimResults: config.features?.interimResults ?? true,
      punctuation: config.features?.punctuation ?? true,
      profanityFilter: config.features?.profanityFilter ?? false,
      smartFormat: config.features?.smartFormat ?? true,
      echoCancellation: config.features?.echoCancellation ?? true,
    },
  };
}
