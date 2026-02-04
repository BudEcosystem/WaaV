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
        }
    }

    /// Get the WebSocket URL.
    pub fn get_websocket_url(&self) -> &'static str {
        if self.no_logging {
            AMIVOICE_WS_NOLOG_URL
        } else {
            AMIVOICE_WS_URL
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
