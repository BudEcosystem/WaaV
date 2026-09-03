//! WebSocket configuration types and handlers
//!
//! This module contains all configuration-related types for WebSocket connections,
//! including STT, TTS, and LiveKit configurations without API keys.

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_128;

use crate::{
    core::{
        emotion::{DeliveryStyle, Emotion, EmotionConfig, EmotionIntensity},
        stt::STTConfig,
        tts::{Pronunciation, TTSConfig},
    },
    livekit::LiveKitConfig,
};

/// Normalize a client-supplied BYOK value.
///
/// `None`, empty strings, and whitespace-only strings mean "fall back to server
/// config"; non-empty values are trimmed before being forwarded to providers.
pub(crate) fn client_api_key(api_key: Option<&str>) -> Option<String> {
    api_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

/// DAG configuration for WebSocket messages
///
/// Allows clients to specify a DAG pipeline for audio processing.
/// The DAG can be specified either by template name or inline definition.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DAGWebSocketConfig {
    /// Name of a pre-registered DAG template to use
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "voice-assistant"))]
    pub template: Option<String>,

    /// Inline DAG definition (takes precedence over template)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<serde_json::Value>,

    /// Enable DAG metrics collection
    #[serde(default)]
    pub enable_metrics: bool,

    /// Maximum execution timeout in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = 30000))]
    pub timeout_ms: Option<u64>,
}

/// Conversation-loop configuration for WebSocket messages (plan W-O2).
///
/// When present on a `config` message, the gateway wires up a built-in
/// automatic conversation loop: each finalized STT turn is sent to an
/// OpenAI-compatible LLM and the reply is streamed to TTS, with per-session
/// history and barge-in. When absent, the gateway keeps its raw STT/TTS
/// behavior (fully backward-compatible).
#[derive(Debug, Deserialize, Serialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConversationWebSocketConfig {
    /// OpenAI-compatible base URL for the LLM (e.g. `https://api.openai.com/v1`).
    #[cfg_attr(feature = "openapi", schema(example = "https://api.openai.com/v1"))]
    pub base_url: String,

    /// Model identifier.
    #[cfg_attr(feature = "openapi", schema(example = "gpt-4o-mini"))]
    pub model: String,

    /// Optional system prompt seeding the conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// API key (literal or `${ENV_VAR}`); falls back to `OPENAI_API_KEY`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Max tokens per completion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Stream tokens to TTS as they arrive (default true).
    #[serde(default = "default_conversation_streaming")]
    pub streaming: bool,

    /// Max retained history messages (default 20).
    #[serde(default = "default_conversation_max_history")]
    pub max_history: usize,

    /// Whether the bot's speech is interruptible / barge-in (default true).
    #[serde(default = "default_conversation_allow_interruption")]
    pub allow_interruption: bool,

    /// Eager end-of-turn (P1.2b): start the LLM speculatively on a
    /// turn-complete prediction, confirm/cancel on the provider final.
    /// Opt-in — raises LLM call volume on resumed turns (default false).
    #[serde(default)]
    pub eager_eot: bool,

    /// LLM vendor wire format (B-G1): `"openai"` (default) | `"anthropic"` |
    /// `"gemini"`. Omitted = OpenAI-compatible, with canonical-host inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, example = "anthropic"))]
    pub provider_kind: Option<crate::core::llm::AdapterKind>,

    /// MinWords barge-in gate (A-G3): while the bot is audibly speaking,
    /// require ≥ N words to interrupt it (a single word suffices when the
    /// bot is silent). Omitted/0 = legacy any-speech barge-in; values < 2
    /// are clamped to 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barge_in_min_words: Option<usize>,

    /// Token-aware context compaction (B-G6): when the session context's
    /// estimated tokens cross this value, older messages are summarized
    /// after the turn (system + recent turns kept verbatim). Omitted/0 =
    /// off.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = 6000))]
    pub summarize_target_tokens: usize,

    /// User-mute strategy (A-G5): while active, USER INPUT is suppressed
    /// (bot/lifecycle signals always flow).
    /// `"always_while_bot_speaks"` = no barge-in at all;
    /// `"until_first_bot_complete"` = greeting/disclaimer guard;
    /// `"first_speech_only"` = only the first user utterance passes.
    /// Omitted = no muting (barge-in works normally).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "until_first_bot_complete"))]
    pub mute_strategy: Option<String>,

    /// Strip markdown (bold/code/links/headings) from LLM sentences before
    /// TTS (C-G4). Default true — spoken asterisks and URLs ruin voice
    /// output.
    #[serde(default = "default_strip_markdown")]
    pub strip_markdown: bool,

    /// Idle re-engagement (A-G7): after this many ms of silence (no user
    /// speech, no bot turn, bot not speaking), the bot gently checks in.
    /// Omitted/0 = off.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = 15000))]
    pub user_idle_timeout_ms: u64,

    /// D1 (REALTIME_REASONING.md §4.1): reasoning/thinking-effort dial —
    /// `off | minimal | low | medium | high`. One typed knob, mapped to each
    /// vendor's native thinking control and clamped to the model's floor (an
    /// adaptive-only model can't go below its floor; the applied/floor values
    /// are echoed back in the config-ack). Omitted = vendor default. Keep a
    /// FAST, non-reasoning model on the spoken path for realtime latency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "minimal"))]
    pub reasoning_effort: Option<crate::core::llm::ReasoningEffort>,

    /// D3 (REALTIME_REASONING.md §4.3): latency-masking mode —
    /// `off | auto | aggressive`. `auto` (default) speaks ONE short action phrase
    /// when first audio is slow (reasoning/RAG/tool), keeping the line alive while
    /// the real answer streams in behind it — at most one masking utterance per
    /// turn, codec- & language-correct, interruptible by barge-in.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = "auto"))]
    pub latency_filler: crate::core::conversation::LatencyFiller,
    /// D3: override the masking wait threshold in ms (mode default otherwise:
    /// auto ~800, aggressive ~400). Keep well under the ~2s "feels broken" line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = 800))]
    pub latency_filler_after_ms: Option<u64>,
    /// D3: custom masking phrases (action wording, e.g. "Let me check that order").
    /// Empty = the built-in pool. Pre-rendered at session start at the call's rate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub latency_filler_phrases: Vec<String>,

    /// S1/S2 (REALTIME_REASONING.md §5): optional slow REASONING tier. Set this
    /// to a smart-but-slow model (e.g. `o3`, `deepseek-r1`) and keep `model` a
    /// FAST model — complex turns escalate to the reasoning tier (sharing the
    /// conversation history) while the fast model handles the rest and the D3
    /// filler masks the latency. Omitted = single-tier. The ONE field that turns
    /// two-tier on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "o3"))]
    pub reasoning_model: Option<String>,
    /// S1/S2: reasoning-tier base URL (defaults to `base_url` — one endpoint can
    /// serve both a fast and a reasoning model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_base_url: Option<String>,
    /// S1/S2: reasoning-tier API key (literal or `${ENV_VAR}`; defaults to `api_key`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_api_key: Option<String>,
    /// S1/S2: reasoning-tier vendor wire format (defaults to `provider_kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, example = "openai"))]
    pub reasoning_provider_kind: Option<crate::core::llm::AdapterKind>,
    /// S2: route turns between tiers — `auto` (default; a heuristic escalates only
    /// complex turns) or `always` (every turn uses the reasoning tier).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = "auto"))]
    pub reasoning_route: crate::core::conversation::RoutingMode,
    /// P1+A7: reasoning-tier max-silence-gap budget (ms) — the longest the line
    /// may go silent before first audio OR between audio chunks. If exceeded, a
    /// reasoner with partial audio commits it (no restart) and one with none
    /// degrades to the fast tier — the caller is never left in silence. Omit for
    /// the safe default (15000); `0` disables. Ignored without a `reasoning_model`.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = 15000))]
    pub reasoning_budget_ms: Option<u64>,
    /// P1: the spoken line used when EVERY LLM tier fails — a graceful apology
    /// instead of dead air. Omit for the built-in default.
    #[serde(default)]
    pub degradation_message: Option<String>,
    /// P2: per-turn ceiling on LLM re-inference rounds (the tool-call loop — the
    /// dominant spend multiplier on a billing gateway). Omit for the safe default
    /// (8).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = 8))]
    pub max_llm_calls_per_turn: Option<u32>,
    /// P2: hard ceiling on the reasoning tier's output tokens (thinking + answer)
    /// — the most direct cost lever for reasoning models. Clamps the reasoning
    /// tier's `max_tokens`; the fast tier is unaffected. Omit for no extra clamp.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = 2048))]
    pub max_reasoning_tokens: Option<u32>,
}

fn default_strip_markdown() -> bool {
    true
}

fn default_conversation_streaming() -> bool {
    true
}

fn default_conversation_max_history() -> usize {
    20
}

fn default_conversation_allow_interruption() -> bool {
    true
}

impl ConversationWebSocketConfig {
    /// Convert into the core `ConversationConfig`.
    pub fn to_conversation_config(&self) -> crate::core::conversation::ConversationConfig {
        crate::core::conversation::ConversationConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            api_key: self.api_key.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            streaming: self.streaming,
            max_history: self.max_history,
            allow_interruption: self.allow_interruption,
            eager_eot: self.eager_eot,
            provider_kind: self.provider_kind,
            barge_in_min_words: self.barge_in_min_words,
            summarize_target_tokens: self.summarize_target_tokens,
            mute_strategy: self.mute_strategy.clone(),
            strip_markdown: self.strip_markdown,
            user_idle_timeout_ms: self.user_idle_timeout_ms,
            reasoning_effort: self.reasoning_effort,
            latency_filler: self.latency_filler,
            latency_filler_after_ms: self.latency_filler_after_ms,
            latency_filler_phrases: self.latency_filler_phrases.clone(),
            reasoning_model: self.reasoning_model.clone(),
            reasoning_base_url: self.reasoning_base_url.clone(),
            reasoning_api_key: self.reasoning_api_key.clone(),
            reasoning_provider_kind: self.reasoning_provider_kind,
            reasoning_route: self.reasoning_route,
            reasoning_budget_ms: self
                .reasoning_budget_ms
                .unwrap_or(crate::core::conversation::DEFAULT_REASONING_BUDGET_MS),
            degradation_message: self.degradation_message.clone(),
            // A pure tool-call turn needs ≥1 round to execute its tools and
            // re-infer; 0 would silently abort every tool turn into dead air, so
            // floor an explicit value at 1 (mirrors the barge_in_min_words clamp).
            max_llm_calls_per_turn: self
                .max_llm_calls_per_turn
                .map(|n| n.max(1))
                .unwrap_or(crate::core::conversation::DEFAULT_MAX_LLM_CALLS_PER_TURN),
            max_reasoning_tokens: self.max_reasoning_tokens,
        }
    }
}

/// Default value for audio enabled flag (true)
pub fn default_audio_enabled() -> Option<bool> {
    Some(true)
}

/// Default value for allow_interruption flag (true)
pub fn default_allow_interruption() -> Option<bool> {
    Some(true)
}

/// Default STT audio encoding (`linear16`, i.e. 16-bit PCM) when a client omits `encoding`.
pub fn default_stt_encoding() -> String {
    "linear16".to_string()
}

/// STT configuration for WebSocket messages (with optional API key)
#[derive(Debug, Deserialize, Serialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct STTWebSocketConfig {
    /// Provider name (e.g., "deepgram")
    #[cfg_attr(feature = "openapi", schema(example = "deepgram"))]
    pub provider: String,
    /// Language code for transcription (e.g., "en-US", "es-ES")
    #[cfg_attr(feature = "openapi", schema(example = "en-US"))]
    pub language: String,
    /// Sample rate of the audio in Hz
    #[cfg_attr(feature = "openapi", schema(example = 16000))]
    pub sample_rate: u32,
    /// Number of audio channels (1 for mono, 2 for stereo)
    #[cfg_attr(feature = "openapi", schema(example = 1))]
    pub channels: u16,
    /// Enable punctuation in results
    #[cfg_attr(feature = "openapi", schema(example = true))]
    pub punctuation: bool,
    /// Encoding of the audio. Optional — defaults to `linear16` (16-bit PCM) when omitted.
    #[serde(default = "default_stt_encoding")]
    #[cfg_attr(feature = "openapi", schema(example = "linear16"))]
    pub encoding: String,
    /// D8 uplink TRANSPORT codec the gateway decodes BEFORE handing PCM to the provider:
    /// `linear16` (default — raw PCM, no decode) | `opus` (one opus packet per WS binary frame,
    /// decoded to PCM16 @ `sample_rate`). Distinct from `encoding`, which some providers
    /// (Alibaba/Reverie) use to ACCEPT opus straight through — this one means "the GATEWAY decodes".
    /// Negotiated: an `opus` request on a gateway built without the `opus-codec` feature degrades to
    /// `linear16` (a `config_warning` is emitted) and the effective codec is echoed in `ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "opus"))]
    pub audio_in_codec: Option<String>,
    /// Model to use for transcription. Optional — defaults to empty, letting the provider pick its
    /// own default model (each provider maps an empty model to its recommended default).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(example = "nova-2"))]
    pub model: String,
    /// Optional API key for this provider (overrides server config)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Canonical, provider-agnostic advanced STT features (diarization, keyterms, redaction,
    /// vad_events, …). Defaults to all-unset so existing clients are unaffected; carried across the
    /// dispatch boundary via [`Self::to_standard_stt`] so providers with a `from_standard` mapping
    /// (Deepgram first) honor them END-TO-END (W1 keystone, closing BRUTAL_REVIEW.md S1).
    #[serde(default)]
    pub features: crate::core::stt::standard::SttFeatures,

    /// Open, typed passthrough for provider-specific parameters not modeled by `features`.
    #[serde(default)]
    pub extras: crate::core::stt::standard::ProviderExtras,

    /// P5 canonical translation block: `{ target_languages: [CanonicalLanguage], translate_to_english?,
    /// partials? }`. `None` = no translation. Threaded across the dispatch boundary via
    /// [`Self::to_standard_stt`] so translation-capable providers (Speechmatics/Gladia side-channel,
    /// OpenAI/Groq English fast path) emit `translations[]{lang,text}`; unsupported providers degrade
    /// with a `config_warning`, never a 400.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<crate::core::stt::standard::TranslationConfig>,

    /// ML turn detection for this session (P1.2): runs the audio-based
    /// smart-turn detector on the live frame path so end-of-turn is PREDICTED
    /// instead of waiting out the provider's silence endpointing. Standardized
    /// — provider-agnostic, applies to every STT provider. Requires a build
    /// with the `smart-turn`/`silero-vad` features; otherwise the session
    /// degrades LOUDLY (warn + waav_degraded_total) to timer fallback.
    #[serde(default)]
    pub turn_detection: Option<TurnDetectionWsConfig>,
}

/// Per-session ML turn-detection knobs (provider-agnostic).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TurnDetectionWsConfig {
    /// Master switch.
    pub enabled: bool,
    /// Decision threshold (model-calibrated default when omitted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
    /// Eager end-of-turn: on a turn-complete prediction, start the LLM
    /// speculatively (held + staged; see conversation_config.eager_eot which
    /// must also be enabled). Default false.
    #[serde(default)]
    pub eager: bool,
}

impl STTWebSocketConfig {
    /// Convert WebSocket STT config to full STT config with API key
    ///
    /// # Arguments
    /// * `api_key` - The API key to use for this provider
    ///
    /// # Returns
    /// * `STTConfig` - Full STT configuration
    pub fn to_stt_config(&self, api_key: String) -> STTConfig {
        STTConfig {
            provider: self.provider.clone(),
            api_key,
            // P2: resolve the canonical language token and map it to THIS provider's native
            // notation at the config→provider boundary, so no STT provider ever sees a raw client
            // string (e.g. the live-proven `us-en`). Identity for already-native values. Warnings
            // are surfaced separately as `config_warning` advisories (never a hard 400) — see
            // [`Self::language_mapping`] and `config_handler::emit_language_config_warnings`.
            language: self.mapped_stt_language(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            punctuation: self.punctuation,
            encoding: self.encoding.clone(),
            model: self.model.clone(),
        }
    }

    /// Map the client's raw `language` to this STT provider's native notation (P2). Returns the
    /// native string the provider should use; an `Auto`/omit result becomes the empty string so the
    /// provider falls back to its own default (the existing "empty language" convention every STT
    /// provider already handles). See [`crate::core::lang`].
    pub(crate) fn mapped_stt_language(&self) -> String {
        let mapped = self.language_mapping();
        if mapped.omit {
            String::new()
        } else {
            mapped.native
        }
    }

    /// The full language mapping result (native + warnings) for this STT config. Pure/cheap;
    /// `config_handler` re-runs it to emit `config_warning` advisories without changing the
    /// `to_stt_config` signature.
    pub(crate) fn language_mapping(&self) -> crate::core::lang::MappedLanguage {
        crate::core::lang::map_language(&self.language, &self.provider, &self.model)
    }

    /// Convert WebSocket STT config to the standardized [`StandardSTTConfig`] that crosses the
    /// dispatch/factory boundary — carrying the flat base **plus** advanced `features`/`extras`.
    ///
    /// This is the reachable W1 keystone path: the live handler routes through here so client
    /// features survive to `create_stt_standard` → `from_standard` instead of being dropped by the
    /// flat factory. Additive — [`Self::to_stt_config`] is unchanged for callers that don't need
    /// features.
    ///
    /// # Arguments
    /// * `api_key` - The resolved API key (client-provided or server config) for this provider.
    pub fn to_standard_stt(
        &self,
        api_key: String,
    ) -> crate::core::stt::standard::StandardSTTConfig {
        crate::core::stt::standard::StandardSTTConfig {
            base: self.to_stt_config(api_key),
            features: self.features.clone(),
            extras: self.extras.clone(),
            translation: self.translation.clone(),
        }
    }
}

/// LiveKit configuration for WebSocket messages
#[derive(Debug, Deserialize, Serialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LiveKitWebSocketConfig {
    /// Room name to join or create
    #[cfg_attr(feature = "openapi", schema(example = "conversation-room-123"))]
    pub room_name: String,
    /// Enable recording for this session
    #[serde(default)]
    pub enable_recording: bool,
    // recording_file_key removed; recording path now determined by stream_id + server prefix
    /// WaaV AI participant identity (defaults to "waav-ai")
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "waav-ai"))]
    pub waav_participant_identity: Option<String>,
    /// WaaV AI participant display name (defaults to "WaaV AI")
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "WaaV AI"))]
    pub waav_participant_name: Option<String>,
    /// List of participant identities to listen to for audio tracks and data messages. (All participants by default)
    ///
    /// **Behavior**:
    /// - If **empty** (default): Audio tracks and data messages from **all participants** will be processed
    /// - If **populated**: Only audio tracks and data messages from participants whose identities
    ///   are in this list will be processed; others will be ignored
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listen_participants: Vec<String>,
}

impl LiveKitWebSocketConfig {
    /// Convert WebSocket LiveKit config to full LiveKit config with audio parameters
    ///
    /// # Arguments
    /// * `token` - JWT token for LiveKit room access (generated by LiveKitRoomHandler)
    /// * `tts_config` - TTS configuration containing audio parameters
    /// * `livekit_url` - LiveKit server URL
    ///
    /// # Returns
    /// * `LiveKitConfig` - Full LiveKit configuration with audio parameters
    pub fn to_livekit_config(
        &self,
        token: String,
        tts_config: &TTSWebSocketConfig,
        ingress_sample_rate: u32,
        livekit_url: &str,
    ) -> LiveKitConfig {
        LiveKitConfig {
            url: livekit_url.to_string(),
            token,
            room_name: self.room_name.clone(),
            // EGRESS track rate must match the BYTES we feed it: the client
            // playback rate when egress resampling is on (C-G5), else the
            // provider rate, defaulting to 24000.
            sample_rate: tts_config
                .client_playback_rate
                .filter(|r| crate::handlers::ws::config_handler::valid_playback_rate(*r))
                .or(tts_config.sample_rate)
                .unwrap_or(24000),
            // INGRESS delivery rate = the STT pipeline's declared rate.
            ingress_sample_rate,
            // Assume mono audio for TTS (1 channel)
            channels: 1,
            // Enable noise filter by default when compiled with the optional feature
            // Can be disabled via config if lower latency is needed
            enable_noise_filter: cfg!(feature = "noise-filter"),
            // Pass through the participant filter list
            listen_participants: self.listen_participants.clone(),
        }
    }
}

/// TTS configuration for WebSocket messages (with optional API key)
#[derive(Debug, Deserialize, Serialize, Clone)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TTSWebSocketConfig {
    /// Provider name (e.g., "deepgram", "hume", "elevenlabs")
    #[cfg_attr(feature = "openapi", schema(example = "deepgram"))]
    pub provider: String,
    /// Voice ID or name to use for synthesis.
    ///
    /// ESCAPE HATCH: when set, this raw provider id is used VERBATIM and the
    /// [`Self::voice_descriptor`] resolution is skipped.
    #[cfg_attr(feature = "openapi", schema(example = "aura-asteria-en"))]
    pub voice_id: Option<String>,
    /// Canonical voice DESCRIPTOR (P4): `{gender, locale/accent, style, age,
    /// name_hint}` resolved SERVER-SIDE to a provider `voice_id` over the `/voices`
    /// catalog when no raw `voice_id` is given. No match → provider default +
    /// `config_warning` (never a 400). The resolved id is echoed back to the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_descriptor: Option<crate::core::voice::VoiceDescriptor>,
    /// Speaking rate (0.25 to 4.0, 1.0 is normal)
    #[cfg_attr(feature = "openapi", schema(example = 1.0))]
    pub speaking_rate: Option<f32>,
    /// Audio format preference
    #[cfg_attr(feature = "openapi", schema(example = "linear16"))]
    pub audio_format: Option<String>,
    /// Sample rate preference
    #[cfg_attr(feature = "openapi", schema(example = 24000))]
    pub sample_rate: Option<u32>,
    /// WS egress re-framing (E-G2): when set, PCM TTS audio is sliced into
    /// chunks of this many milliseconds before the WebSocket send, so a
    /// barge-in clear truncates within ONE chunk instead of letting a whole
    /// synthesis frame play out client-side. Omitted = verbatim frames
    /// (legacy). LiveKit egress already frames at 10ms internally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = 20))]
    pub audio_out_chunk_ms: Option<u32>,

    /// Client playback rate (C-G5): when set, the gateway resamples PCM TTS
    /// egress from the provider's rate to THIS rate before delivery (WS
    /// binary and LiveKit), so clients with a fixed-rate audio sink never
    /// resample provider-side or client-side. Identity (provider already at
    /// this rate) is zero-cost; non-PCM formats pass through unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = 48000))]
    pub client_playback_rate: Option<u32>,
    /// D8 downlink TRANSPORT codec for TTS egress: `linear16` (default — raw PCM16 frames) | `opus`
    /// (the gateway encodes each normalized PCM16 chunk to one opus packet per WS binary frame; the
    /// client decodes before playout). Encodes from the C-G5-normalized PCM16 stream and reuses
    /// `audio_out_chunk_ms` as the opus frame size (default 20ms) + `client_playback_rate` as the opus
    /// rate (constrained to {8,12,16,24,48}kHz; default 48000 when opus). A non-PCM provider stream
    /// (mp3/ogg) falls back to passthrough + warns. Negotiated: an `opus` request on a gateway without
    /// the `opus-codec` feature degrades to `linear16` (`config_warning`) and the effective codec is
    /// echoed in `ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "opus"))]
    pub audio_out_codec: Option<String>,
    /// Connection timeout in seconds
    #[cfg_attr(feature = "openapi", schema(example = 30))]
    pub connection_timeout: Option<u64>,
    /// Request timeout in seconds
    #[cfg_attr(feature = "openapi", schema(example = 60))]
    pub request_timeout: Option<u64>,
    /// Model to use for TTS
    #[cfg_attr(feature = "openapi", schema(example = "aura-asteria-en"))]
    pub model: String,
    /// Pronunciation replacements to apply before TTS
    #[serde(default)]
    pub pronunciations: Vec<Pronunciation>,
    /// Optional API key for this provider (overrides server config)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    // =========================================================================
    // Emotion Control (Unified Emotion System)
    // =========================================================================
    /// Emotion to express in speech synthesis.
    ///
    /// Supported by: Hume AI (all), ElevenLabs (core set), Azure (SSML styles).
    /// Providers without emotion support will synthesize speech normally with a warning.
    ///
    /// Examples: "happy", "sad", "angry", "excited", "calm", "sarcastic"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "happy"))]
    pub emotion: Option<Emotion>,

    /// Intensity of the emotion (0.0 to 1.0, or "low"/"medium"/"high").
    ///
    /// - 0.0-0.3: Subtle expression
    /// - 0.4-0.7: Moderate expression (default: 0.6)
    /// - 0.8-1.0: Strong expression
    ///
    /// Alternatively, use named levels: "low" (0.3), "medium" (0.6), "high" (1.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = 0.8))]
    pub emotion_intensity: Option<EmotionIntensity>,

    /// Delivery style modifier for speech.
    ///
    /// Affects pacing and prosody independently of emotion.
    /// Examples: "whispered", "shouted", "rushed", "measured", "expressive"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "expressive"))]
    pub delivery_style: Option<DeliveryStyle>,

    /// Free-form emotion description (for providers like Hume AI).
    ///
    /// When provided, this takes precedence over the `emotion` field for Hume AI.
    /// Maximum 100 characters.
    ///
    /// Examples: "warm, friendly, inviting", "whispered fearfully", "sarcastic, dry"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "warm, friendly, inviting"))]
    pub emotion_description: Option<String>,

    /// Canonical, provider-agnostic advanced TTS features (voice settings, instructions, SSML,
    /// streaming, …). Defaults to all-unset so existing clients are unaffected; carried across the
    /// dispatch boundary via [`Self::to_standard_tts`] so providers with a `from_standard` mapping
    /// (Deepgram first) honor them END-TO-END (W1 keystone, closing BRUTAL_REVIEW.md S1/S5).
    #[serde(default)]
    pub features: crate::core::tts::standard::TtsFeatures,

    /// Open, typed passthrough for provider-specific parameters not modeled by `features`.
    #[serde(default)]
    pub extras: crate::core::tts::standard::ProviderExtras,
}

impl TTSWebSocketConfig {
    /// Convert WebSocket TTS config to full TTS config with API key and proper defaults
    ///
    /// # Arguments
    /// * `api_key` - The API key to use for this provider
    ///
    /// # Returns
    /// * `TTSConfig` - Full TTS configuration with defaults applied
    pub fn to_tts_config(&self, api_key: String) -> TTSConfig {
        // Start with defaults
        let defaults = TTSConfig::default();

        // Extract emotion config from WebSocket config fields
        let emotion_config = self.to_emotion_config();

        TTSConfig {
            provider: self.provider.clone(),
            api_key,
            model: self.model.clone(),
            // Use provided values or fall back to defaults
            voice_id: self.voice_id.clone().or(defaults.voice_id),
            speaking_rate: self.speaking_rate.or(defaults.speaking_rate),
            audio_format: self.audio_format.clone().or(defaults.audio_format),
            sample_rate: self.sample_rate.or(defaults.sample_rate),
            connection_timeout: self.connection_timeout.or(defaults.connection_timeout),
            request_timeout: self.request_timeout.or(defaults.request_timeout),
            pronunciations: self.pronunciations.clone(),
            request_pool_size: defaults.request_pool_size,
            emotion_config,
        }
    }

    /// Convert WebSocket TTS config to the standardized [`StandardTTSConfig`] that crosses the
    /// dispatch/factory boundary — carrying the flat base (with emotion config applied) **plus**
    /// advanced `features`/`extras`.
    ///
    /// This is the reachable W1 keystone path: the live handler routes through here so client
    /// features survive to `create_tts_standard` → `from_standard` instead of being dropped by the
    /// flat factory. Additive — [`Self::to_tts_config`] is unchanged for callers that don't need
    /// features.
    ///
    /// # Arguments
    /// * `api_key` - The resolved API key (client-provided or server config) for this provider.
    pub fn to_standard_tts(
        &self,
        api_key: String,
    ) -> crate::core::tts::standard::StandardTTSConfig {
        let mut features = self.features.clone();
        // P2: map the client's TTS `features.language` to THIS provider's native notation at the
        // config→provider boundary (TTS language rides `TtsFeatures.language`, the typed slot every
        // TTS provider's `from_standard` reads). Identity for already-native values; unmapped/omit
        // clears the field so the provider keeps its default. Warnings surface as `config_warning`
        // (see `config_handler::emit_language_config_warnings`).
        if let Some(mapped) = self.mapped_tts_language() {
            features.language = if mapped.omit {
                None
            } else {
                Some(mapped.native)
            };
        }
        crate::core::tts::standard::StandardTTSConfig {
            base: self.to_tts_config(api_key),
            features,
            extras: self.extras.clone(),
        }
    }

    /// Map the client's `features.language` to this TTS provider's native notation (P2). Returns
    /// `None` when the client supplied no language (leave the provider default untouched), else the
    /// mapping result. See [`crate::core::lang`].
    pub(crate) fn mapped_tts_language(&self) -> Option<crate::core::lang::MappedLanguage> {
        self.tts_language_mapping()
    }

    /// The full TTS language mapping (native + warnings), or `None` if no `features.language` was
    /// supplied. The TTS-provider id is suffixed for the model-aware / composite TTS mappers
    /// (`google-tts`, `baidu-tts`, `reverie-tts`) so they diverge from their STT siblings.
    pub(crate) fn tts_language_mapping(&self) -> Option<crate::core::lang::MappedLanguage> {
        let raw = self.features.language.as_ref()?;
        if raw.trim().is_empty() {
            return None;
        }
        Some(crate::core::lang::map_language(
            raw,
            &tts_provider_alias(&self.provider),
            &self.model,
        ))
    }

    /// Extract emotion configuration from WebSocket config.
    ///
    /// Combines all emotion-related fields into a unified `EmotionConfig`.
    /// Returns `None` if no emotion settings are specified.
    ///
    /// # Returns
    ///
    /// An `EmotionConfig` if any emotion fields are set, `None` otherwise.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ws_config = TTSWebSocketConfig {
    ///     provider: "hume".to_string(),
    ///     emotion: Some(Emotion::Happy),
    ///     emotion_intensity: Some(EmotionIntensity::from_f32(0.8)),
    ///     ..Default::default()
    /// };
    ///
    /// if let Some(emotion_config) = ws_config.to_emotion_config() {
    ///     let mapper = get_mapper_for_provider(&ws_config.provider);
    ///     let mapped = mapper.map_emotion(&emotion_config);
    /// }
    /// ```
    pub fn to_emotion_config(&self) -> Option<EmotionConfig> {
        // DOUBLE-PATH UNIFICATION (P4). Two inputs can carry emotion:
        //   (a) the structured fields `emotion` / `emotion_intensity` / `delivery_style`
        //       / `emotion_description` on this config, and
        //   (b) the flat `features.emotion` String sugar (e.g. "happy", or a free-form
        //       "warm and reassuring").
        // Defined precedence: structured > string-as-emotion > string-as-description.
        // The structured path wins whenever ANY structured field is set; otherwise the
        // flat String is parsed via `emotion_config_from_string`.
        if self.emotion.is_some()
            || self.emotion_intensity.is_some()
            || self.delivery_style.is_some()
            || self.emotion_description.is_some()
        {
            return Some(EmotionConfig {
                emotion: self.emotion,
                intensity: self.emotion_intensity,
                style: self.delivery_style,
                description: self.emotion_description.clone(),
                context: None, // Context is not exposed in WebSocket config for simplicity
            });
        }

        // No structured fields → fold the flat `features.emotion` String sugar in.
        self.features
            .emotion
            .as_deref()
            .and_then(Self::emotion_config_from_string)
    }

    /// Parse the flat `features.emotion` String sugar into a structured
    /// [`EmotionConfig`] (P4 double-path unification).
    ///
    /// Precedence: a recognized canonical [`Emotion`] token wins; else a recognized
    /// [`DeliveryStyle`] token; else — if the value looks free-form (more than one
    /// word and not a known token) — it is treated as a Hume/OpenAI-style
    /// `description`. A single unrecognized word yields `None` (nothing to apply).
    pub(crate) fn emotion_config_from_string(raw: &str) -> Option<EmotionConfig> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(emotion) = Emotion::from_str(trimmed) {
            return Some(EmotionConfig {
                emotion: Some(emotion),
                ..Default::default()
            });
        }
        if let Some(style) = DeliveryStyle::from_str(trimmed) {
            return Some(EmotionConfig {
                style: Some(style),
                ..Default::default()
            });
        }
        // Free-form (multi-word) → description; single unknown word → ignore.
        if trimmed.split_whitespace().count() > 1 {
            return Some(EmotionConfig {
                description: Some(trimmed.to_string()),
                ..Default::default()
            });
        }
        None
    }

    /// Returns whether any emotion settings are configured (structured fields OR the
    /// flat `features.emotion` String sugar).
    #[inline]
    pub fn has_emotion_config(&self) -> bool {
        self.emotion.is_some()
            || self.emotion_intensity.is_some()
            || self.delivery_style.is_some()
            || self.emotion_description.is_some()
            || self
                .features
                .emotion
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
    }
}

/// Map a provider name to its TTS-side language-mapper key (P2). A handful of providers expose a
/// DIFFERENT native language notation for TTS than for STT on the SAME canonical — Google
/// (`cmn-CN` for TTS vs `cmn-Hans-CN` for STT), Baidu (`zh`/`ct` enum vs numeric dev_pid), Reverie
/// (`{lang}_{gender}` composite vs ISO short code). Those have a dedicated `*-tts` mapper in
/// [`crate::core::lang::mappers`]; everything else shares one mapper across STT/TTS, so the name
/// passes through unchanged.
fn tts_provider_alias(provider: &str) -> String {
    match provider.to_lowercase().as_str() {
        "google" | "google-tts" | "google_tts" => "google-tts".to_string(),
        "baidu" | "baidu-tts" | "baidu_tts" => "baidu-tts".to_string(),
        "reverie" | "reverie-ai" | "reverie_ai" | "reverie-tts" | "reverieinc" => {
            "reverie-tts".to_string()
        }
        "openai" => "openai-tts".to_string(),
        other => other.to_string(),
    }
}

/// Compute TTS configuration hash for caching.
///
/// Hashes the base config AND the standardized advanced `features` + provider-specific `extras`,
/// because those carry audio-changing parameters (voice settings, emotion, ssml, seed, latency, …)
/// that live outside `base`. Omitting them caused cache collisions for the TTS providers that have
/// no rich per-provider hash (Review Bug-class C).
pub fn compute_tts_config_hash(standard: &crate::core::tts::standard::StandardTTSConfig) -> String {
    let tts_config = &standard.base;
    let mut s = String::new();
    s.push_str(tts_config.provider.as_str());
    s.push('|');
    s.push_str(tts_config.voice_id.as_deref().unwrap_or(""));
    s.push('|');
    s.push_str(&tts_config.model);
    s.push('|');
    s.push_str(tts_config.audio_format.as_deref().unwrap_or(""));
    s.push('|');
    if let Some(sr) = tts_config.sample_rate {
        s.push_str(&sr.to_string());
    }
    s.push('|');
    if let Some(rate) = tts_config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }
    s.push('|');
    if let Ok(f) = serde_json::to_string(&standard.features) {
        s.push_str(&f);
    }
    s.push('|');
    if let Ok(e) = serde_json::to_string(&standard.extras) {
        s.push_str(&e);
    }
    format!("{:032x}", xxh3_128(s.as_bytes()))
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn conversation_config_threads_reasoning_effort() {
        use crate::core::llm::ReasoningEffort;
        // Present + typed (lowercase) → threads to the core config.
        let c: ConversationWebSocketConfig = serde_json::from_value(serde_json::json!({
            "base_url": "http://localhost:11434/v1",
            "model": "llama3.2:1b",
            "reasoning_effort": "minimal"
        }))
        .unwrap();
        assert_eq!(c.reasoning_effort, Some(ReasoningEffort::Minimal));
        assert_eq!(
            c.to_conversation_config().reasoning_effort,
            Some(ReasoningEffort::Minimal)
        );

        // Omitted → None (backward-compatible).
        let c2: ConversationWebSocketConfig = serde_json::from_value(serde_json::json!({
            "base_url": "http://localhost:11434/v1",
            "model": "llama3.2:1b"
        }))
        .unwrap();
        assert_eq!(c2.reasoning_effort, None);

        // A typo is rejected at deserialize (typed enum).
        assert!(
            serde_json::from_value::<ConversationWebSocketConfig>(serde_json::json!({
                "base_url": "x", "model": "y", "reasoning_effort": "minimial"
            }))
            .is_err()
        );
    }

    #[test]
    fn tts_cache_hash_includes_features_and_extras() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let base = StandardTTSConfig::from_base(crate::core::tts::TTSConfig {
            provider: "elevenlabs".to_string(),
            voice_id: Some("rachel".to_string()),
            model: "eleven_multilingual_v2".to_string(),
            ..Default::default()
        });
        let h0 = compute_tts_config_hash(&base);

        // An audio-changing FEATURE (voice stability) must change the cache key (Review Bug-class C).
        let mut with_feat = base.clone();
        with_feat.features = TtsFeatures {
            stability: Some(0.9),
            ..Default::default()
        };
        assert_ne!(
            h0,
            compute_tts_config_hash(&with_feat),
            "features must affect the cache key"
        );

        // A provider-specific EXTRA must change the cache key too.
        let mut with_extra = base.clone();
        with_extra
            .extras
            .0
            .insert("seed".to_string(), serde_json::json!(424242));
        assert_ne!(
            h0,
            compute_tts_config_hash(&with_extra),
            "extras must affect the cache key"
        );

        // Identical configs hash equal (stable).
        assert_eq!(h0, compute_tts_config_hash(&base.clone()));
    }

    #[test]
    fn stt_config_defaults_encoding_and_model_when_omitted() {
        // A client that omits `encoding`/`model` must no longer be hard-rejected with
        // "missing field …" — both now fall back to sensible defaults.
        let json = serde_json::json!({
            "provider": "deepgram",
            "language": "en-US",
            "sample_rate": 16000,
            "channels": 1,
            "punctuation": true
        });
        let cfg: STTWebSocketConfig = serde_json::from_value(json)
            .expect("stt_config should deserialize without encoding/model");
        assert_eq!(cfg.encoding, "linear16");
        assert_eq!(cfg.model, "");
        assert_eq!(cfg.provider, "deepgram");
    }

    #[test]
    fn stt_config_explicit_encoding_and_model_are_honored() {
        let json = serde_json::json!({
            "provider": "elevenlabs",
            "language": "en",
            "sample_rate": 16000,
            "channels": 1,
            "punctuation": true,
            "encoding": "mulaw",
            "model": "scribe_v2_realtime"
        });
        let cfg: STTWebSocketConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.encoding, "mulaw");
        assert_eq!(cfg.model, "scribe_v2_realtime");
    }

    #[test]
    fn p5_client_api_key_trims_and_treats_blank_as_absent() {
        assert_eq!(super::client_api_key(None), None);
        assert_eq!(super::client_api_key(Some("")), None);
        assert_eq!(super::client_api_key(Some("   \n\t")), None);
        assert_eq!(
            super::client_api_key(Some("  sk-live  ")),
            Some("sk-live".to_string())
        );
    }

    /// Helper: a minimal STT WS config for a provider + raw language.
    fn stt_ws(provider: &str, language: &str, model: &str) -> STTWebSocketConfig {
        STTWebSocketConfig {
            provider: provider.to_string(),
            language: language.to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: default_stt_encoding(),
            model: model.to_string(),
            api_key: None,
            features: Default::default(),
            extras: Default::default(),
            turn_detection: None,
            translation: None,
            audio_in_codec: None,
        }
    }

    #[test]
    fn p2_stt_language_is_mapped_to_provider_native_at_boundary() {
        // The live-proven `us-en` reversal bug: the provider must receive the NATIVE notation, not
        // the raw client string. Deepgram is identity-BCP47 → "en-US".
        let dg = stt_ws("deepgram", "us-en", "");
        assert_eq!(dg.to_stt_config("k".into()).language, "en-US");

        // ElevenLabs is ISO-639-1 downgrade → "en" (region dropped) for the SAME canonical input.
        let el = stt_ws("elevenlabs", "en-US", "scribe_v1");
        assert_eq!(el.to_stt_config("k".into()).language, "en");

        // Speechmatics ISO-639-3 for Chinese.
        let sm = stt_ws("speechmatics", "zh-CN", "");
        assert_eq!(sm.to_stt_config("k".into()).language, "cmn");

        // Already-native value resolves as identity (additive guarantee).
        let id = stt_ws("deepgram", "en-US", "");
        assert_eq!(id.to_stt_config("k".into()).language, "en-US");
    }

    #[test]
    fn p2_stt_auto_becomes_provider_default_or_native_token() {
        // Deepgram "auto" → the native "multi" code-switch token.
        let dg = stt_ws("deepgram", "auto", "");
        assert_eq!(dg.to_stt_config("k".into()).language, "multi");

        // A provider with no auto token + auto request → empty string (provider falls back to its
        // own default), and a warning is recorded in the mapping.
        let ct = stt_ws("cartesia", "auto", "");
        assert_eq!(ct.to_stt_config("k".into()).language, "en"); // cartesia Auto -> "en" + warn
        assert!(ct.language_mapping().has_warnings());
    }

    #[test]
    fn p2_stt_unknown_token_passes_through_with_warning() {
        // Graceful: an unrecognized token is forwarded VERBATIM (never a hard 400) + warns.
        let cfg = stt_ws("deepgram", "klingon", "");
        assert_eq!(cfg.to_stt_config("k".into()).language, "klingon");
        assert!(cfg.language_mapping().has_warnings());
    }

    /// Helper: a TTS WS config from provider/model/language (via serde, so it stays robust to the
    /// full TTSWebSocketConfig field set). A `None` language omits `features.language`.
    fn tts_ws(provider: &str, model: &str, language: Option<&str>) -> TTSWebSocketConfig {
        let mut v = serde_json::json!({
            "provider": provider,
            "voice_id": "rachel",
            "model": model,
        });
        if let Some(l) = language {
            v["features"] = serde_json::json!({ "language": l });
        }
        serde_json::from_value(v).expect("tts ws config deserializes")
    }

    #[test]
    fn p2_tts_features_language_is_mapped() {
        // TTS language rides features.language. ElevenLabs downgrade: "es-ES" -> "es".
        let el = tts_ws("elevenlabs", "eleven_turbo_v2_5", Some("es-ES"));
        assert_eq!(
            el.to_standard_tts("k".into()).features.language.as_deref(),
            Some("es")
        );

        // Reverie TTS composite: the {lang} half is canonical.iso639_1() — "hi-IN" -> "hi".
        let rv = tts_ws("reverie", "indian", Some("hi-IN"));
        assert_eq!(
            rv.to_standard_tts("k".into()).features.language.as_deref(),
            Some("hi")
        );

        // Google TTS keeps cmn-CN (no script subtag), diverging from Google STT's cmn-Hans-CN.
        let gt = tts_ws("google", "", Some("cmn-CN"));
        assert_eq!(
            gt.to_standard_tts("k".into()).features.language.as_deref(),
            Some("cmn-CN")
        );

        // A name spelling resolves too: "spanish" -> es-ES -> elevenlabs "es".
        let name = tts_ws("elevenlabs", "eleven_turbo_v2_5", Some("spanish"));
        assert_eq!(
            name.to_standard_tts("k".into())
                .features
                .language
                .as_deref(),
            Some("es")
        );
    }

    #[test]
    fn p2_tts_no_language_leaves_features_untouched() {
        // A TTS config without features.language must not invent one (backward-compat).
        let tts = tts_ws("elevenlabs", "eleven_turbo_v2_5", None);
        assert_eq!(tts.to_standard_tts("k".into()).features.language, None);
    }

    // ---- P4 double-path emotion unification --------------------------------

    #[test]
    fn p4_flat_emotion_string_folds_to_emotion() {
        // features.emotion: "happy" (no structured fields) → EmotionConfig{emotion: Happy}.
        let cfg: TTSWebSocketConfig = serde_json::from_value(serde_json::json!({
            "provider": "cartesia",
            "model": "sonic-3",
            "features": { "emotion": "happy" }
        }))
        .unwrap();
        let ec = cfg.to_emotion_config().expect("string sugar folds in");
        assert_eq!(ec.emotion, Some(Emotion::Happy));
        assert!(ec.description.is_none());
    }

    #[test]
    fn p4_flat_emotion_string_delivery_style() {
        // A delivery-style word lands on the style slot, not emotion.
        let cfg: TTSWebSocketConfig = serde_json::from_value(serde_json::json!({
            "provider": "azure",
            "model": "neural",
            "features": { "emotion": "whispering" }
        }))
        .unwrap();
        let ec = cfg.to_emotion_config().unwrap();
        assert_eq!(ec.style, Some(DeliveryStyle::Whispered));
        assert!(ec.emotion.is_none());
    }

    #[test]
    fn p4_flat_emotion_string_freeform_becomes_description() {
        // Multi-word free-form → description (Hume/OpenAI path).
        let cfg: TTSWebSocketConfig = serde_json::from_value(serde_json::json!({
            "provider": "hume",
            "model": "octave",
            "features": { "emotion": "warm and reassuring, like a friend" }
        }))
        .unwrap();
        let ec = cfg.to_emotion_config().unwrap();
        assert_eq!(
            ec.description.as_deref(),
            Some("warm and reassuring, like a friend")
        );
        assert!(ec.emotion.is_none());
    }

    #[test]
    fn p4_structured_wins_over_flat_string() {
        // Structured `emotion` present → flat string is ignored (defined precedence).
        let cfg: TTSWebSocketConfig = serde_json::from_value(serde_json::json!({
            "provider": "cartesia",
            "model": "sonic-3",
            "emotion": "sad",
            "features": { "emotion": "happy" }
        }))
        .unwrap();
        let ec = cfg.to_emotion_config().unwrap();
        assert_eq!(ec.emotion, Some(Emotion::Sad));
    }

    #[test]
    fn p4_flat_string_reaches_cartesia_native_token() {
        // End-to-end through the unifier: features.emotion "ecstatic" → base.emotion_config
        // → Cartesia native token "euphoric".
        let cfg: TTSWebSocketConfig = serde_json::from_value(serde_json::json!({
            "provider": "cartesia",
            "model": "sonic-3",
            "features": { "emotion": "ecstatic" }
        }))
        .unwrap();
        let base = cfg.to_tts_config("k".into());
        let ec = base.emotion_config.expect("unified emotion config present");
        assert_eq!(ec.emotion, Some(Emotion::Ecstatic));
    }

    #[test]
    fn p4_single_unknown_word_is_ignored() {
        // A single unrecognized token yields no emotion config (nothing to apply).
        assert!(TTSWebSocketConfig::emotion_config_from_string("zzz").is_none());
        assert!(TTSWebSocketConfig::emotion_config_from_string("   ").is_none());
    }
}
