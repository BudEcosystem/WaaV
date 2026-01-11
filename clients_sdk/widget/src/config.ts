/**
 * Widget configuration parsing
 */

import type {
  WidgetConfig,
  STTConfig,
  TTSConfig,
  FeatureFlags,
  EmotionConfig,
  RealtimeConfig,
  AudioFeatures,
  STTProvider,
  TTSProvider,
  RealtimeProvider,
  EmotionType,
  DeliveryStyle,
} from './types';

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
      provider: sttProvider as STTProvider,
      language: element.dataset.sttLanguage || 'en-US',
      model: element.dataset.sttModel,
      sampleRate: parseInt(element.dataset.sttSampleRate || '16000', 10),
      channels: parseInt(element.dataset.sttChannels || '1', 10),
      encoding: element.dataset.sttEncoding || 'linear16',
    };
  }

  // Parse TTS config with emotion support
  const ttsProvider = element.dataset.ttsProvider;
  if (ttsProvider) {
    config.tts = {
      provider: ttsProvider as TTSProvider,
      voice: element.dataset.ttsVoice,
      voiceId: element.dataset.ttsVoiceId,
      model: element.dataset.ttsModel,
      sampleRate: parseInt(element.dataset.ttsSampleRate || '24000', 10),
    };

    // Parse emotion config
    const emotion = element.dataset.ttsEmotion;
    if (emotion) {
      config.tts.emotion = {
        emotion: emotion as EmotionType,
        intensity: parseEmotionIntensity(element.dataset.ttsEmotionIntensity),
        deliveryStyle: element.dataset.ttsDeliveryStyle as DeliveryStyle | undefined,
        description: element.dataset.ttsEmotionDescription,
      };
    }
  }

  // Parse realtime config
  const realtimeProvider = element.dataset.realtimeProvider;
  if (realtimeProvider) {
    config.realtime = {
      provider: realtimeProvider as RealtimeProvider,
      model: element.dataset.realtimeModel,
      systemPrompt: element.dataset.realtimeSystemPrompt,
      voiceId: element.dataset.realtimeVoiceId,
      temperature: element.dataset.realtimeTemperature
        ? parseFloat(element.dataset.realtimeTemperature)
        : undefined,
      maxTokens: element.dataset.realtimeMaxTokens
        ? parseInt(element.dataset.realtimeMaxTokens, 10)
        : undefined,
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

  // Parse audio features
  config.audioFeatures = parseAudioFeatures(element);

  return config;
}

/**
 * Parse emotion intensity from string
 */
function parseEmotionIntensity(
  value: string | undefined
): 'low' | 'medium' | 'high' | number | undefined {
  if (!value) return undefined;
  if (value === 'low' || value === 'medium' || value === 'high') {
    return value;
  }
  const num = parseFloat(value);
  if (!isNaN(num) && num >= 0 && num <= 1) {
    return num;
  }
  return undefined;
}

/**
 * Parse audio features from data attributes
 */
function parseAudioFeatures(element: HTMLElement): AudioFeatures {
  const features: AudioFeatures = {};

  // Turn detection
  if (element.dataset.turnDetection !== undefined) {
    features.turnDetection = {
      enabled: element.dataset.turnDetection !== 'false',
      threshold: element.dataset.turnDetectionThreshold
        ? parseFloat(element.dataset.turnDetectionThreshold)
        : undefined,
      silenceMs: element.dataset.turnDetectionSilenceMs
        ? parseInt(element.dataset.turnDetectionSilenceMs, 10)
        : undefined,
      prefixPaddingMs: element.dataset.turnDetectionPrefixPaddingMs
        ? parseInt(element.dataset.turnDetectionPrefixPaddingMs, 10)
        : undefined,
    };
  }

  // Noise filter
  if (element.dataset.noiseFilter !== undefined) {
    features.noiseFilter = {
      enabled: element.dataset.noiseFilter !== 'false',
      strength: parseNoiseFilterStrength(element.dataset.noiseFilterStrength),
    };
  }

  // VAD
  if (element.dataset.vadEnabled !== undefined) {
    features.vad = {
      enabled: element.dataset.vadEnabled !== 'false',
      threshold: element.dataset.vadThreshold
        ? parseFloat(element.dataset.vadThreshold)
        : undefined,
      silenceMs: element.dataset.vadSilenceMs
        ? parseInt(element.dataset.vadSilenceMs, 10)
        : undefined,
    };
  }

  return features;
}

function parseNoiseFilterStrength(
  value: string | undefined
): 'low' | 'medium' | 'high' | undefined {
  if (value === 'low' || value === 'medium' || value === 'high') {
    return value;
  }
  return undefined;
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

function parseMode(value: string | undefined): 'push-to-talk' | 'vad' | 'realtime' {
  if (value === 'push-to-talk' || value === 'vad' || value === 'realtime') {
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
    realtime: config.realtime,
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
    audioFeatures: config.audioFeatures,
  };
}
