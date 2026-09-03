import type { DAGConfig } from './dag.js';
import type { TurnDetectionConfig } from './audio-features.js';
import type { ConversationConfig } from './conversation.js';

// Re-export so the full config surface is importable from one place.
export type { DAGConfig } from './dag.js';
export type { TurnDetectionConfig } from './audio-features.js';
export type {
  ConversationConfig,
  AdapterKind,
  ReasoningEffort,
  LatencyFiller,
  RoutingMode,
  MuteStrategy,
} from './conversation.js';

// =============================================================================
// Emotion Types (Unified Emotion System)
// =============================================================================

/**
 * Standardized emotions supported across TTS providers.
 * Each emotion maps to provider-specific formats (SSML, audio tags, natural language, etc.)
 */
/**
 * Mirrors the gateway `Emotion` value space 1:1 (gateway/src/core/emotion/types.rs;
 * widened 22 → 44 in P4). A bare `string` is also accepted on the wire so an
 * unrecognized token is forwarded + resolved server-side (never a hard error);
 * old variants are NEVER removed (the gateway resolves them).
 */
export type Emotion =
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
  // P4 widened affective states (each has a direct provider token).
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
 * Delivery styles that modify how speech is expressed.
 * These can be combined with emotions for nuanced expression.
 *
 * Mirrors the gateway `DeliveryStyle` value space. P4 PROMOTED the affective words
 * `soft`/`cheerful`/`serious` to {@link Emotion}, but the gateway still accepts them
 * here as backward-compatible cross-enum aliases, so those old variants are KEPT.
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
  // P4 scenario / narration framings (Azure express-as / AWS Polly amazon:domain).
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

/**
 * Emotion intensity presets.
 * - 'low': Subtle emotion (0.3 intensity)
 * - 'medium': Moderate emotion (0.6 intensity)
 * - 'high': Strong emotion (1.0 intensity)
 */
export type EmotionIntensityLevel = 'low' | 'medium' | 'high';

/**
 * Emotion configuration for TTS.
 * Supports both the unified emotion system and free-form descriptions.
 */
export interface EmotionConfig {
  /** Primary emotion to express */
  emotion?: Emotion;
  /** Emotion intensity (0.0 to 1.0 or preset level) */
  intensity?: number | EmotionIntensityLevel;
  /** Delivery style */
  style?: DeliveryStyle;
  /** Free-form description (for providers like Hume that support natural language) */
  description?: string;
}

// =============================================================================
// Voice Descriptor (P4 — abstract voice selection; resolved server-side)
// =============================================================================

/** Desired voice gender for {@link VoiceDescriptor} (gateway `Gender`). */
export type VoiceGender = 'male' | 'female' | 'neutral';

/** Desired voice age band for {@link VoiceDescriptor} (gateway `Age`). */
export type VoiceAge = 'young' | 'middle_aged' | 'old';

/**
 * Abstract, provider-agnostic voice selection (P4).
 *
 * Mirrors the gateway `VoiceDescriptor` (gateway/src/core/voice/types.rs). Describe
 * the voice you WANT (gender / locale / accent / age / style / name hint) and the
 * gateway resolves it to a concrete provider `voiceId` server-side — so the same
 * descriptor works across every TTS provider without hardcoding IDs.
 *
 * Used ONLY when no explicit `voiceId` (or `voice`) is set on the TTS config — a raw
 * `voiceId` ALWAYS wins. Serialized under `tts_config.voice_descriptor` on the wire
 * (snake_case object); every field is optional and only sent when set.
 */
export interface VoiceDescriptor {
  /** Desired gender: `male` | `female` | `neutral`. */
  gender?: VoiceGender;
  /** BCP-47 locale (preferred): `en-US`, `en-GB`, `hi-IN`, … */
  locale?: string;
  /** Free accent string when no `locale` is given: `american`, `british`, … */
  accent?: string;
  /** Desired age band: `young` | `middle_aged` | `old`. */
  age?: VoiceAge;
  /** Free timbre/style matched against provider metadata: `warm`, `bright`, … */
  style?: string;
  /** Optional preferred voice name (`asteria`, `jenny`); substring-matched. */
  nameHint?: string;
}

/**
 * Canonical, provider-agnostic in-stream/batch translation request (P5).
 *
 * Reuses P2's canonical language tokens ({@link CanonicalLanguage}). Lives as
 * `STTConfig.translation` (nested under `stt_config.translation` on the wire) and
 * on the batch envelope; the gateway emits a uniform `translations:[{lang,text}]`
 * array merged onto the transcript event regardless of provider, degrading with a
 * `config_warning` (never a 400) where a provider lacks streaming translation.
 *
 * Provider classes:
 *  - Class A (arbitrary targets, side-channel): Speechmatics
 *    (`translation_config.target_languages`, MAX 5), Gladia
 *    (`realtime_processing.translation`), AssemblyAI (batch only).
 *  - Class B (English-only fast path): OpenAI/Groq `/audio/translations` — set
 *    `translateToEnglish: true`.
 */
export interface TranslationConfig {
  /**
   * Canonical target languages (region-qualified BCP-47, e.g. `["es-ES","de-DE"]`).
   * Mapped to each provider's native codes server-side; capped per-provider
   * (Speechmatics MAX 5 → warn + truncate).
   */
  targetLanguages?: string[];
  /**
   * Fast path: translate the whole stream to ENGLISH (OpenAI/Groq
   * `/audio/translations`). For Class-A providers this is sugar for
   * `targetLanguages: ["en-US"]`.
   */
  translateToEnglish?: boolean;
  /**
   * Emit partial (interim) translations where supported (Speechmatics
   * `enable_partials` / Gladia live). Omit for provider default (finals only).
   */
  partials?: boolean;
}

/**
 * STT (Speech-to-Text) Configuration
 * Maps to the gateway STTWebSocketConfig in src/handlers/ws/config.rs
 */
export interface STTConfig {
  /** Provider name (e.g., "deepgram", "google", "elevenlabs", "microsoft-azure", "cartesia") */
  provider: string;
  /** Language code for transcription (e.g., "en-US", "es-ES") */
  language?: string;
  /** Sample rate of the audio in Hz (default: 16000) */
  sampleRate?: number;
  /** Number of audio channels (1 for mono, 2 for stereo, default: 1) */
  channels?: number;
  /** Enable punctuation in results (default: true) */
  punctuation?: boolean;
  /** Alias for punctuation (Deepgram style) */
  punctuate?: boolean;
  /** Encoding of the audio (default: "linear16") */
  encoding?: string;
  /**
   * D8 uplink transport codec the gateway decodes before STT: `"linear16"` (default) | `"opus"`.
   * Prefer the higher-level capability negotiation (the SDK sets this only when it can encode opus
   * via WebCodecs); the gateway echoes the effective codec in `ready` and degrades to linear16 if
   * its build lacks opus. Distinct from {@link encoding}.
   */
  audioInCodec?: 'linear16' | 'opus';
  /** Model to use for transcription (e.g., "nova-2", "nova-3") */
  model?: string;
  /** Enable interim/partial results */
  interimResults?: boolean;
  /** Enable profanity filter */
  profanityFilter?: boolean;
  /** Enable smart formatting */
  smartFormat?: boolean;
  /** Enable speaker diarization */
  diarize?: boolean;
  /** Keywords to boost recognition */
  keywords?: string[];
  /** Custom vocabulary */
  customVocabulary?: string[];
  /** Endpointing settings */
  endpointing?: number;
  /** Utterance end timeout in ms */
  utteranceEndMs?: number;

  // ---------------------------------------------------------------------------
  // Advanced STT features — 1:1 with the gateway canonical `SttFeatures`
  // (gateway/src/core/stt/standard.rs). These nest under `stt_config.features{}`
  // on the wire (see configToWire); flat-on-top was silently dropped pre-P1.
  // ---------------------------------------------------------------------------
  /** Per-word timestamps in the transcript. */
  wordTimestamps?: boolean;
  /** Emit filler words ("um", "uh") instead of suppressing them. */
  fillerWords?: boolean;
  /** Emit explicit VAD speech-start/stop events. */
  vadEvents?: boolean;
  /** Silence (ms) after speech before finalizing an utterance. */
  endpointingMs?: number;
  /** Hard cap (ms) on how long an utterance may run before a forced finalize. */
  utteranceEndMsFeature?: number;
  /** Boost recognition of these key terms / phrases (canonical `keyterms`). */
  keyterms?: string[];
  /** PII entity classes to redact from the transcript. */
  redaction?: string[];
  /** Auto-detect the spoken language. */
  languageDetection?: boolean;
  /** Tag named entities in the transcript. */
  entityDetection?: boolean;
  /** Render numerals as digits ("twenty" → "20"). */
  numerals?: boolean;
  /** Transcribe each audio channel independently. */
  multichannel?: boolean;
  /** Number of alternative transcripts to return (0-255). */
  alternatives?: number;
  /** Per-utterance sentiment analysis. */
  sentiment?: boolean;
  /** Emit a speech-begin event when speech first starts. */
  speechBeginEvent?: boolean;

  /**
   * Open passthrough forwarded verbatim into the gateway `extras` map (never
   * flagged by the gateway config linter — the escape hatch for provider-only
   * knobs with no canonical field, e.g. `custom_vocabulary`).
   */
  extras?: Record<string, unknown>;
  /** Per-session API key override (literal or '${ENV}'). */
  apiKey?: string;

  /**
   * Canonical in-stream/batch translation (P5). When set, the gateway emits a
   * uniform `translations:[{lang,text}]` array merged onto the transcript;
   * unsupported providers degrade with a `config_warning` (never a 400). Nests
   * under `stt_config.translation` on the wire (see `sttConfigToWire`).
   */
  translation?: TranslationConfig;
}

/**
 * TTS (Text-to-Speech) Configuration
 * Maps to the gateway TTSWebSocketConfig in src/handlers/ws/config.rs
 */
export interface TTSConfig {
  /** Provider name (e.g., "deepgram", "elevenlabs", "google", "azure", "cartesia") */
  provider: string;
  /** Voice name */
  voice?: string;
  /** Voice ID or name to use for synthesis */
  voiceId?: string;
  /**
   * Abstract voice selection (gender/locale/accent/age/style/nameHint) resolved to
   * a concrete provider `voiceId` server-side (P4). Used ONLY when no `voiceId`
   * /`voice` is set — a raw `voiceId` always wins. Serialized under
   * `tts_config.voice_descriptor` on the wire.
   */
  voiceDescriptor?: VoiceDescriptor;
  /** Speaking rate (0.25 to 4.0, 1.0 is normal) */
  speakingRate?: number;
  /** Alias for speakingRate */
  speed?: number;
  /** Pitch adjustment */
  pitch?: number;
  /** Volume adjustment */
  volume?: number;
  /** Audio format preference (e.g., "linear16", "mp3", "wav") */
  audioFormat?: string;
  /** Sample rate preference (e.g., 16000, 24000, 48000) */
  sampleRate?: number;
  /** Connection timeout in seconds */
  connectionTimeout?: number;
  /** Request timeout in seconds */
  requestTimeout?: number;
  /** Model to use for TTS */
  model?: string;
  /** Pronunciation replacements to apply before TTS */
  pronunciations?: Pronunciation[];
  /** Voice stability (ElevenLabs specific, 0-1) */
  stability?: number;
  /** Voice similarity boost (ElevenLabs specific, 0-1) */
  similarityBoost?: number;
  /** Voice style (ElevenLabs specific, 0-1) */
  style?: number;
  /** Use speaker boost (ElevenLabs specific) */
  useSpeakerBoost?: boolean;

  // Emotion settings (Unified Emotion System)
  /** Primary emotion to express */
  emotion?: Emotion;
  /** Emotion intensity (0.0 to 1.0 or preset level) */
  emotionIntensity?: number | EmotionIntensityLevel;
  /** Delivery style */
  deliveryStyle?: DeliveryStyle;
  /** Free-form emotion description (for Hume and other natural language providers) */
  emotionDescription?: string;

  // Hume-specific settings
  /** Acting instructions for Hume Octave (max 100 chars, e.g., "whispered fearfully") */
  actingInstructions?: string;
  /** Voice description for Hume voice design */
  voiceDescription?: string;
  /** Trailing silence in seconds (Hume) */
  trailingSilence?: number;
  /** Enable instant mode for lower latency (Hume) */
  instantMode?: boolean;

  // ---------------------------------------------------------------------------
  // Gateway egress / timing knobs (top-level on TTSWebSocketConfig, config.rs).
  // ---------------------------------------------------------------------------
  /** WS egress reframing: emit audio in chunks of this many ms. */
  audioOutChunkMs?: number;
  /** Resample the egress audio to this rate for client playback. */
  clientPlaybackRate?: number;
  /**
   * D8 downlink transport codec for TTS egress: `"linear16"` (default) | `"opus"`. The gateway
   * encodes each PCM chunk to one opus packet per binary frame; the SDK decodes before playout.
   * Prefer the higher-level capability negotiation (set only when the SDK can decode opus via
   * WebCodecs); the gateway echoes the effective codec in `ready` and degrades to linear16 otherwise.
   */
  audioOutCodec?: 'linear16' | 'opus';
  /** Per-session API key override (literal or '${ENV}'). */
  apiKey?: string;

  // ---------------------------------------------------------------------------
  // Advanced TTS features — 1:1 with the gateway canonical `TtsFeatures`
  // (gateway/src/core/tts/standard.rs). These nest under `tts_config.features{}`
  // on the wire (see configToWire). `speed`/`pitch`/`volume`/`stability`/
  // `similarityBoost`/`style`/`useSpeakerBoost` above are ALSO mirrored here.
  // ---------------------------------------------------------------------------
  /** Natural-language acting instructions (canonical `instructions`; OpenAI/Hume). */
  instructions?: string;
  /** Treat the input text as SSML markup. */
  ssml?: boolean;
  /** Language hint for the TTS voice (provider-native code). */
  language?: string;
  /** Per-word timestamps in the synthesized audio stream. */
  wordTimestamps?: boolean;
  /** Stream audio as it is generated (vs. one-shot). */
  streaming?: boolean;
  /** Deterministic synthesis seed. */
  seed?: number;
  /** Latency/quality trade-off knob (ElevenLabs `optimize_streaming_latency`, 0-4). */
  optimizeStreamingLatency?: number;
  /** Which timestamp granularities to include (e.g. ["word","character"]). */
  includeTimestampTypes?: string[];
  /** Speaking-rate delta as a percentage (-100..100). */
  ratePercentage?: number;
  /** Pitch delta as a percentage (-100..100). */
  pitchPercentage?: number;

  /**
   * Open passthrough forwarded verbatim into the gateway `extras` map (never
   * flagged by the gateway config linter — the escape hatch for provider-only
   * knobs with no canonical field).
   */
  extras?: Record<string, unknown>;
}

/**
 * Pronunciation replacement rule
 */
export interface Pronunciation {
  /** Text pattern to match */
  from: string;
  /** Replacement text */
  to: string;
}

/**
 * LiveKit Configuration
 * Maps to the gateway LiveKitWebSocketConfig in src/handlers/ws/config.rs
 */
export interface LiveKitConfig {
  /** Room name to join or create */
  roomName: string;
  /** Enable recording for this session */
  enableRecording?: boolean;
  /** WaaV AI participant identity (defaults to "waav-ai") */
  waavParticipantIdentity?: string;
  /** WaaV AI participant display name (defaults to "WaaV AI") */
  waavParticipantName?: string;
  /** List of participant identities to listen to for audio tracks and data messages */
  listenParticipants?: string[];
}

/**
 * Complete WebSocket session configuration — a 1:1 mirror of the gateway `/ws`
 * config envelope (IncomingMessage::Config, gateway/src/handlers/ws/messages.rs).
 *
 * Every gateway config sub-block is reachable from here:
 *   - sttConfig / ttsConfig / livekit / dag / conversation
 *   - turnDetection (nested into stt_config.turn_detection on the wire)
 *
 * A drift-guard test (tests/unit/config-drift.test.ts) asserts this field set
 * covers the openapi schema's config properties, so a new gateway field can
 * never become silently unreachable again.
 */
export interface SessionConfig {
  /** Optional unique identifier for this WebSocket session (wire: stream_id). */
  streamId?: string;
  /** Enable audio processing (STT/TTS). Defaults to true. */
  audio?: boolean;
  /** STT configuration (required when audio=true). */
  sttConfig?: STTConfig;
  /** TTS configuration (required when audio=true). */
  ttsConfig?: TTSConfig;
  /** LiveKit configuration for real-time audio streaming. */
  livekit?: LiveKitConfig;
  /** DAG routing configuration (named template or inline definition). */
  dag?: DAGConfig;
  /**
   * Built-in conversation/agent loop (LLM + reasoning + barge-in + latency
   * filler). Serialized into `conversation_config`. This is the flagship
   * surface; `bud.agent(...)` is the easy way to build it.
   */
  conversation?: ConversationConfig;
  /**
   * ML turn-detection. Nests into `stt_config.turn_detection` on the wire (its
   * real home), NOT a top-level key.
   */
  turnDetection?: TurnDetectionConfig;
}

/**
 * Default STT configuration values
 */
export const DEFAULT_STT_CONFIG: Partial<STTConfig> = {
  sampleRate: 16000,
  channels: 1,
  punctuation: true,
  encoding: 'linear16',
};

/**
 * Default TTS configuration values
 */
export const DEFAULT_TTS_CONFIG: Partial<TTSConfig> = {
  sampleRate: 24000,
  audioFormat: 'linear16',
  speakingRate: 1.0,
};

/**
 * Create a complete STT config with defaults
 */
export function createSTTConfig(config: Partial<STTConfig> & Pick<STTConfig, 'provider' | 'language' | 'model'>): STTConfig {
  return {
    ...DEFAULT_STT_CONFIG,
    ...config,
  } as STTConfig;
}

/**
 * Create a complete TTS config with defaults
 */
export function createTTSConfig(config: Partial<TTSConfig> & Pick<TTSConfig, 'provider' | 'model'>): TTSConfig {
  return {
    ...DEFAULT_TTS_CONFIG,
    ...config,
  } as TTSConfig;
}

// =============================================================================
// Voice Cloning Types
// =============================================================================

/**
 * Provider for voice cloning operations.
 */
export type VoiceCloneProvider = 'hume' | 'elevenlabs';

/**
 * Request to clone a voice from audio samples or description.
 */
export interface VoiceCloneRequest {
  /** Provider to use for voice cloning */
  provider: VoiceCloneProvider;
  /** Name for the cloned voice */
  name: string;
  /** Description of the voice (used by Hume for voice design) */
  description?: string;
  /** Audio samples for cloning (base64-encoded). ElevenLabs: 1-2 min recommended */
  audioSamples?: string[];
  /** Sample text for voice generation (Hume only) */
  sampleText?: string;
  /** Remove background noise from samples (ElevenLabs only) */
  removeBackgroundNoise?: boolean;
  /** Labels for the voice (ElevenLabs only) */
  labels?: Record<string, string>;
}

/**
 * Response from voice cloning operation.
 */
export interface VoiceCloneResponse {
  /** Unique identifier for the cloned voice */
  voiceId: string;
  /** Name of the cloned voice */
  name: string;
  /** Provider that created the voice */
  provider: VoiceCloneProvider;
  /** Status of the voice (ready, processing, failed) */
  status: 'ready' | 'processing' | 'failed';
  /** ISO 8601 timestamp when the voice was created */
  createdAt: string;
  /** Additional metadata from the provider */
  metadata?: Record<string, unknown>;
}

// =============================================================================
// Hume EVI (Empathic Voice Interface) Types
// =============================================================================

/**
 * Hume EVI version.
 */
export type HumeEVIVersion = '1' | '2' | '3' | '4-mini';

/**
 * Hume EVI configuration for audio-to-audio realtime streaming.
 */
export interface HumeEVIConfig {
  /** EVI configuration ID from Hume dashboard */
  configId?: string;
  /** Chat group ID for resuming a previous conversation */
  resumedChatGroupId?: string;
  /** EVI version to use (default: '3') */
  eviVersion?: HumeEVIVersion;
  /** Voice ID to use */
  voiceId?: string;
  /** Enable verbose transcription */
  verboseTranscription?: boolean;
  /** System prompt override */
  systemPrompt?: string;
}

/**
 * Prosody (emotion) scores from Hume EVI.
 * Provides emotion dimensions detected in speech.
 *
 * Note: Hume's prosody API documents 48 core emotion dimensions, but the exact
 * fields may vary. Missing fields will be 0 when not returned by the API.
 */
export interface ProsodyScores {
  admiration: number;
  adoration: number;
  aestheticAppreciation: number;
  amusement: number;
  anger: number;
  anxiety: number;
  awe: number;
  awkwardness: number;
  boredom: number;
  calmness: number;
  concentration: number;
  confusion: number;
  contemplation: number;
  contempt: number;
  contentment: number;
  craving: number;
  desire: number;
  determination: number;
  disappointment: number;
  disgust: number;
  distress: number;
  doubt: number;
  ecstasy: number;
  embarrassment: number;
  empathicPain: number;
  enthusiasm: number;
  entrancement: number;
  envy: number;
  excitement: number;
  fear: number;
  gratitude: number;
  guilt: number;
  horror: number;
  interest: number;
  joy: number;
  love: number;
  nostalgia: number;
  pain: number;
  pride: number;
  realization: number;
  relief: number;
  romance: number;
  sadness: number;
  satisfaction: number;
  shame: number;
  surpriseNegative: number;
  surprisePositive: number;
  sympathy: number;
  tiredness: number;
  triumph: number;
}

/**
 * Get the top N emotions from prosody scores.
 */
export function getTopEmotions(
  scores: ProsodyScores,
  n: number = 3
): Array<{ name: string; score: number }> {
  const entries = Object.entries(scores) as Array<[string, number]>;
  return entries
    .sort((a, b) => b[1] - a[1])
    .slice(0, n)
    .map(([name, score]) => ({ name, score }));
}

/**
 * Get the dominant emotion from prosody scores.
 */
export function getDominantEmotion(
  scores: ProsodyScores
): { name: string; score: number } | null {
  const top = getTopEmotions(scores, 1);
  return top[0] ?? null;
}

/**
 * Map intensity level to numeric value.
 */
export function intensityToNumber(intensity: number | EmotionIntensityLevel): number {
  if (typeof intensity === 'number') {
    return Math.max(0, Math.min(1, intensity));
  }
  switch (intensity) {
    case 'low':
      return 0.3;
    case 'medium':
      return 0.6;
    case 'high':
      return 1.0;
    default:
      return 0.6;
  }
}
