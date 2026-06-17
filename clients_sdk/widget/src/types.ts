/**
 * Widget configuration types
 */

// =============================================================================
// Provider Types
// =============================================================================
//
// The canonical provider value-space (32 STT / 37 TTS / 12 realtime) lives in
// ./providers.ts as runtime const arrays sourced from the gateway dispatch
// tables. Import the union TYPES for local use, and re-export them so existing
// imports (`import { STTProvider } from './types'`) keep working while the
// single source of truth stays in one place.

import type { STTProvider, TTSProvider, RealtimeProvider } from './providers';
export type { STTProvider, TTSProvider, RealtimeProvider } from './providers';

/**
 * Reasoning/thinking-effort dial for the conversation LLM.
 * Mapped server-side to each vendor's native thinking control (gateway config.rs reasoning_effort).
 */
export type ReasoningEffort = 'off' | 'minimal' | 'low' | 'medium' | 'high';

/**
 * Latency-masking mode for slow LLM/reasoning turns.
 * 'auto' speaks one short filler phrase when first audio is slow (gateway config.rs latency_filler).
 */
export type LatencyFiller = 'off' | 'auto' | 'aggressive';

// =============================================================================
// Emotion System
// =============================================================================

/**
 * Emotion types for TTS.
 * Matches Python SDK's Emotion enum for cross-SDK consistency.
 */
export type EmotionType =
  // Original 22.
  | 'neutral'
  | 'happy'
  | 'sad'
  | 'angry'
  | 'fearful'
  | 'surprised'
  | 'disgusted'
  | 'excited'
  | 'calm'
  | 'anxious'
  | 'confident'
  | 'confused'
  | 'empathetic'
  | 'sarcastic'
  | 'hopeful'
  | 'disappointed'
  | 'curious'
  | 'grateful'
  | 'proud'
  | 'embarrassed'
  | 'content'
  | 'bored'
  // P4 widened affective states (gateway Emotion 22 → 44).
  | 'amazed'
  | 'amused'
  | 'affectionate'
  | 'intrigued'
  | 'flirtatious'
  | 'frustrated'
  | 'annoyed'
  | 'determined'
  | 'reassuring'
  | 'sympathetic'
  | 'nostalgic'
  | 'serene'
  | 'terrified'
  | 'ecstatic'
  | 'skeptical'
  | 'relieved'
  | 'panicked'
  | 'concerned'
  | 'apologetic'
  | 'resigned'
  | 'wistful'
  | 'contemplative'
  // Forward-compat: accept any raw token the gateway may resolve.
  | (string & {});

/**
 * Delivery styles for TTS.
 * Matches Python SDK's DeliveryStyle enum for cross-SDK consistency.
 * P4 promoted `soft`/`cheerful`/`serious` to {@link EmotionType}; they are KEPT here
 * as backward-compatible cross-enum aliases (the gateway still accepts them).
 */
export type DeliveryStyle =
  | 'normal'
  | 'whispered'
  | 'shouted'
  | 'rushed'
  | 'measured'
  | 'monotone'
  | 'expressive'
  | 'professional'
  | 'casual'
  | 'storytelling'
  | 'soft'
  | 'loud'
  | 'cheerful'
  | 'serious'
  | 'formal'
  // P4 scenario / narration framings.
  | 'newscast'
  | 'newscast_casual'
  | 'newscast_formal'
  | 'customer_service'
  | 'assistant'
  | 'chat'
  | 'advertisement_upbeat'
  | 'sports_commentary'
  | 'sports_commentary_excited'
  | 'documentary_narration'
  | 'narration_professional'
  | 'narration_relaxed'
  | 'poetry_reading'
  | 'lyrical'
  | 'gentle'
  // Forward-compat: accept any raw token the gateway may resolve.
  | (string & {});

/** Desired voice gender for {@link VoiceDescriptor} (gateway `Gender`). */
export type VoiceGender = 'male' | 'female' | 'neutral';

/** Desired voice age band for {@link VoiceDescriptor} (gateway `Age`). */
export type VoiceAge = 'young' | 'middle_aged' | 'old';

/**
 * Abstract, provider-agnostic voice selection (P4).
 *
 * Mirrors the gateway `VoiceDescriptor`. Describe the voice you WANT (gender /
 * locale / accent / age / style / name hint) via `data-voice-*` attributes and the
 * gateway resolves it to a concrete provider `voiceId` server-side. Used ONLY when
 * no explicit `voiceId`/`voice` is set — a raw `voiceId` ALWAYS wins. Serialized
 * under `tts_config.voice_descriptor` (snake_case object); only-set fields emitted.
 */
export interface VoiceDescriptor {
  /** `male` | `female` | `neutral` (`data-voice-gender`). */
  gender?: VoiceGender;
  /** BCP-47 locale: `en-US`, `hi-IN`, … (`data-voice-locale`). */
  locale?: string;
  /** Free accent string: `american`, `british`, … (`data-voice-accent`). */
  accent?: string;
  /** `young` | `middle_aged` | `old` (`data-voice-age`). */
  age?: VoiceAge;
  /** Free timbre/style: `warm`, `bright`, … (`data-voice-style`). */
  style?: string;
  /** Preferred voice name (`asteria`, `jenny`); substring-matched (`data-voice-name-hint`). */
  nameHint?: string;
}

/** Emotion intensity */
export type EmotionIntensity = 'low' | 'medium' | 'high' | number;

/** Emotion configuration */
export interface EmotionConfig {
  emotion?: EmotionType;
  intensity?: EmotionIntensity;
  deliveryStyle?: DeliveryStyle;
  description?: string; // Max 100 chars (Hume)
}

// =============================================================================
// Audio Features
// =============================================================================

/**
 * ML turn-detection configuration.
 *
 * Maps to the gateway's `stt_config.turn_detection` (provider-agnostic smart-turn
 * detector). The gateway accepts only `enabled`, `threshold`, and `eager`
 * (gateway config.rs TurnDetectionWsConfig); `silenceMs`/`prefixPaddingMs` are
 * NOT wire keys on /ws and are intentionally not serialized.
 */
export interface TurnDetectionConfig {
  enabled: boolean;
  threshold?: number; // 0.0-1.0
  /** Eager end-of-turn: start the LLM speculatively on a turn-complete prediction. */
  eager?: boolean;
  /** @deprecated Not a /ws wire key; ignored when sending stt_config.turn_detection. */
  silenceMs?: number;
  /** @deprecated Not a /ws wire key; ignored when sending stt_config.turn_detection. */
  prefixPaddingMs?: number;
}

/** Noise filter configuration */
export interface NoiseFilterConfig {
  enabled: boolean;
  strength?: 'low' | 'medium' | 'high';
}

/** VAD (Voice Activity Detection) configuration */
export interface VADConfig {
  enabled: boolean;
  threshold?: number; // 0.0-1.0
  silenceMs?: number;
}

// =============================================================================
// Main Configuration
// =============================================================================

export interface WidgetConfig {
  /** WebSocket URL of the gateway */
  gatewayUrl: string;
  /** API key for authentication */
  apiKey?: string;
  /**
   * Proxy/alias name (P3, `data-alias`): a server-defined name the gateway
   * resolves to a full {stt,tts,llm,dag} bundle. `<bud-widget data-alias="...">`
   * is a complete agent; the bundle can be re-pointed server-side with no client
   * change. An explicit stt/tts/conversation here overrides the alias default.
   */
  alias?: string;
  /** STT configuration */
  stt?: STTConfig;
  /** TTS configuration */
  tts?: TTSConfig;
  /** Realtime configuration (for the separate /realtime S2S endpoint; NOT sent on /ws) */
  realtime?: RealtimeConfig;
  /**
   * Conversation-loop (LLM) configuration. When present, the gateway runs the
   * built-in STT -> LLM -> TTS loop so the bot talks back. Serialized to the
   * gateway `conversation_config` envelope.
   */
  conversation?: ConversationConfig;
  /** UI theme */
  theme?: 'light' | 'dark' | 'auto';
  /** Widget position */
  position?: 'bottom-right' | 'bottom-left' | 'top-right' | 'top-left';
  /** Voice activation mode */
  mode?: 'push-to-talk' | 'vad' | 'realtime';
  /** Show metrics overlay */
  showMetrics?: boolean;
  /** Feature flags */
  features?: FeatureFlags;
  /** Audio features */
  audioFeatures?: AudioFeatures;
  /** Custom CSS */
  customCss?: string;
}

export interface STTConfig {
  provider: STTProvider;
  language?: string;
  model?: string;
  sampleRate?: number;
  channels?: number;
  encoding?: string;
  /**
   * Canonical in-stream translation (P5). Ask for translation with the same
   * canonical BCP-47 tokens as {@link language}; the gateway maps to each
   * provider's native code and emits `translations:[{lang,text}]` on the
   * stt_result (Speechmatics/Gladia side-channel, OpenAI/Groq English fast path).
   * Serialized under `stt_config.translation`.
   */
  translation?: TranslationConfig;
}

/** Canonical translation request block (P5). Mirrors the gateway TranslationConfig. */
export interface TranslationConfig {
  /** Canonical BCP-47 target languages, e.g. `["hi-IN", "es-ES"]`. */
  targetLanguages?: string[];
  /** Fast path: translate the whole stream to English (OpenAI/Groq). */
  translateToEnglish?: boolean;
  /** Emit interim (partial) translations, not just finals. */
  partials?: boolean;
}

export interface TTSConfig {
  provider: TTSProvider;
  voice?: string;
  voiceId?: string;
  /**
   * Abstract voice selection (gender/locale/accent/age/style/nameHint) resolved to
   * a concrete provider `voiceId` server-side (P4, `data-voice-*`). Used ONLY when
   * no `voiceId`/`voice` is set — a raw `voiceId` always wins. Serialized under
   * `tts_config.voice_descriptor` on the wire.
   */
  voiceDescriptor?: VoiceDescriptor;
  model?: string;
  sampleRate?: number;
  /** Emotion configuration */
  emotion?: EmotionConfig;
}

export interface RealtimeConfig {
  provider: RealtimeProvider;
  model?: string;
  systemPrompt?: string;
  voiceId?: string;
  temperature?: number;
  maxTokens?: number;
  /** EVI version (Hume) */
  eviVersion?: string;
  /** Enable verbose transcription (Hume) */
  verboseTranscription?: boolean;
  /** Resume from previous chat group (Hume) */
  resumedChatGroupId?: string;
  /** Input audio transcription config (OpenAI) */
  inputAudioTranscription?: {
    model?: string;
  };
  /** Turn detection config for realtime mode */
  turnDetection?: TurnDetectionConfig;
}

export interface AudioFeatures {
  turnDetection?: TurnDetectionConfig;
  noiseFilter?: NoiseFilterConfig;
  vad?: VADConfig;
}

/**
 * LLM adapter kind for `providerKind` / `reasoningProviderKind`.
 *
 * Mirrors the gateway `AdapterKind` enum (gateway/src/core/llm/adapter.rs:40,
 * `serde(rename_all="lowercase")`). NOT a registered openapi schema (the gateway
 * overrides it to a plain nullable string), so the union is authored here from
 * adapter.rs. Omit => OpenAI-compatible with canonical-host inference.
 */
export type AdapterKind = 'openai' | 'anthropic' | 'gemini';

/** Reasoning-tier routing mode (gateway `RoutingMode`; default auto). */
export type RoutingMode = 'auto' | 'always';

/**
 * Mute/barge-in strategy while the bot speaks (gateway `mute_strategy`, an
 * Option<String> typed by convention — these are the accepted values).
 */
export type MuteStrategy =
  | 'always_while_bot_speaks'
  | 'until_first_bot_complete'
  | 'first_speech_only';

/**
 * Conversation-loop (LLM) configuration -> gateway `conversation_config`.
 *
 * Drives the built-in STT -> LLM -> TTS loop. `baseUrl` + `model` are required
 * by the gateway (ConversationWebSocketConfig). This is a 1:1 mirror of all 29
 * gateway fields (verified by the widget drift guard, `test/drift.test.mjs`):
 * a gateway field added/renamed without a matching key here fails CI. Includes
 * the REALTIME_REASONING two-tier dials (reasoningModel + reasoning* knobs) and
 * the barge-in / latency-filler controls.
 */
export interface ConversationConfig {
  // --- Core LLM (required: baseUrl, model) ---
  /** OpenAI-compatible base URL, e.g. "http://localhost:11434/v1" (required). */
  baseUrl: string;
  /** FAST model identifier, e.g. "qwen2.5:7b-instruct" (required). Keep this fast. */
  model: string;
  /** Optional system prompt seeding the conversation. */
  systemPrompt?: string;
  /** API key (literal or "${ENV_VAR}"); falls back to OPENAI_API_KEY server-side. */
  apiKey?: string;
  /** Sampling temperature. */
  temperature?: number;
  /** Max tokens per completion. */
  maxTokens?: number;
  /** Stream tokens to TTS as they arrive (gateway default true). */
  streaming?: boolean;
  /** Conversation history window in turns (gateway default 20). */
  maxHistory?: number;
  /** Whether the bot's speech is interruptible / barge-in (gateway default true). */
  allowInterruption?: boolean;
  /** Force the LLM adapter kind; omit for OpenAI-compatible canonical-host inference. */
  providerKind?: AdapterKind;
  /** Strip markdown from spoken text (gateway default true). */
  stripMarkdown?: boolean;

  // --- Barge-in / mute (A-G group) ---
  /** Eager end-of-turn: start the LLM speculatively on a turn-complete prediction. */
  eagerEot?: boolean;
  /** While the bot speaks, require >= N words to interrupt it (gateway clamps >=2). */
  bargeInMinWords?: number;
  /** Which audio to mute/ignore while the bot speaks. */
  muteStrategy?: MuteStrategy;
  /** Speak a re-engagement prompt after the user is idle this many ms (0 = off). */
  userIdleTimeoutMs?: number;
  /** Summarize older history once it exceeds this many tokens (0 = off). */
  summarizeTargetTokens?: number;

  // --- Latency filler (D3) ---
  /** Latency-masking mode for slow first audio (gateway default auto). */
  latencyFiller?: LatencyFiller;
  /** Only emit a filler if first audio is slower than this many ms. */
  latencyFillerAfterMs?: number;
  /** Custom filler phrases to choose from (overrides the built-in set). */
  latencyFillerPhrases?: string[];

  // --- Reasoning tier (D1/S1/S2 — two-tier) ---
  /** Reasoning/thinking-effort dial. */
  reasoningEffort?: ReasoningEffort;
  /** Optional slow REASONING-tier model — the field that turns two-tier ON (keep `model` fast). */
  reasoningModel?: string;
  /** Base URL for the reasoning model (defaults to baseUrl). */
  reasoningBaseUrl?: string;
  /** API key for the reasoning model (defaults to apiKey). */
  reasoningApiKey?: string;
  /** Force the reasoning adapter kind; omit for OpenAI-compatible inference. */
  reasoningProviderKind?: AdapterKind;
  /** When to route to the reasoning tier (gateway default auto). */
  reasoningRoute?: RoutingMode;
  /** Max ms to wait on the reasoning tier before falling back (default 15000, 0 = off). */
  reasoningBudgetMs?: number;
  /** Spoken when all tiers fail. */
  degradationMessage?: string;
  /** Cap LLM calls per turn (default 8, floored >=1). */
  maxLlmCallsPerTurn?: number;
  /** Cap reasoning-tier output tokens. */
  maxReasoningTokens?: number;
}

export interface FeatureFlags {
  vad?: boolean;
  noiseCancellation?: boolean;
  speakerDiarization?: boolean;
  interimResults?: boolean;
  punctuation?: boolean;
  profanityFilter?: boolean;
  smartFormat?: boolean;
  echoCancellation?: boolean;
}

export interface TranscriptResult {
  text: string;
  isFinal: boolean;
  confidence?: number;
  speakerId?: number;
}

/**
 * A typed gateway advisory surfaced from an `OutgoingMessage::ConfigWarning`
 * wire frame (gateway handlers/ws/messages.rs + config_lint.rs).
 *
 * The gateway deserializes the `config` envelope leniently: an unknown key (a
 * typo like `diarizationn`, or a wrong-nesting mistake like `turn_detection` at
 * the top level instead of under `stt_config`) is silently DROPPED — to the
 * client that looks exactly like success even though the feature never took
 * effect. Rather than fail forward-compat, the gateway emits this non-fatal
 * advisory naming every ignored key (plus targeted nesting hints), and the
 * widget surfaces it as a `configWarning` event so a developer LEARNS the
 * config was partially ignored instead of debugging a silent no-op.
 *
 * `config_warning` NEVER closes the session; it is informational.
 *
 * Known `code` values:
 *  - `"unknown_config_keys"` — keys serde dropped (typo / wrong nesting). The
 *    `detail.ignored_keys` array lists the dotted paths.
 *  - `"reasoning_model_on_voice_path"` — a reasoning model was placed on the
 *    spoken path (high time-to-first-audio).
 *  - `"reasoning_effort_clamped"` — effort was clamped up to a model's floor.
 *  - `"emotion_ignored_for_provider"` / `"language_unsupported"` — a graceful
 *    no-op (e.g. emotion sent to a provider that ignores it) surfaced loudly.
 */
export interface ConfigWarning {
  /** Stable machine-readable code (e.g. "unknown_config_keys"). */
  code: string;
  /** Human-readable explanation + a one-line fix hint. */
  message: string;
  /** Optional free-form detail (e.g. `{ ignored_keys: ["stt_config.features.diarizationn"] }`). */
  detail?: Record<string, unknown>;
}

export interface AudioChunk {
  audio: ArrayBuffer;
  format: string;
  sampleRate: number;
  isFinal?: boolean;
}

export interface WidgetMetrics {
  sttTtft?: number;
  ttsTtfb?: number;
  e2eLatency?: number;
  messagesReceived: number;
  messagesSent: number;
}

export type WidgetState = 'idle' | 'connecting' | 'connected' | 'listening' | 'speaking' | 'error';

export interface WidgetEventMap {
  'ready': CustomEvent<{ streamId: string }>;
  'transcript': CustomEvent<TranscriptResult>;
  'audio': CustomEvent<AudioChunk>;
  'stateChange': CustomEvent<{ state: WidgetState; previousState: WidgetState }>;
  'metrics': CustomEvent<WidgetMetrics>;
  /** Non-fatal gateway advisory: a config key was ignored / a feature is a no-op. */
  'configWarning': CustomEvent<ConfigWarning>;
  /** D10: the mic went silent for ~0.5s (likely OS-muted / dead device). */
  'micSilence': CustomEvent<{ message: string }>;
  'error': CustomEvent<Error>;
}
