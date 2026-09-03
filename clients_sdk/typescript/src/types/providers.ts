// =============================================================================
// Provider Type Definitions
// =============================================================================

/**
 * STT (Speech-to-Text) providers supported by the WaaV gateway (the canonical
 * primary names from the gateway dispatch table, gateway/src/plugin/dispatch.rs
 * STT_PROVIDER_MAP / get_supported_stt_providers). 32 providers.
 *
 * Kept in sync by the provider-enum drift guard (tests/unit/providers.test.ts).
 */
export const STT_PROVIDERS = [
  'alibaba-cloud',
  'amivoice',
  'assemblyai',
  'aws-transcribe',
  'baidu',
  'bhashini',
  'cartesia',
  'deepgram',
  'elevenlabs',
  'fpt-ai',
  'gladia',
  'gnani',
  'google',
  'groq',
  'huawei-cloud',
  'ibm-watson',
  'iflytek',
  'microsoft-azure',
  'naver-clova',
  'nectec',
  'openai',
  'phonexia',
  'prosa-ai',
  'revai',
  'reverie',
  'sarvam',
  'sberdevices',
  'speechmatics',
  'tencent',
  'tinkoff',
  'viettel-ai',
  'yandex',
] as const;

export type STTProvider = (typeof STT_PROVIDERS)[number];

/**
 * TTS (Text-to-Speech) providers supported by the WaaV gateway (canonical
 * primary names from the gateway dispatch table TTS_PROVIDER_MAP). 37 providers.
 */
export const TTS_PROVIDERS = [
  'acapela',
  'alibaba-cloud',
  'aws-polly',
  'baidu',
  'bhashini',
  'cartesia',
  'cereproc',
  'deepgram',
  'elevenlabs',
  'fpt-ai',
  'gnani',
  'google',
  'huawei-cloud',
  'hume',
  'ibm-watson',
  'iflytek',
  'lmnt',
  'microsoft-azure',
  'murf',
  'naver-clova',
  'nectec',
  'openai',
  'playht',
  'prosa-ai',
  'resemble',
  'reverie',
  'sberdevices',
  'smallest',
  'speechify',
  'speechmatics',
  'tencent',
  'tinkoff',
  'unrealspeech',
  'viettel-ai',
  'wellsaid',
  'yandex',
  'zalo-ai',
] as const;

export type TTSProvider = (typeof TTS_PROVIDERS)[number];

/**
 * Realtime (speech-to-speech / S2S) providers for bidirectional conversation
 * (canonical primary names from the gateway dispatch table REALTIME_PROVIDER_MAP
 * / BuiltinRealtimeProvider enum). 12 providers — these are the gateway's
 * `/realtime` providers, NOT the OpenAI-native names the old list used.
 */
export const REALTIME_PROVIDERS = [
  'openai',
  'hume',
  'azure',
  'grok',
  'inworld',
  'deepgram',
  'elevenlabs',
  'gemini',
  'ultravox',
  'nova_sonic',
  'speechmatics',
  'yandex',
] as const;

export type RealtimeProvider = (typeof REALTIME_PROVIDERS)[number];

// =============================================================================
// Provider Validation
// =============================================================================

/**
 * Check if a string is a valid STT provider.
 */
export function isValidSTTProvider(provider: string): provider is STTProvider {
  return STT_PROVIDERS.includes(provider as STTProvider);
}

/**
 * Check if a string is a valid TTS provider.
 */
export function isValidTTSProvider(provider: string): provider is TTSProvider {
  return TTS_PROVIDERS.includes(provider as TTSProvider);
}

/**
 * Check if a string is a valid Realtime provider.
 */
export function isValidRealtimeProvider(provider: string): provider is RealtimeProvider {
  return REALTIME_PROVIDERS.includes(provider as RealtimeProvider);
}

// =============================================================================
// Provider Capabilities
// =============================================================================

/**
 * STT provider capabilities.
 */
export interface STTCapabilities {
  streaming: boolean;
  languages: string[];
  models: string[];
  supportsInterim: boolean;
  supportsDiarization: boolean;
  supportsWordTimestamps: boolean;
  supportsPunctuation: boolean;
  supportsProfanityFilter: boolean;
  supportsSmartFormat: boolean;
}

/**
 * TTS provider capabilities.
 */
export interface TTSCapabilities {
  streaming: boolean;
  voices: string[];
  languages: string[];
  supportsEmotion: boolean;
  supportsSSML: boolean;
  supportsVoiceCloning: boolean;
  supportsPronunciations: boolean;
  maxCharacters: number;
  outputFormats: string[];
}

/**
 * Realtime provider capabilities.
 */
export interface RealtimeCapabilities {
  streaming: boolean;
  supportsInterruption: boolean;
  supportsFunctionCalling: boolean;
  supportsEmotion: boolean;
  supportsVAD: boolean;
  models: string[];
  maxSessionDuration: number; // seconds
}

export type ProviderCapabilities = STTCapabilities | TTSCapabilities | RealtimeCapabilities;

// =============================================================================
// Provider Capability Database
// =============================================================================

// Capability hints are a CONVENIENCE database (not the source of truth — the
// gateway is). It covers the commonly-used providers; unknown providers simply
// have no entry (getProviderCapabilities returns undefined for them rather than
// throwing, so a beginner using a less-common provider is never blocked).
const STT_CAPABILITIES: Partial<Record<STTProvider, STTCapabilities>> = {
  deepgram: {
    streaming: true,
    languages: ['en-US', 'en-GB', 'es', 'fr', 'de', 'pt', 'it', 'nl', 'ja', 'ko', 'zh'],
    models: ['nova-2', 'nova-3', 'enhanced', 'base'],
    supportsInterim: true,
    supportsDiarization: true,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: true,
    supportsSmartFormat: true,
  },
  google: {
    streaming: true,
    languages: ['en-US', 'en-GB', 'es-ES', 'fr-FR', 'de-DE', 'pt-BR', 'it-IT', 'ja-JP', 'ko-KR', 'zh-CN'],
    models: ['latest_long', 'latest_short', 'phone_call', 'video'],
    supportsInterim: true,
    supportsDiarization: true,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: true,
    supportsSmartFormat: false,
  },
  'microsoft-azure': {
    streaming: true,
    languages: ['en-US', 'en-GB', 'es-ES', 'fr-FR', 'de-DE', 'pt-BR', 'it-IT', 'ja-JP', 'ko-KR', 'zh-CN'],
    models: ['whisper', 'conversation', 'dictation'],
    supportsInterim: true,
    supportsDiarization: true,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: true,
    supportsSmartFormat: false,
  },
  cartesia: {
    streaming: true,
    languages: ['en-US'],
    models: ['sonic'],
    supportsInterim: true,
    supportsDiarization: false,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: false,
    supportsSmartFormat: false,
  },
  assemblyai: {
    streaming: true,
    languages: ['en-US', 'en-GB', 'es', 'fr', 'de', 'pt', 'it', 'nl', 'ja'],
    models: ['best', 'nano'],
    supportsInterim: true,
    supportsDiarization: true,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: true,
    supportsSmartFormat: true,
  },
  'aws-transcribe': {
    streaming: true,
    languages: ['en-US', 'en-GB', 'es-US', 'fr-FR', 'de-DE', 'pt-BR', 'it-IT', 'ja-JP', 'ko-KR', 'zh-CN'],
    models: ['general', 'medical', 'call-analytics'],
    supportsInterim: true,
    supportsDiarization: true,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: true,
    supportsSmartFormat: false,
  },
  'ibm-watson': {
    streaming: true,
    languages: ['en-US', 'en-GB', 'es-ES', 'fr-FR', 'de-DE', 'pt-BR', 'it-IT', 'ja-JP', 'ko-KR', 'zh-CN'],
    models: ['en-US_BroadbandModel', 'en-US_NarrowbandModel', 'en-US_Telephony'],
    supportsInterim: true,
    supportsDiarization: true,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: true,
    supportsSmartFormat: true,
  },
  groq: {
    streaming: true,
    languages: ['en', 'es', 'fr', 'de', 'pt', 'it', 'nl', 'ja', 'ko', 'zh'],
    models: ['whisper-large-v3', 'whisper-large-v3-turbo', 'distil-whisper-large-v3-en'],
    supportsInterim: false,
    supportsDiarization: false,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: false,
    supportsSmartFormat: false,
  },
  openai: {
    streaming: false,
    languages: ['en', 'es', 'fr', 'de', 'pt', 'it', 'nl', 'ja', 'ko', 'zh'],
    models: ['whisper-1', 'gpt-4o-transcribe', 'gpt-4o-mini-transcribe'],
    supportsInterim: false,
    supportsDiarization: false,
    supportsWordTimestamps: true,
    supportsPunctuation: true,
    supportsProfanityFilter: false,
    supportsSmartFormat: false,
  },
};

const TTS_CAPABILITIES: Partial<Record<TTSProvider, TTSCapabilities>> = {
  deepgram: {
    streaming: true,
    voices: ['aura-asteria-en', 'aura-arcas-en', 'aura-helios-en', 'aura-luna-en', 'aura-orpheus-en'],
    languages: ['en-US'],
    supportsEmotion: false,
    supportsSSML: false,
    supportsVoiceCloning: false,
    supportsPronunciations: true,
    maxCharacters: 100000,
    outputFormats: ['linear16', 'mp3', 'opus', 'flac', 'alaw', 'mulaw'],
  },
  elevenlabs: {
    streaming: true,
    voices: ['rachel', 'drew', 'clyde', 'paul', 'domi', 'dave', 'fin', 'sarah', 'antoni', 'thomas'],
    languages: ['en', 'es', 'fr', 'de', 'pt', 'it', 'pl', 'hi', 'ar'],
    supportsEmotion: true,
    supportsSSML: true,
    supportsVoiceCloning: true,
    supportsPronunciations: true,
    maxCharacters: 10000,
    outputFormats: ['mp3', 'pcm_16000', 'pcm_22050', 'pcm_24000', 'pcm_44100'],
  },
  google: {
    streaming: true,
    voices: ['en-US-Standard-A', 'en-US-Standard-B', 'en-US-Wavenet-A', 'en-US-Neural2-A'],
    languages: ['en-US', 'en-GB', 'es-ES', 'fr-FR', 'de-DE', 'pt-BR', 'it-IT', 'ja-JP', 'ko-KR', 'zh-CN'],
    supportsEmotion: false,
    supportsSSML: true,
    supportsVoiceCloning: false,
    supportsPronunciations: true,
    maxCharacters: 5000,
    outputFormats: ['LINEAR16', 'MP3', 'OGG_OPUS'],
  },
  'microsoft-azure': {
    streaming: true,
    voices: ['en-US-JennyNeural', 'en-US-GuyNeural', 'en-US-AriaNeural', 'en-US-DavisNeural'],
    languages: ['en-US', 'en-GB', 'es-ES', 'fr-FR', 'de-DE', 'pt-BR', 'it-IT', 'ja-JP', 'ko-KR', 'zh-CN'],
    supportsEmotion: true,
    supportsSSML: true,
    supportsVoiceCloning: true,
    supportsPronunciations: true,
    maxCharacters: 10000,
    outputFormats: ['audio-16khz-128kbitrate-mono-mp3', 'riff-16khz-16bit-mono-pcm', 'ogg-16khz-16bit-mono-opus'],
  },
  cartesia: {
    streaming: true,
    voices: ['sonic-english-male', 'sonic-english-female'],
    languages: ['en'],
    supportsEmotion: false,
    supportsSSML: false,
    supportsVoiceCloning: false,
    supportsPronunciations: false,
    maxCharacters: 50000,
    outputFormats: ['pcm_f32le', 'pcm_s16le'],
  },
  openai: {
    streaming: true,
    voices: ['alloy', 'echo', 'fable', 'onyx', 'nova', 'shimmer'],
    languages: ['en', 'es', 'fr', 'de', 'pt', 'it', 'pl', 'ru', 'ja', 'ko', 'zh'],
    supportsEmotion: false,
    supportsSSML: false,
    supportsVoiceCloning: false,
    supportsPronunciations: false,
    maxCharacters: 4096,
    outputFormats: ['mp3', 'opus', 'aac', 'flac', 'wav', 'pcm'],
  },
  'aws-polly': {
    streaming: true,
    voices: ['Joanna', 'Matthew', 'Amy', 'Brian', 'Emma', 'Russell', 'Nicole', 'Olivia'],
    languages: ['en-US', 'en-GB', 'en-AU', 'es-ES', 'fr-FR', 'de-DE', 'pt-BR', 'it-IT', 'ja-JP'],
    supportsEmotion: false,
    supportsSSML: true,
    supportsVoiceCloning: false,
    supportsPronunciations: true,
    maxCharacters: 3000,
    outputFormats: ['mp3', 'ogg_vorbis', 'pcm'],
  },
  'ibm-watson': {
    streaming: true,
    voices: ['en-US_AllisonV3Voice', 'en-US_MichaelV3Voice', 'en-US_EmilyV3Voice'],
    languages: ['en-US', 'en-GB', 'es-ES', 'fr-FR', 'de-DE', 'pt-BR', 'it-IT', 'ja-JP'],
    supportsEmotion: false,
    supportsSSML: true,
    supportsVoiceCloning: false,
    supportsPronunciations: true,
    maxCharacters: 5000,
    outputFormats: ['audio/wav', 'audio/mp3', 'audio/ogg;codecs=opus'],
  },
  hume: {
    streaming: true,
    voices: ['ITO', 'KORA', 'DACHER', 'AURA', 'STELLA'],
    languages: ['en'],
    supportsEmotion: true,
    supportsSSML: false,
    supportsVoiceCloning: true,
    supportsPronunciations: false,
    maxCharacters: 10000,
    outputFormats: ['pcm'],
  },
  lmnt: {
    streaming: true,
    voices: ['lily', 'daniel', 'mira', 'emily'],
    languages: ['en'],
    supportsEmotion: false,
    supportsSSML: false,
    supportsVoiceCloning: true,
    supportsPronunciations: false,
    maxCharacters: 5000,
    outputFormats: ['mp3', 'wav'],
  },
  playht: {
    streaming: true,
    voices: ['matthew', 'jennifer', 'richard', 'sarah'],
    languages: ['en', 'es', 'fr', 'de', 'pt', 'it'],
    supportsEmotion: false,
    supportsSSML: true,
    supportsVoiceCloning: true,
    supportsPronunciations: false,
    maxCharacters: 10000,
    outputFormats: ['mp3', 'wav'],
  },
};

const REALTIME_CAPABILITIES: Partial<Record<RealtimeProvider, RealtimeCapabilities>> = {
  openai: {
    streaming: true,
    supportsInterruption: true,
    supportsFunctionCalling: true,
    supportsEmotion: false,
    supportsVAD: true,
    models: ['gpt-realtime', 'gpt-4o-realtime-preview', 'gpt-4o-realtime-preview-2024-12-17'],
    maxSessionDuration: 1800, // 30 minutes
  },
  hume: {
    streaming: true,
    supportsInterruption: true,
    supportsFunctionCalling: true,
    supportsEmotion: true,
    supportsVAD: true,
    models: ['evi-1', 'evi-2', 'evi-3', 'evi-4-mini'],
    maxSessionDuration: 3600, // 60 minutes
  },
};

/**
 * Get capabilities for a provider.
 *
 * The capability database is a convenience subset (the gateway is the source of
 * truth), so this returns `undefined` for a valid-but-uncatalogued provider
 * rather than throwing — a beginner using a less-common provider is never
 * blocked. It still throws for a genuinely unknown provider name / bad type.
 *
 * @param provider - Provider name
 * @param type - Provider type ('stt', 'tts', or 'realtime')
 * @returns Provider capabilities, or undefined if not catalogued.
 */
export function getProviderCapabilities(
  provider: string,
  type: 'stt' | 'tts' | 'realtime'
): ProviderCapabilities | undefined {
  switch (type) {
    case 'stt':
      if (!isValidSTTProvider(provider)) {
        throw new Error(`Unknown STT provider: ${provider}`);
      }
      return STT_CAPABILITIES[provider];

    case 'tts':
      if (!isValidTTSProvider(provider)) {
        throw new Error(`Unknown TTS provider: ${provider}`);
      }
      return TTS_CAPABILITIES[provider];

    case 'realtime':
      if (!isValidRealtimeProvider(provider)) {
        throw new Error(`Unknown Realtime provider: ${provider}`);
      }
      return REALTIME_CAPABILITIES[provider];

    default:
      throw new Error(`Unknown provider type: ${type}`);
  }
}

/**
 * Get list of providers that support a specific feature.
 */
export function getProvidersWithFeature<T extends keyof TTSCapabilities>(
  feature: T,
  type: 'tts'
): TTSProvider[];
export function getProvidersWithFeature<T extends keyof STTCapabilities>(
  feature: T,
  type: 'stt'
): STTProvider[];
export function getProvidersWithFeature<T extends keyof RealtimeCapabilities>(
  feature: T,
  type: 'realtime'
): RealtimeProvider[];
export function getProvidersWithFeature(
  feature: string,
  type: 'stt' | 'tts' | 'realtime'
): string[] {
  const has = (caps: ProviderCapabilities | undefined): boolean =>
    caps !== undefined && (caps as unknown as Record<string, unknown>)[feature] === true;
  switch (type) {
    case 'stt':
      return STT_PROVIDERS.filter((p) => has(STT_CAPABILITIES[p]));
    case 'tts':
      return TTS_PROVIDERS.filter((p) => has(TTS_CAPABILITIES[p]));
    case 'realtime':
      return REALTIME_PROVIDERS.filter((p) => has(REALTIME_CAPABILITIES[p]));
    default:
      return [];
  }
}

/**
 * Get default model for a provider (from the convenience capability database).
 */
export function getDefaultModel(provider: string, type: 'stt' | 'tts' | 'realtime'): string {
  const caps = getProviderCapabilities(provider, type);
  if (caps && 'models' in caps && caps.models[0] !== undefined) {
    return caps.models[0];
  }
  throw new Error(`No default model for ${type} provider: ${provider}`);
}

/**
 * Get default voice for a TTS provider.
 */
export function getDefaultVoice(provider: TTSProvider): string {
  const caps = TTS_CAPABILITIES[provider];
  if (caps && caps.voices[0] !== undefined) {
    return caps.voices[0];
  }
  throw new Error(`No default voice for TTS provider: ${provider}`);
}
