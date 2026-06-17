/**
 * Widget configuration parsing
 */

import type {
  WidgetConfig,
  ConversationConfig,
  AudioFeatures,
  STTProvider,
  TTSProvider,
  RealtimeProvider,
  EmotionType,
  DeliveryStyle,
  ReasoningEffort,
  LatencyFiller,
  AdapterKind,
  MuteStrategy,
  RoutingMode,
} from './types';

/**
 * Parse configuration from data attributes
 */
export function parseConfigFromAttributes(element: HTMLElement): WidgetConfig {
  const config: WidgetConfig = {
    gatewayUrl: element.dataset.gatewayUrl || 'ws://localhost:3009/ws',
    apiKey: element.dataset.apiKey || element.dataset.authToken,
    // P3 proxy/alias: a server-defined name resolved to a full agent bundle.
    alias: element.dataset.alias,
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

  // Parse TTS config with emotion support.
  //
  // Flagship one-tag ergonomics: a bare `data-tts-voice` (or `data-tts-voice-id`)
  // is enough to make the bot speak — we default the provider to `deepgram` when
  // only a voice is given, so `data-llm-* + data-stt-provider + data-tts-voice`
  // produces a full talking loop in a single HTML tag. An explicit
  // `data-tts-provider` always wins.
  const ttsProvider = element.dataset.ttsProvider;
  const ttsVoice = element.dataset.ttsVoice;
  const ttsVoiceId = element.dataset.ttsVoiceId;
  if (ttsProvider || ttsVoice || ttsVoiceId) {
    config.tts = {
      provider: (ttsProvider as TTSProvider | undefined) || 'deepgram',
      voice: ttsVoice,
      voiceId: ttsVoiceId,
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

  // Parse conversation-loop (LLM) config — drives the gateway STT->LLM->TTS loop
  // so the bot talks back. Requires at least a base URL + model. This is the
  // FLAGSHIP one-tag surface: `data-llm-base-url` + `data-llm-model` give a
  // talking bot; the reasoning / barge-in / latency-filler dials below add
  // realtime-reasoning in a few more attributes. Maps 1:1 to conversation_config.
  const llmBaseUrl = element.dataset.llmBaseUrl;
  const llmModel = element.dataset.llmModel;
  if (llmBaseUrl && llmModel) {
    config.conversation = parseConversationFromAttributes(element, llmBaseUrl, llmModel);
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
 * Parse the conversation-loop (LLM) config from `data-llm-*` attributes.
 *
 * `baseUrl` + `model` are required (passed in by the caller after the presence
 * check). Every other field maps 1:1 to a {@link ConversationConfig} key and is
 * only set when its attribute is present, so an unspecified dial takes the
 * gateway default. Exported-shaped as a module helper for testability.
 */
function parseConversationFromAttributes(
  element: HTMLElement,
  baseUrl: string,
  model: string
): ConversationConfig {
  const d = element.dataset;
  const conv: ConversationConfig = { baseUrl, model };

  // Core LLM
  if (d.llmSystemPrompt !== undefined) conv.systemPrompt = d.llmSystemPrompt;
  if (d.llmApiKey !== undefined) conv.apiKey = d.llmApiKey;
  const temperature = parseFloatAttr(d.llmTemperature);
  if (temperature !== undefined) conv.temperature = temperature;
  const maxTokens = parseIntAttr(d.llmMaxTokens);
  if (maxTokens !== undefined) conv.maxTokens = maxTokens;
  const streaming = parseBoolAttr(d.llmStreaming);
  if (streaming !== undefined) conv.streaming = streaming;
  const maxHistory = parseIntAttr(d.llmMaxHistory);
  if (maxHistory !== undefined) conv.maxHistory = maxHistory;
  const allowInterruption = parseBoolAttr(d.llmAllowInterruption);
  if (allowInterruption !== undefined) conv.allowInterruption = allowInterruption;
  if (d.llmProviderKind !== undefined) conv.providerKind = d.llmProviderKind as AdapterKind;
  const stripMarkdown = parseBoolAttr(d.llmStripMarkdown);
  if (stripMarkdown !== undefined) conv.stripMarkdown = stripMarkdown;

  // Barge-in / mute
  const eagerEot = parseBoolAttr(d.llmEagerEot);
  if (eagerEot !== undefined) conv.eagerEot = eagerEot;
  const bargeInMinWords = parseIntAttr(d.llmBargeInMinWords);
  if (bargeInMinWords !== undefined) conv.bargeInMinWords = bargeInMinWords;
  if (d.llmMuteStrategy !== undefined) conv.muteStrategy = d.llmMuteStrategy as MuteStrategy;
  const userIdleTimeoutMs = parseIntAttr(d.llmUserIdleTimeoutMs);
  if (userIdleTimeoutMs !== undefined) conv.userIdleTimeoutMs = userIdleTimeoutMs;
  const summarizeTargetTokens = parseIntAttr(d.llmSummarizeTargetTokens);
  if (summarizeTargetTokens !== undefined) conv.summarizeTargetTokens = summarizeTargetTokens;

  // Latency filler
  if (d.llmLatencyFiller !== undefined) conv.latencyFiller = d.llmLatencyFiller as LatencyFiller;
  const latencyFillerAfterMs = parseIntAttr(d.llmLatencyFillerAfterMs);
  if (latencyFillerAfterMs !== undefined) conv.latencyFillerAfterMs = latencyFillerAfterMs;
  const latencyFillerPhrases = parseCsvAttr(d.llmLatencyFillerPhrases);
  if (latencyFillerPhrases !== undefined) conv.latencyFillerPhrases = latencyFillerPhrases;

  // Reasoning tier (two-tier)
  if (d.llmReasoningEffort !== undefined)
    conv.reasoningEffort = d.llmReasoningEffort as ReasoningEffort;
  if (d.llmReasoningModel !== undefined) conv.reasoningModel = d.llmReasoningModel;
  if (d.llmReasoningBaseUrl !== undefined) conv.reasoningBaseUrl = d.llmReasoningBaseUrl;
  if (d.llmReasoningApiKey !== undefined) conv.reasoningApiKey = d.llmReasoningApiKey;
  if (d.llmReasoningProviderKind !== undefined)
    conv.reasoningProviderKind = d.llmReasoningProviderKind as AdapterKind;
  if (d.llmReasoningRoute !== undefined) conv.reasoningRoute = d.llmReasoningRoute as RoutingMode;
  const reasoningBudgetMs = parseIntAttr(d.llmReasoningBudgetMs);
  if (reasoningBudgetMs !== undefined) conv.reasoningBudgetMs = reasoningBudgetMs;
  if (d.llmDegradationMessage !== undefined) conv.degradationMessage = d.llmDegradationMessage;
  const maxLlmCallsPerTurn = parseIntAttr(d.llmMaxLlmCallsPerTurn);
  if (maxLlmCallsPerTurn !== undefined) conv.maxLlmCallsPerTurn = maxLlmCallsPerTurn;
  const maxReasoningTokens = parseIntAttr(d.llmMaxReasoningTokens);
  if (maxReasoningTokens !== undefined) conv.maxReasoningTokens = maxReasoningTokens;

  return conv;
}

/** Parse an integer attribute; undefined when absent or non-numeric. */
function parseIntAttr(value: string | undefined): number | undefined {
  if (value === undefined || value === '') return undefined;
  const n = parseInt(value, 10);
  return Number.isNaN(n) ? undefined : n;
}

/** Parse a float attribute; undefined when absent or non-numeric. */
function parseFloatAttr(value: string | undefined): number | undefined {
  if (value === undefined || value === '') return undefined;
  const n = parseFloat(value);
  return Number.isNaN(n) ? undefined : n;
}

/** Parse a boolean attribute; undefined when absent ("false" => false, else true). */
function parseBoolAttr(value: string | undefined): boolean | undefined {
  if (value === undefined) return undefined;
  return value !== 'false';
}

/** Parse a comma-separated attribute into a trimmed, non-empty string array. */
function parseCsvAttr(value: string | undefined): string[] | undefined {
  if (value === undefined) return undefined;
  const parts = value
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  return parts.length > 0 ? parts : undefined;
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
      eager: element.dataset.turnDetectionEager === 'true' || undefined,
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
    gatewayUrl: config.gatewayUrl || 'ws://localhost:3009/ws',
    apiKey: config.apiKey,
    // P3 proxy/alias: carry the server-defined alias name through (it was being
    // dropped by this explicit-field rebuild — a real bug the alias test caught).
    alias: config.alias,
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
    conversation: config.conversation,
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
