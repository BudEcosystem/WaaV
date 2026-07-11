//! AmiVoice STT configuration module.
//!
//! This module provides configuration types for the AmiVoice Cloud Platform
//! Speech-to-Text API from Advanced Media Inc. (Japan).
//!
//! # Authentication
//!
//! AmiVoice uses a simple API key (APPKEY) for authentication.
//! Obtain your APPKEY from: https://acp.amivoice.com/
//!
//! # Engines
//!
//! AmiVoice provides two types of speech recognition engines:
//!
//! - **End-to-End (E2E)**: Next-generation neural models with higher accuracy
//! - **Hybrid**: Domain-optimized models with word registration support
//!
//! # Audio Format
//!
//! All engines support 8kHz (telephony) and 16kHz (multimedia) audio.
//! PCM 16-bit little-endian mono is the standard format.

use crate::core::stt::base::STTConfig;
use serde::{Deserialize, Serialize};

fn validate_amivoice_stt_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    crate::core::net::validate_url_for_ssrf(endpoint, &["ws", "wss"])
        .map_err(|e| format!("{source} rejected (SSRF protection): {e}"))
}

// =============================================================================
// Constants
// =============================================================================

/// AmiVoice WebSocket endpoint with logging
pub const AMIVOICE_WS_URL: &str = "wss://acp-api.amivoice.com/v1/";

/// AmiVoice WebSocket endpoint without logging
pub const AMIVOICE_WS_NOLOG_URL: &str = "wss://acp-api.amivoice.com/v1/nolog/";

/// AmiVoice Synchronous HTTP endpoint with logging
pub const AMIVOICE_HTTP_URL: &str = "https://acp-api.amivoice.com/v1/recognize";

/// AmiVoice Synchronous HTTP endpoint without logging
pub const AMIVOICE_HTTP_NOLOG_URL: &str = "https://acp-api.amivoice.com/v1/nolog/recognize";

/// Default speech recognition engine (Japanese general purpose)
pub const DEFAULT_ENGINE: &str = "-a-general";

/// Default result update interval in milliseconds
pub const DEFAULT_RESULT_UPDATED_INTERVAL: u32 = 1000;

/// Default inactivity timeout in seconds
pub const DEFAULT_INACTIVITY_TIMEOUT: u32 = 30;

// =============================================================================
// Speech Recognition Engines
// =============================================================================

/// AmiVoice speech recognition engine types.
///
/// # Engine Categories
///
/// ## End-to-End (E2E) Engines
/// Next-generation neural models with generally higher accuracy.
/// Do NOT support word registration.
///
/// ## Hybrid Engines
/// Domain-optimized models combining acoustic and language models.
/// Support word registration for custom vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AmiVoiceEngine {
    // =========================================================================
    // End-to-End (E2E) Engines - Real-time
    // =========================================================================
    /// Japanese general purpose E2E (real-time)
    #[serde(rename = "-a2-ja-general")]
    E2EJapaneseGeneral,

    /// Chinese general purpose E2E (real-time)
    #[serde(rename = "-a2-zh-general")]
    E2EChineseGeneral,

    /// Multilingual E2E (Japanese, English, Chinese - real-time)
    #[serde(rename = "-a2-multi-general")]
    E2EMultilingual,

    // =========================================================================
    // End-to-End (E2E) Engines - Batch (Higher Accuracy)
    // =========================================================================
    /// Japanese general purpose E2E (batch)
    #[serde(rename = "-a2b-ja-general")]
    E2EBatchJapaneseGeneral,

    /// Chinese general purpose E2E (batch)
    #[serde(rename = "-a2b-zh-general")]
    E2EBatchChineseGeneral,

    /// Multilingual E2E (batch)
    #[serde(rename = "-a2b-multi-general")]
    E2EBatchMultilingual,

    // =========================================================================
    // Hybrid Engines - Conversation (Telephone/Meeting)
    // =========================================================================
    /// Japanese general conversation (default)
    #[default]
    #[serde(rename = "-a-general")]
    HybridJapaneseGeneral,

    /// Japanese medical conversation
    #[serde(rename = "-a-medical")]
    HybridJapaneseMedical,

    /// Japanese finance conversation
    #[serde(rename = "-a-finance")]
    HybridJapaneseFinance,

    /// Japanese insurance conversation
    #[serde(rename = "-a-insurance")]
    HybridJapaneseInsurance,

    /// Chinese general conversation
    #[serde(rename = "-a-general-zh")]
    HybridChineseGeneral,

    /// English general conversation
    #[serde(rename = "-a-general-en")]
    HybridEnglishGeneral,

    /// Korean general conversation
    #[serde(rename = "-a-general-ko")]
    HybridKoreanGeneral,

    // =========================================================================
    // Hybrid Engines - Voice Input (Dictation)
    // =========================================================================
    /// Japanese general voice input
    #[serde(rename = "-a-general-input")]
    HybridJapaneseGeneralInput,

    /// Japanese medical voice input
    #[serde(rename = "-a-medical-input")]
    HybridJapaneseMedicalInput,

    /// Japanese finance voice input
    #[serde(rename = "-a-finance-input")]
    HybridJapaneseFinanceInput,

    /// Japanese insurance voice input
    #[serde(rename = "-a-insurance-input")]
    HybridJapaneseInsuranceInput,

    /// Japanese name recognition
    #[serde(rename = "-a-name-input")]
    HybridJapaneseNameInput,

    /// Japanese address recognition
    #[serde(rename = "-a-address-input")]
    HybridJapaneseAddressInput,
}

impl AmiVoiceEngine {
    /// Get the engine ID string for API requests.
    pub fn engine_id(&self) -> &'static str {
        match self {
            // E2E Real-time
            Self::E2EJapaneseGeneral => "-a2-ja-general",
            Self::E2EChineseGeneral => "-a2-zh-general",
            Self::E2EMultilingual => "-a2-multi-general",
            // E2E Batch
            Self::E2EBatchJapaneseGeneral => "-a2b-ja-general",
            Self::E2EBatchChineseGeneral => "-a2b-zh-general",
            Self::E2EBatchMultilingual => "-a2b-multi-general",
            // Hybrid Conversation
            Self::HybridJapaneseGeneral => "-a-general",
            Self::HybridJapaneseMedical => "-a-medical",
            Self::HybridJapaneseFinance => "-a-finance",
            Self::HybridJapaneseInsurance => "-a-insurance",
            Self::HybridChineseGeneral => "-a-general-zh",
            Self::HybridEnglishGeneral => "-a-general-en",
            Self::HybridKoreanGeneral => "-a-general-ko",
            // Hybrid Voice Input
            Self::HybridJapaneseGeneralInput => "-a-general-input",
            Self::HybridJapaneseMedicalInput => "-a-medical-input",
            Self::HybridJapaneseFinanceInput => "-a-finance-input",
            Self::HybridJapaneseInsuranceInput => "-a-insurance-input",
            Self::HybridJapaneseNameInput => "-a-name-input",
            Self::HybridJapaneseAddressInput => "-a-address-input",
        }
    }

    /// Get the display name for the engine.
    pub fn display_name(&self) -> &'static str {
        match self {
            // E2E Real-time
            Self::E2EJapaneseGeneral => "E2E Japanese General (Real-time)",
            Self::E2EChineseGeneral => "E2E Chinese General (Real-time)",
            Self::E2EMultilingual => "E2E Multilingual (Real-time)",
            // E2E Batch
            Self::E2EBatchJapaneseGeneral => "E2E Japanese General (Batch)",
            Self::E2EBatchChineseGeneral => "E2E Chinese General (Batch)",
            Self::E2EBatchMultilingual => "E2E Multilingual (Batch)",
            // Hybrid Conversation
            Self::HybridJapaneseGeneral => "Hybrid Japanese General",
            Self::HybridJapaneseMedical => "Hybrid Japanese Medical",
            Self::HybridJapaneseFinance => "Hybrid Japanese Finance",
            Self::HybridJapaneseInsurance => "Hybrid Japanese Insurance",
            Self::HybridChineseGeneral => "Hybrid Chinese General",
            Self::HybridEnglishGeneral => "Hybrid English General",
            Self::HybridKoreanGeneral => "Hybrid Korean General",
            // Hybrid Voice Input
            Self::HybridJapaneseGeneralInput => "Hybrid Japanese General Input",
            Self::HybridJapaneseMedicalInput => "Hybrid Japanese Medical Input",
            Self::HybridJapaneseFinanceInput => "Hybrid Japanese Finance Input",
            Self::HybridJapaneseInsuranceInput => "Hybrid Japanese Insurance Input",
            Self::HybridJapaneseNameInput => "Hybrid Japanese Name Recognition",
            Self::HybridJapaneseAddressInput => "Hybrid Japanese Address Recognition",
        }
    }

    /// Check if this engine supports word registration.
    /// E2E engines do NOT support word registration.
    pub fn supports_word_registration(&self) -> bool {
        !matches!(
            self,
            Self::E2EJapaneseGeneral
                | Self::E2EChineseGeneral
                | Self::E2EMultilingual
                | Self::E2EBatchJapaneseGeneral
                | Self::E2EBatchChineseGeneral
                | Self::E2EBatchMultilingual
        )
    }

    /// Check if this is a batch-optimized engine.
    pub fn is_batch_engine(&self) -> bool {
        matches!(
            self,
            Self::E2EBatchJapaneseGeneral
                | Self::E2EBatchChineseGeneral
                | Self::E2EBatchMultilingual
        )
    }

    /// Get the primary language for this engine.
    pub fn primary_language(&self) -> &'static str {
        match self {
            Self::E2EJapaneseGeneral
            | Self::E2EBatchJapaneseGeneral
            | Self::HybridJapaneseGeneral
            | Self::HybridJapaneseMedical
            | Self::HybridJapaneseFinance
            | Self::HybridJapaneseInsurance
            | Self::HybridJapaneseGeneralInput
            | Self::HybridJapaneseMedicalInput
            | Self::HybridJapaneseFinanceInput
            | Self::HybridJapaneseInsuranceInput
            | Self::HybridJapaneseNameInput
            | Self::HybridJapaneseAddressInput => "ja",

            Self::E2EChineseGeneral | Self::E2EBatchChineseGeneral | Self::HybridChineseGeneral => {
                "zh"
            }

            Self::HybridEnglishGeneral => "en",
            Self::HybridKoreanGeneral => "ko",

            Self::E2EMultilingual | Self::E2EBatchMultilingual => "multi",
        }
    }

    /// Parse engine from string (engine ID or name).
    pub fn from_str_relaxed(s: &str) -> Option<Self> {
        let s_lower = s.to_lowercase().replace('_', "-");
        match s_lower.as_str() {
            // E2E Real-time
            "-a2-ja-general" | "e2e-japanese-general" | "e2e-ja" | "a2-ja" => {
                Some(Self::E2EJapaneseGeneral)
            }
            "-a2-zh-general" | "e2e-chinese-general" | "e2e-zh" | "a2-zh" => {
                Some(Self::E2EChineseGeneral)
            }
            "-a2-multi-general" | "e2e-multilingual" | "e2e-multi" | "a2-multi" => {
                Some(Self::E2EMultilingual)
            }
            // E2E Batch
            "-a2b-ja-general" | "e2e-batch-japanese" | "a2b-ja" => {
                Some(Self::E2EBatchJapaneseGeneral)
            }
            "-a2b-zh-general" | "e2e-batch-chinese" | "a2b-zh" => {
                Some(Self::E2EBatchChineseGeneral)
            }
            "-a2b-multi-general" | "e2e-batch-multilingual" | "a2b-multi" => {
                Some(Self::E2EBatchMultilingual)
            }
            // Hybrid Conversation
            "-a-general" | "hybrid-japanese-general" | "ja-general" | "general" => {
                Some(Self::HybridJapaneseGeneral)
            }
            "-a-medical" | "hybrid-japanese-medical" | "ja-medical" | "medical" => {
                Some(Self::HybridJapaneseMedical)
            }
            "-a-finance" | "hybrid-japanese-finance" | "ja-finance" | "finance" => {
                Some(Self::HybridJapaneseFinance)
            }
            "-a-insurance" | "hybrid-japanese-insurance" | "ja-insurance" | "insurance" => {
                Some(Self::HybridJapaneseInsurance)
            }
            "-a-general-zh" | "hybrid-chinese-general" | "zh-general" => {
                Some(Self::HybridChineseGeneral)
            }
            "-a-general-en" | "hybrid-english-general" | "en-general" => {
                Some(Self::HybridEnglishGeneral)
            }
            "-a-general-ko" | "hybrid-korean-general" | "ko-general" => {
                Some(Self::HybridKoreanGeneral)
            }
            // Hybrid Voice Input
            "-a-general-input" | "hybrid-japanese-general-input" | "ja-input" => {
                Some(Self::HybridJapaneseGeneralInput)
            }
            "-a-medical-input" | "hybrid-japanese-medical-input" | "medical-input" => {
                Some(Self::HybridJapaneseMedicalInput)
            }
            "-a-finance-input" | "hybrid-japanese-finance-input" | "finance-input" => {
                Some(Self::HybridJapaneseFinanceInput)
            }
            "-a-insurance-input" | "hybrid-japanese-insurance-input" | "insurance-input" => {
                Some(Self::HybridJapaneseInsuranceInput)
            }
            "-a-name-input" | "hybrid-japanese-name-input" | "name-input" | "name" => {
                Some(Self::HybridJapaneseNameInput)
            }
            "-a-address-input" | "hybrid-japanese-address-input" | "address-input" | "address" => {
                Some(Self::HybridJapaneseAddressInput)
            }
            _ => None,
        }
    }

    /// Get all available engines.
    pub fn all() -> Vec<Self> {
        vec![
            // E2E Real-time
            Self::E2EJapaneseGeneral,
            Self::E2EChineseGeneral,
            Self::E2EMultilingual,
            // E2E Batch
            Self::E2EBatchJapaneseGeneral,
            Self::E2EBatchChineseGeneral,
            Self::E2EBatchMultilingual,
            // Hybrid Conversation
            Self::HybridJapaneseGeneral,
            Self::HybridJapaneseMedical,
            Self::HybridJapaneseFinance,
            Self::HybridJapaneseInsurance,
            Self::HybridChineseGeneral,
            Self::HybridEnglishGeneral,
            Self::HybridKoreanGeneral,
            // Hybrid Voice Input
            Self::HybridJapaneseGeneralInput,
            Self::HybridJapaneseMedicalInput,
            Self::HybridJapaneseFinanceInput,
            Self::HybridJapaneseInsuranceInput,
            Self::HybridJapaneseNameInput,
            Self::HybridJapaneseAddressInput,
        ]
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// Audio format specification for AmiVoice API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum AmiVoiceAudioFormat {
    /// 16kHz 16-bit PCM (default for multimedia)
    #[default]
    #[serde(rename = "16K")]
    Pcm16kHz,

    /// 8kHz 16-bit PCM (telephony)
    #[serde(rename = "8K")]
    Pcm8kHz,

    /// 16kHz 16-bit PCM Little Endian
    #[serde(rename = "LSB16K")]
    Lsb16kHz,

    /// 8kHz 16-bit PCM Little Endian
    #[serde(rename = "LSB8K")]
    Lsb8kHz,
}

impl AmiVoiceAudioFormat {
    /// Get the format string for API requests.
    pub fn format_code(&self) -> &'static str {
        match self {
            Self::Pcm16kHz => "16K",
            Self::Pcm8kHz => "8K",
            Self::Lsb16kHz => "LSB16K",
            Self::Lsb8kHz => "LSB8K",
        }
    }

    /// Get the sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Pcm16kHz | Self::Lsb16kHz => 16000,
            Self::Pcm8kHz | Self::Lsb8kHz => 8000,
        }
    }

    /// Create format from sample rate.
    pub fn from_sample_rate(sample_rate: u32) -> Self {
        if sample_rate <= 8000 {
            Self::Pcm8kHz
        } else {
            Self::Pcm16kHz
        }
    }
}

// =============================================================================
// AmiVoice STT Configuration
// =============================================================================

/// AmiVoice-specific STT configuration.
///
/// This struct contains all parameters for configuring the AmiVoice
/// Cloud Platform Speech-to-Text API.
#[derive(Debug, Clone)]
pub struct AmiVoiceSTTConfig {
    /// Base STT configuration.
    pub base: STTConfig,

    /// APPKEY for authentication.
    pub app_key: String,

    /// Speech recognition engine to use.
    pub engine: AmiVoiceEngine,

    /// Audio format specification.
    pub audio_format: AmiVoiceAudioFormat,

    /// Whether to use the no-logging endpoint.
    pub no_logging: bool,

    /// Enable interim (partial) results.
    pub interim_results: bool,

    /// Result update interval in milliseconds.
    /// Only applicable when interim_results is true.
    pub result_updated_interval: u32,

    /// Custom word definitions for hybrid engines.
    /// Format: JSON array of {written, spoken, class}
    pub profile_words: Option<String>,

    /// Profile ID for user-specific settings.
    pub profile_id: Option<String>,

    /// Enable sentiment/emotion analysis.
    pub enable_sentiment: bool,

    /// Enable speaker diarization.
    pub enable_diarization: bool,

    /// Segmenter properties (e.g., "useDiarizer=1").
    pub segmenter_properties: Option<String>,

    /// Connection timeout in seconds.
    pub connection_timeout_secs: u64,

    /// Inactivity timeout in seconds.
    pub inactivity_timeout_secs: u32,

    // -------------------------------------------------------------------------
    // Advanced AmiVoice request parameters (emitted on the `s` start command).
    // All `Option`, so `None` omits the param and keeps the AmiVoice default.
    // -------------------------------------------------------------------------
    /// Keep filler tokens ("えーと", "あのー") in the result instead of dropping them
    /// (`keepFillerToken=1`). Mapped from the typed `SttFeatures::filler_words`.
    pub keep_filler_words: Option<bool>,

    /// No-input timeout in milliseconds (`noInputTimeout`): how long to wait for the first
    /// utterance before ending the session.
    pub no_input_timeout: Option<u32>,

    /// Usage-aggregation tag (`extension`): a free-form string AmiVoice echoes back for
    /// per-tag usage accounting.
    pub usage_aggregation_tag: Option<String>,

    /// Max decoding time in milliseconds (`maxDecodingTime`).
    pub max_decoding_time: Option<u32>,

    /// Max response time in milliseconds (`maxResponseTime`).
    pub max_response_time: Option<u32>,

    /// Max decoding rate, a real-time-factor multiplier (`maxDecodingRate`).
    pub max_decoding_rate: Option<f32>,

    /// Target response time in milliseconds (`targetResponseTime`).
    pub target_response_time: Option<u32>,

    /// Target decoding rate, a real-time-factor multiplier (`targetDecodingRate`).
    pub target_decoding_rate: Option<f32>,

    /// Recognition timeout in milliseconds (`recognitionTimeout`): hard cap on recognition time.
    pub recognition_timeout: Option<u32>,

    /// Carried from the standardized `endpoint_override` — points the dial at the in-repo mock/proxy
    /// (a local `ws://` server) for credential-free end-to-end integration tests; `None` uses the
    /// production AmiVoice endpoint. The featured `s` start command is unchanged (auth is in-band);
    /// only the dialed scheme://host is swapped, with the AmiVoice `/v1/` path re-appended.
    pub endpoint_override: Option<String>,
}

impl Default for AmiVoiceSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            app_key: String::new(),
            engine: AmiVoiceEngine::default(),
            audio_format: AmiVoiceAudioFormat::default(),
            no_logging: false,
            interim_results: true,
            result_updated_interval: DEFAULT_RESULT_UPDATED_INTERVAL,
            profile_words: None,
            profile_id: None,
            enable_sentiment: false,
            enable_diarization: false,
            segmenter_properties: None,
            connection_timeout_secs: 30,
            inactivity_timeout_secs: DEFAULT_INACTIVITY_TIMEOUT,
            keep_filler_words: None,
            no_input_timeout: None,
            usage_aggregation_tag: None,
            max_decoding_time: None,
            max_response_time: None,
            max_decoding_rate: None,
            target_response_time: None,
            target_decoding_rate: None,
            recognition_timeout: None,
            endpoint_override: None,
        }
    }
}

impl AmiVoiceSTTConfig {
    /// Create configuration from base STTConfig.
    pub fn from_base(config: STTConfig) -> Self {
        let app_key = config.api_key.clone();

        // Parse engine from model field if provided
        let engine = if config.model.is_empty() {
            AmiVoiceEngine::default()
        } else {
            AmiVoiceEngine::from_str_relaxed(&config.model).unwrap_or_default()
        };

        // Determine audio format from sample rate
        let audio_format = AmiVoiceAudioFormat::from_sample_rate(config.sample_rate);

        Self {
            base: config,
            app_key,
            engine,
            audio_format,
            no_logging: false,
            interim_results: true,
            result_updated_interval: DEFAULT_RESULT_UPDATED_INTERVAL,
            profile_words: None,
            profile_id: None,
            enable_sentiment: false,
            enable_diarization: false,
            segmenter_properties: None,
            connection_timeout_secs: 30,
            inactivity_timeout_secs: DEFAULT_INACTIVITY_TIMEOUT,
            keep_filler_words: None,
            no_input_timeout: None,
            usage_aggregation_tag: None,
            max_decoding_time: None,
            max_response_time: None,
            max_decoding_rate: None,
            target_response_time: None,
            target_decoding_rate: None,
            recognition_timeout: None,
            endpoint_override: None,
        }
    }

    /// Build from the standardized config (W1 keystone). AmiVoice exposes a narrow boolean
    /// surface, so this maps the standardized features whose meaning matches an existing AmiVoice
    /// field: speaker diarization (`enable_diarization`), interim/partial results
    /// (`interim_results`) and filler-word retention (typed `filler_words` → `keepFillerToken`).
    /// AmiVoice also has a set of provider-specific request knobs with no canonical SttFeatures
    /// field (no-input timeout, usage-aggregation tag, decoding/response time + rate caps,
    /// recognition timeout); those are forwarded through the open `ProviderExtras` passthrough.
    /// Features AmiVoice cannot express (word_timestamps, smart_format, profanity_filter,
    /// redaction, keyterms, vad_events, language_detection, entity_detection) are capability gaps
    /// and stay at default.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone());
        if let Some(d) = f.diarization {
            cfg.enable_diarization = d;
        }
        if let Some(i) = f.interim_results {
            cfg.interim_results = i;
        }
        // keep_filler_words is a shared semantic (typed): filler_words=true ⇒ keepFillerToken=1.
        if let Some(keep) = f.filler_words {
            cfg.keep_filler_words = Some(keep);
        }
        // Provider-specific tuning knobs via the open passthrough.
        let ex = &std.extras.0;
        cfg.no_input_timeout = ex
            .get("no_input_timeout")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        cfg.usage_aggregation_tag = ex
            .get("usage_aggregation_tag")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        cfg.max_decoding_time = ex
            .get("max_decoding_time")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        cfg.max_response_time = ex
            .get("max_response_time")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        cfg.max_decoding_rate = ex
            .get("max_decoding_rate")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32);
        cfg.target_response_time = ex
            .get("target_response_time")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        cfg.target_decoding_rate = ex
            .get("target_decoding_rate")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32);
        cfg.recognition_timeout = ex
            .get("recognition_timeout")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        // Standardized endpoint override (mock/proxy host) for credential-free integration tests.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        cfg
    }

    /// Get the WebSocket URL. Honors an `endpoint_override` (the in-repo mock/proxy points this at a
    /// local ws:// server) for credential-free integration; otherwise the production AmiVoice
    /// endpoint. The override carries only `scheme://host[:port]`, so the AmiVoice `/v1/` (or
    /// `/v1/nolog/`) path is re-appended — a path-less URL would fail the WS handshake.
    pub fn get_websocket_url(&self) -> String {
        let path = if self.no_logging {
            "/v1/nolog/"
        } else {
            "/v1/"
        };
        match self
            .endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
        {
            Some(o) => format!("{}{}", o.trim_end_matches('/'), path),
            None if self.no_logging => AMIVOICE_WS_NOLOG_URL.to_string(),
            None => AMIVOICE_WS_URL.to_string(),
        }
    }

    /// Get the HTTP URL.
    pub fn get_http_url(&self) -> &'static str {
        if self.no_logging {
            AMIVOICE_HTTP_NOLOG_URL
        } else {
            AMIVOICE_HTTP_URL
        }
    }

    /// Build the 's' command for WebSocket session start.
    ///
    /// Format: `s <sample_rate> <engine_id> [key=value ...]`
    pub fn build_start_command(&self) -> String {
        let mut parts = Vec::new();

        // Start with 's' command
        parts.push("s".to_string());

        // Audio format (sample rate)
        parts.push(self.audio_format.format_code().to_string());

        // Engine ID
        parts.push(self.engine.engine_id().to_string());

        // Authorization
        parts.push(format!("authorization={}", self.app_key));

        // Optional: interim results interval
        if self.interim_results {
            parts.push(format!(
                "resultUpdatedInterval={}",
                self.result_updated_interval
            ));
        }

        // Optional: profile ID
        if let Some(profile_id) = &self.profile_id {
            parts.push(format!("profileId={}", profile_id));
        }

        // Optional: profile words (custom vocabulary)
        if let Some(profile_words) = &self.profile_words {
            parts.push(format!("profileWords={}", profile_words));
        }

        // Optional: segmenter properties (for diarization)
        if self.enable_diarization {
            let props = self
                .segmenter_properties
                .clone()
                .unwrap_or_else(|| "useDiarizer=1".to_string());
            parts.push(format!("segmenterProperties=\"{}\"", props));
        } else if let Some(props) = &self.segmenter_properties {
            parts.push(format!("segmenterProperties=\"{}\"", props));
        }

        // --- Advanced AmiVoice request parameters -------------------------------------------
        // Each is emitted as a `key=value` token only when set, so unset knobs keep the
        // AmiVoice default. `keepFillerToken` is a 0/1 flag; the rest are numeric/string.
        if let Some(keep) = self.keep_filler_words {
            parts.push(format!("keepFillerToken={}", if keep { 1 } else { 0 }));
        }
        if let Some(t) = self.no_input_timeout {
            parts.push(format!("noInputTimeout={t}"));
        }
        if let Some(tag) = &self.usage_aggregation_tag {
            parts.push(format!("extension={tag}"));
        }
        if let Some(v) = self.max_decoding_time {
            parts.push(format!("maxDecodingTime={v}"));
        }
        if let Some(v) = self.max_response_time {
            parts.push(format!("maxResponseTime={v}"));
        }
        if let Some(v) = self.max_decoding_rate {
            parts.push(format!("maxDecodingRate={v}"));
        }
        if let Some(v) = self.target_response_time {
            parts.push(format!("targetResponseTime={v}"));
        }
        if let Some(v) = self.target_decoding_rate {
            parts.push(format!("targetDecodingRate={v}"));
        }
        if let Some(v) = self.recognition_timeout {
            parts.push(format!("recognitionTimeout={v}"));
        }

        parts.join(" ")
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.app_key.is_empty() {
            return Err("AmiVoice APPKEY is required".to_string());
        }

        if self.result_updated_interval < 100 {
            return Err("result_updated_interval must be at least 100ms".to_string());
        }

        if let Some(endpoint) = self
            .endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|endpoint| !endpoint.is_empty())
        {
            validate_amivoice_stt_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// Get list of supported engines with their IDs and names.
    pub fn supported_engines() -> Vec<(String, String)> {
        AmiVoiceEngine::all()
            .into_iter()
            .map(|e| (e.engine_id().to_string(), e.display_name().to_string()))
            .collect()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features unlock AmiVoice's diarization + interim results
    // surface — previously unreachable via the flat factory.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "amivoice".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                interim_results: Some(false),
                filler_words: Some(true),
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let cfg = AmiVoiceSTTConfig::from_standard(&std);
        assert!(cfg.enable_diarization);
        assert!(!cfg.interim_results);
        assert_eq!(cfg.keep_filler_words, Some(true)); // keep_filler_words (typed)
    }

    // WIRE-LEVEL keystone: typed filler_words + ProviderExtras knobs -> from_standard ->
    // build_start_command. The 9 advanced params must land in the actual `s` start command sent
    // on the wire (not merely on the config struct — the recurring bug class).
    #[test]
    fn from_standard_advanced_params_reach_start_command() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let extras = ProviderExtras(
            serde_json::json!({
                "no_input_timeout": 5000,
                "usage_aggregation_tag": "tenant-acme",
                "max_decoding_time": 12000,
                "max_response_time": 8000,
                "max_decoding_rate": 1.5,
                "target_response_time": 3000,
                "target_decoding_rate": 1.2,
                "recognition_timeout": 60000
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "amivoice".into(),
                api_key: "APPKEY".into(),
                ..Default::default()
            },
            features: SttFeatures {
                filler_words: Some(true),
                ..Default::default()
            },
            extras,
            translation: None,
        };
        let cfg = AmiVoiceSTTConfig::from_standard(&std);
        // Mapped onto the config...
        assert_eq!(cfg.keep_filler_words, Some(true));
        assert_eq!(cfg.no_input_timeout, Some(5000));
        assert_eq!(cfg.usage_aggregation_tag.as_deref(), Some("tenant-acme"));
        assert_eq!(cfg.max_decoding_time, Some(12000));
        assert_eq!(cfg.max_response_time, Some(8000));
        assert_eq!(cfg.max_decoding_rate, Some(1.5));
        assert_eq!(cfg.target_response_time, Some(3000));
        assert_eq!(cfg.target_decoding_rate, Some(1.2));
        assert_eq!(cfg.recognition_timeout, Some(60000));
        // ...and reach the actual `s` start command sent on the wire.
        let cmd = cfg.build_start_command();
        assert!(
            cmd.contains("keepFillerToken=1"),
            "keepFillerToken missing: {cmd}"
        );
        assert!(
            cmd.contains("noInputTimeout=5000"),
            "noInputTimeout missing: {cmd}"
        );
        assert!(
            cmd.contains("extension=tenant-acme"),
            "extension missing: {cmd}"
        );
        assert!(
            cmd.contains("maxDecodingTime=12000"),
            "maxDecodingTime missing: {cmd}"
        );
        assert!(
            cmd.contains("maxResponseTime=8000"),
            "maxResponseTime missing: {cmd}"
        );
        assert!(
            cmd.contains("maxDecodingRate=1.5"),
            "maxDecodingRate missing: {cmd}"
        );
        assert!(
            cmd.contains("targetResponseTime=3000"),
            "targetResponseTime missing: {cmd}"
        );
        assert!(
            cmd.contains("targetDecodingRate=1.2"),
            "targetDecodingRate missing: {cmd}"
        );
        assert!(
            cmd.contains("recognitionTimeout=60000"),
            "recognitionTimeout missing: {cmd}"
        );
    }

    // WIRE-LEVEL negative guard: keepFillerToken must reflect false as `0`, and unset advanced
    // params must NOT appear in the start command at all.
    #[test]
    fn unset_advanced_params_absent_and_filler_false_is_zero() {
        let mut cfg = AmiVoiceSTTConfig::default();
        cfg.app_key = "APPKEY".to_string();
        // Nothing set => none of the advanced tokens appear.
        let cmd = cfg.build_start_command();
        for tok in [
            "keepFillerToken=",
            "noInputTimeout=",
            "extension=",
            "maxDecodingTime=",
            "maxResponseTime=",
            "maxDecodingRate=",
            "targetResponseTime=",
            "targetDecodingRate=",
            "recognitionTimeout=",
        ] {
            assert!(!cmd.contains(tok), "{tok} leaked when unset: {cmd}");
        }
        // filler_words=false => keepFillerToken=0 (explicitly retain-off on the wire).
        cfg.keep_filler_words = Some(false);
        let cmd = cfg.build_start_command();
        assert!(
            cmd.contains("keepFillerToken=0"),
            "filler false not emitted as 0: {cmd}"
        );
    }

    #[test]
    fn test_engine_id() {
        assert_eq!(
            AmiVoiceEngine::HybridJapaneseGeneral.engine_id(),
            "-a-general"
        );
        assert_eq!(
            AmiVoiceEngine::E2EJapaneseGeneral.engine_id(),
            "-a2-ja-general"
        );
        assert_eq!(
            AmiVoiceEngine::E2EMultilingual.engine_id(),
            "-a2-multi-general"
        );
    }

    #[test]
    fn test_engine_from_str_relaxed() {
        assert_eq!(
            AmiVoiceEngine::from_str_relaxed("-a-general"),
            Some(AmiVoiceEngine::HybridJapaneseGeneral)
        );
        assert_eq!(
            AmiVoiceEngine::from_str_relaxed("general"),
            Some(AmiVoiceEngine::HybridJapaneseGeneral)
        );
        assert_eq!(
            AmiVoiceEngine::from_str_relaxed("e2e-ja"),
            Some(AmiVoiceEngine::E2EJapaneseGeneral)
        );
        assert_eq!(AmiVoiceEngine::from_str_relaxed("invalid"), None);
    }

    #[test]
    fn test_engine_word_registration_support() {
        assert!(AmiVoiceEngine::HybridJapaneseGeneral.supports_word_registration());
        assert!(AmiVoiceEngine::HybridJapaneseMedical.supports_word_registration());
        assert!(!AmiVoiceEngine::E2EJapaneseGeneral.supports_word_registration());
        assert!(!AmiVoiceEngine::E2EMultilingual.supports_word_registration());
    }

    #[test]
    fn test_audio_format() {
        assert_eq!(AmiVoiceAudioFormat::Pcm16kHz.format_code(), "16K");
        assert_eq!(AmiVoiceAudioFormat::Pcm8kHz.format_code(), "8K");
        assert_eq!(AmiVoiceAudioFormat::Pcm16kHz.sample_rate(), 16000);
        assert_eq!(AmiVoiceAudioFormat::Pcm8kHz.sample_rate(), 8000);
    }

    #[test]
    fn test_audio_format_from_sample_rate() {
        assert_eq!(
            AmiVoiceAudioFormat::from_sample_rate(16000),
            AmiVoiceAudioFormat::Pcm16kHz
        );
        assert_eq!(
            AmiVoiceAudioFormat::from_sample_rate(8000),
            AmiVoiceAudioFormat::Pcm8kHz
        );
        assert_eq!(
            AmiVoiceAudioFormat::from_sample_rate(44100),
            AmiVoiceAudioFormat::Pcm16kHz
        );
    }

    #[test]
    fn test_config_from_base() {
        let base = STTConfig {
            api_key: "test_app_key".to_string(),
            sample_rate: 16000,
            model: "-a-medical".to_string(),
            ..Default::default()
        };

        let config = AmiVoiceSTTConfig::from_base(base);

        assert_eq!(config.app_key, "test_app_key");
        assert_eq!(config.engine, AmiVoiceEngine::HybridJapaneseMedical);
        assert_eq!(config.audio_format, AmiVoiceAudioFormat::Pcm16kHz);
    }

    #[test]
    fn test_config_validation() {
        let config = AmiVoiceSTTConfig::default();
        assert!(config.validate().is_err());

        let mut config = AmiVoiceSTTConfig::default();
        config.app_key = "test_key".to_string();
        assert!(config.validate().is_ok());

        config.result_updated_interval = 50;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let mut config = AmiVoiceSTTConfig {
            app_key: "test_key".to_string(),
            endpoint_override: Some("wss://amivoice-proxy.example.com".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("ws://amivoice-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("ws://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("non-WebSocket endpoint_override must be rejected");
        assert!(err.contains("not allowed"), "{err}");

        config.endpoint_override = Some("https://amivoice-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("HTTP endpoint_override must be rejected for AmiVoice WebSocket dial");
        assert!(err.contains("not allowed"), "{err}");

        // SAFETY: restore the process env before releasing the test env lock.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
            } else {
                std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
            }
        }
    }

    #[test]
    fn test_build_start_command() {
        let mut config = AmiVoiceSTTConfig::default();
        config.app_key = "TEST_APP_KEY".to_string();
        config.engine = AmiVoiceEngine::HybridJapaneseGeneral;
        config.audio_format = AmiVoiceAudioFormat::Pcm16kHz;
        config.interim_results = true;
        config.result_updated_interval = 1000;

        let cmd = config.build_start_command();

        assert!(cmd.starts_with("s "));
        assert!(cmd.contains("16K"));
        assert!(cmd.contains("-a-general"));
        assert!(cmd.contains("authorization=TEST_APP_KEY"));
        assert!(cmd.contains("resultUpdatedInterval=1000"));
    }

    #[test]
    fn test_build_start_command_with_diarization() {
        let mut config = AmiVoiceSTTConfig::default();
        config.app_key = "TEST_KEY".to_string();
        config.enable_diarization = true;

        let cmd = config.build_start_command();

        assert!(cmd.contains("segmenterProperties=\"useDiarizer=1\""));
    }

    #[test]
    fn test_websocket_url() {
        let mut config = AmiVoiceSTTConfig::default();

        config.no_logging = false;
        assert_eq!(config.get_websocket_url(), AMIVOICE_WS_URL);

        config.no_logging = true;
        assert_eq!(config.get_websocket_url(), AMIVOICE_WS_NOLOG_URL);
    }

    #[test]
    fn test_websocket_url_trims_endpoint_override() {
        let mut config = AmiVoiceSTTConfig {
            endpoint_override: Some(" wss://amivoice-proxy.example.com/ ".to_string()),
            ..Default::default()
        };

        assert_eq!(
            config.get_websocket_url(),
            "wss://amivoice-proxy.example.com/v1/"
        );

        config.no_logging = true;
        assert_eq!(
            config.get_websocket_url(),
            "wss://amivoice-proxy.example.com/v1/nolog/"
        );
    }

    #[test]
    fn test_all_engines() {
        let engines = AmiVoiceEngine::all();
        assert!(engines.len() >= 19);

        // Check that all engines have valid IDs
        for engine in engines {
            assert!(!engine.engine_id().is_empty());
            assert!(!engine.display_name().is_empty());
        }
    }

    #[test]
    fn test_engine_primary_language() {
        assert_eq!(
            AmiVoiceEngine::HybridJapaneseGeneral.primary_language(),
            "ja"
        );
        assert_eq!(
            AmiVoiceEngine::HybridEnglishGeneral.primary_language(),
            "en"
        );
        assert_eq!(
            AmiVoiceEngine::HybridChineseGeneral.primary_language(),
            "zh"
        );
        assert_eq!(AmiVoiceEngine::HybridKoreanGeneral.primary_language(), "ko");
        assert_eq!(AmiVoiceEngine::E2EMultilingual.primary_language(), "multi");
    }

    #[test]
    fn test_supported_engines() {
        let engines = AmiVoiceSTTConfig::supported_engines();
        assert!(!engines.is_empty());

        // Check that the default engine is in the list
        let default_engine = AmiVoiceEngine::default();
        assert!(
            engines
                .iter()
                .any(|(id, _)| id == default_engine.engine_id())
        );
    }
}
