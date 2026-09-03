/**
 * Canonical provider value-space — mirrors the WaaV gateway dispatch tables.
 *
 * SOURCE OF TRUTH (do not hand-curate divergently): the gateway's provider
 * dispatch/registry tables:
 *   - STT:      gateway/src/core/stt/standard.rs (create_stt_standard match arms)
 *               + gateway/src/plugin/dispatch.rs (BUILTIN_STT_NAMES / STT_PROVIDER_MAP)
 *               + gateway/src/core/stt/mod.rs (get_supported_stt_providers).
 *   - TTS:      gateway/src/core/tts/standard.rs (create_tts_standard match arms)
 *               + gateway/src/plugin/dispatch.rs (BUILTIN_TTS_NAMES / TTS_PROVIDER_MAP).
 *   - REALTIME: gateway/src/plugin/dispatch.rs (BUILTIN_REALTIME_NAMES /
 *               REALTIME_PROVIDER_MAP) + core/realtime/mod.rs
 *               (get_supported_realtime_providers).
 *
 * These are the CANONICAL primary names (not aliases). A value here is one the
 * gateway recognizes and will route; offering a value the gateway does not
 * recognize would silently no-op, so this list must stay a faithful mirror.
 *
 * The matching gateway-side counts are 32 STT / 37 TTS / 12 realtime. The
 * widget drift guard (`test/drift.test.mjs`) asserts these counts so a gateway
 * provider added without updating the widget is caught in CI.
 *
 * NOTE on realtime: realtime/S2S targets a SEPARATE `/realtime` endpoint and is
 * NOT sent on the `/ws` conversation path. The list is exposed for completeness
 * and provider-pickers; the widget's `/ws` config never serializes it.
 */

/** All STT providers the gateway can dispatch (canonical primary names). */
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

/** All TTS providers the gateway can dispatch (canonical primary names). */
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

/**
 * All realtime/S2S providers the gateway can dispatch (canonical primary names).
 * These target the `/realtime` endpoint, NOT `/ws` — exposed for provider
 * pickers and parity, not serialized by the widget's `/ws` config.
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

/** A canonical STT provider name the gateway can dispatch. */
export type STTProvider = (typeof STT_PROVIDERS)[number];
/** A canonical TTS provider name the gateway can dispatch. */
export type TTSProvider = (typeof TTS_PROVIDERS)[number];
/** A canonical realtime/S2S provider name the gateway can dispatch. */
export type RealtimeProvider = (typeof REALTIME_PROVIDERS)[number];

/** True if `provider` is a gateway-recognized STT provider. */
export function isValidSTTProvider(provider: string): provider is STTProvider {
  return (STT_PROVIDERS as readonly string[]).includes(provider);
}

/** True if `provider` is a gateway-recognized TTS provider. */
export function isValidTTSProvider(provider: string): provider is TTSProvider {
  return (TTS_PROVIDERS as readonly string[]).includes(provider);
}

/** True if `provider` is a gateway-recognized realtime/S2S provider. */
export function isValidRealtimeProvider(provider: string): provider is RealtimeProvider {
  return (REALTIME_PROVIDERS as readonly string[]).includes(provider);
}
