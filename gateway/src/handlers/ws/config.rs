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
            language: self.language.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            punctuation: self.punctuation,
            encoding: self.encoding.clone(),
            model: self.model.clone(),
        }
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
    pub fn to_standard_stt(&self, api_key: String) -> crate::core::stt::standard::StandardSTTConfig {
        crate::core::stt::standard::StandardSTTConfig {
            base: self.to_stt_config(api_key),
            features: self.features.clone(),
            extras: self.extras.clone(),
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
    /// Voice ID or name to use for synthesis
    #[cfg_attr(feature = "openapi", schema(example = "aura-asteria-en"))]
    pub voice_id: Option<String>,
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
    pub fn to_standard_tts(&self, api_key: String) -> crate::core::tts::standard::StandardTTSConfig {
        crate::core::tts::standard::StandardTTSConfig {
            base: self.to_tts_config(api_key),
            features: self.features.clone(),
            extras: self.extras.clone(),
        }
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
        // Only return config if at least one emotion field is set
        if self.emotion.is_none()
            && self.emotion_intensity.is_none()
            && self.delivery_style.is_none()
            && self.emotion_description.is_none()
        {
            return None;
        }

        Some(EmotionConfig {
            emotion: self.emotion,
            intensity: self.emotion_intensity,
            style: self.delivery_style,
            description: self.emotion_description.clone(),
            context: None, // Context is not exposed in WebSocket config for simplicity
        })
    }

    /// Returns whether any emotion settings are configured.
    #[inline]
    pub fn has_emotion_config(&self) -> bool {
        self.emotion.is_some()
            || self.emotion_intensity.is_some()
            || self.delivery_style.is_some()
            || self.emotion_description.is_some()
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
        assert_eq!(c.to_conversation_config().reasoning_effort, Some(ReasoningEffort::Minimal));

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
        assert_ne!(h0, compute_tts_config_hash(&with_feat), "features must affect the cache key");

        // A provider-specific EXTRA must change the cache key too.
        let mut with_extra = base.clone();
        with_extra
            .extras
            .0
            .insert("seed".to_string(), serde_json::json!(424242));
        assert_ne!(h0, compute_tts_config_hash(&with_extra), "extras must affect the cache key");

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
        let cfg: STTWebSocketConfig =
            serde_json::from_value(json).expect("stt_config should deserialize without encoding/model");
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
}
