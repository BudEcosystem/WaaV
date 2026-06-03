//! Configuration types for Microsoft Azure Speech-to-Text API.
//!
//! This module contains all configuration-related types including:
//! - Output format specifications
//! - Profanity handling options
//! - Provider-specific configuration options
//!
//! Note: The `AzureRegion` type is now defined in `crate::core::providers::azure`
//! and re-exported here for backwards compatibility.

use super::super::base::STTConfig;

// Re-export AzureRegion from the shared providers module for backwards compatibility
pub use crate::core::providers::azure::AzureRegion;

// =============================================================================
// Output Format
// =============================================================================

/// Azure Speech-to-Text output format options.
///
/// Controls the level of detail in recognition results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AzureOutputFormat {
    /// Simple format - returns basic recognition results with just the display text.
    ///
    /// Recommended for applications that only need the final transcription text.
    Simple,

    /// Detailed format - returns rich results including NBest alternatives,
    /// confidence scores, and optional word-level timing.
    ///
    /// Use this when you need confidence scores or alternative transcriptions.
    #[default]
    Detailed,
}

impl AzureOutputFormat {
    /// Convert to the API query parameter string value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::Detailed => "detailed",
        }
    }
}

// =============================================================================
// Profanity Handling
// =============================================================================

/// Azure Speech-to-Text profanity handling options.
///
/// Controls how profane words are handled in recognition results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AzureProfanityOption {
    /// Masked - replace profane words with asterisks.
    ///
    /// Example: "What the ****" instead of the actual word.
    #[default]
    Masked,

    /// Removed - completely remove profane words from output.
    ///
    /// The words are omitted entirely from the transcription.
    Removed,

    /// Raw - return profane words unchanged.
    ///
    /// Use when you need the exact transcription including profanity.
    Raw,
}

impl AzureProfanityOption {
    /// Convert to the API query parameter string value.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Masked => "masked",
            Self::Removed => "removed",
            Self::Raw => "raw",
        }
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Configuration specific to Microsoft Azure Speech-to-Text API.
///
/// This configuration extends the base `STTConfig` with Azure-specific
/// parameters for the WebSocket streaming API.
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::stt::azure::{AzureSTTConfig, AzureRegion, AzureOutputFormat};
/// use waav_gateway::core::stt::STTConfig;
///
/// let config = AzureSTTConfig {
///     base: STTConfig {
///         api_key: "your-subscription-key".to_string(),
///         language: "en-US".to_string(),
///         ..Default::default()
///     },
///     region: AzureRegion::WestEurope,
///     output_format: AzureOutputFormat::Detailed,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct AzureSTTConfig {
    /// Base STT configuration (shared across all providers).
    ///
    /// Contains common settings like api_key, language, sample_rate, etc.
    pub base: STTConfig,

    /// Azure region for the Speech Service endpoint.
    ///
    /// Choose the region closest to your users for optimal latency.
    pub region: AzureRegion,

    /// Output format for recognition results.
    ///
    /// - `Simple`: Basic results with just the display text
    /// - `Detailed`: Rich results with NBest alternatives and confidence scores
    pub output_format: AzureOutputFormat,

    /// Profanity handling option.
    ///
    /// Controls how profane words appear in the transcription output.
    pub profanity: AzureProfanityOption,

    /// Enable interim (partial) results during speech recognition.
    ///
    /// When `true`, the service sends `speech.hypothesis` events with
    /// partial transcriptions as audio is being processed. This enables
    /// real-time display of transcription progress.
    pub interim_results: bool,

    /// Request word-level timing information in detailed mode.
    ///
    /// Only effective when `output_format` is `Detailed`.
    /// Provides start and end times for each recognized word.
    pub word_level_timing: bool,

    /// Custom Speech endpoint ID for specialized recognition models.
    ///
    /// Use this to specify a Custom Speech model trained on your specific
    /// domain or vocabulary. Leave as `None` to use the default model.
    pub endpoint_id: Option<String>,

    /// Languages for automatic language detection.
    ///
    /// When specified, Azure will automatically detect which of these
    /// languages is being spoken and switch recognition accordingly.
    /// Leave as `None` to use the language specified in `base.language`.
    ///
    /// Example: `Some(vec!["en-US".to_string(), "es-ES".to_string()])`
    pub auto_detect_languages: Option<Vec<String>>,

    // -- Advanced `speech.context` features (USP protocol, sent as a JSON message after the
    //    handshake — distinct from the URL query params above). Each maps to a documented
    //    `speech.context` property path. See `speech_context_body` in the client. ------------
    /// Streaming speaker diarization. Maps the typed `diarization` feature onto the
    /// `phraseDetection.speakerDiarization.mode` property (`"Identity"` when on; omitted when off).
    pub speaker_diarization: bool,

    /// Endpointing / segmentation silence timeout (ms). Maps the typed `endpointing_ms` feature
    /// onto `phraseDetection.<mode>.segmentation.segmentationSilenceTimeoutMs`. When set, the
    /// segmentation `mode` is forced to `"Custom"` so Azure honors the timeout.
    pub segmentation_silence_timeout_ms: Option<u32>,

    /// Continuous automatic language-detection mode. Maps the typed `language_detection` feature
    /// onto `languageId.mode` (`"DetectContinuous"` when on). Effective with `auto_detect_languages`.
    pub language_id_continuous: bool,

    /// N-best alternatives count (detailed phrase output). Maps the typed `alternatives` feature
    /// onto `phraseOutput.detailed.options` (`["WordTimings"]` plus an N-best request) and forces
    /// `phraseOutput.format = "Detailed"`. `None` keeps the provider default.
    pub nbest_count: Option<u8>,

    /// Phrase list / dynamic grammar bias (keyterm boosting). Maps the typed `keyterms` feature
    /// onto `dgi.Groups[].Items` (the dynamic-grammar phrase list in `speech.context`).
    pub phrase_list: Vec<String>,

    /// Word-level confidence / detailed phrase-output options (e.g. `"WordTimings"`,
    /// `"SNR"`, `"Pronunciation"`). Provider-specific → `ProviderExtras` (`phrase_output_options`).
    /// Emitted under `phraseOutput.detailed.options`.
    pub phrase_output_options: Vec<String>,

    /// Disfluency / filler-word inclusion (dictation mode). Maps the typed `filler_words` feature
    /// onto `phraseDetection.mode = "Dictation"` (dictation keeps disfluencies; conversation mode
    /// drops them).
    pub dictation_mode: bool,

    /// Per-utterance sentiment analysis. Maps the typed `sentiment` feature onto
    /// `phraseDetection.sentimentAnalysis.enabled = true`.
    pub sentiment_analysis: bool,
}

impl Default for AzureSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            region: AzureRegion::default(),
            output_format: AzureOutputFormat::Detailed,
            profanity: AzureProfanityOption::Masked,
            interim_results: true,
            word_level_timing: false,
            endpoint_id: None,
            auto_detect_languages: None,
            speaker_diarization: false,
            segmentation_silence_timeout_ms: None,
            language_id_continuous: false,
            nbest_count: None,
            phrase_list: Vec::new(),
            phrase_output_options: Vec::new(),
            dictation_mode: false,
            sentiment_analysis: false,
        }
    }
}

impl AzureSTTConfig {
    /// Build the complete WebSocket URL with all query parameters.
    ///
    /// Constructs the full WebSocket URL for connecting to Azure Speech Service,
    /// including:
    /// - Regional endpoint base URL
    /// - Speech recognition API path
    /// - All configuration query parameters
    ///
    /// # Returns
    ///
    /// The complete WebSocket URL string ready for connection.
    ///
    /// # Example URL
    ///
    /// ```text
    /// wss://eastus.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1
    ///     ?language=en-US
    ///     &format=detailed
    ///     &profanity=masked
    /// ```
    pub fn build_websocket_url(&self) -> String {
        let base_url = self.region.stt_websocket_base_url();

        // NOTE: language (a locale like "en-US"), endpoint_id (a GUID) and auto-detect locales are
        // constrained identifiers with no spaces/query-delimiters, so they are not percent-encoded
        // (unlike genuinely free-text values such as Deepgram keyterms).

        // Start with the base path and required parameters
        let mut url = format!(
            "{}/speech/recognition/conversation/cognitiveservices/v1?language={}&format={}&profanity={}",
            base_url,
            self.base.language,
            self.output_format.as_str(),
            self.profanity.as_str()
        );

        // Add endpoint ID for Custom Speech models
        if let Some(ref endpoint_id) = self.endpoint_id {
            url.push_str("&cid=");
            url.push_str(endpoint_id);
        }

        // Add auto-detect languages if specified and non-empty
        if let Some(ref languages) = self.auto_detect_languages
            && !languages.is_empty()
        {
            // Azure expects comma-separated language codes for the 'languages' parameter
            url.push_str("&languages=");
            url.push_str(&languages.join(","));
        }

        url
    }

    /// The USP `speech.context` segmentation `mode` key for the active `phraseDetection.mode`.
    /// Azure nests segmentation under the lowercase recognition mode: `dictation`, `conversation`,
    /// or `interactive`. We use `conversation` for streaming recognition, `dictation` when
    /// disfluencies are requested. (Research-provided path; matches the Speech SDK
    /// `SpeechContext-phraseDetection.<mode>.segmentation.segmentationSilenceTimeoutMs` mapping.)
    fn phrase_detection_mode_key(&self) -> &'static str {
        if self.dictation_mode { "dictation" } else { "conversation" }
    }

    /// True when any advanced `speech.context` feature is set and a context message must be sent.
    pub fn has_speech_context(&self) -> bool {
        self.speaker_diarization
            || self.segmentation_silence_timeout_ms.is_some()
            || self.language_id_continuous
            || self.nbest_count.is_some()
            || !self.phrase_list.is_empty()
            || !self.phrase_output_options.is_empty()
            || self.dictation_mode
            || self.sentiment_analysis
    }

    /// Build the USP `speech.context` JSON body carrying the advanced recognition features.
    /// Returns `None` when no advanced feature is configured (so the message is skipped and the
    /// wire stays at the provider defaults). The property paths are the documented Azure Speech
    /// `speech.context` schema (research-provided / Speech-SDK `SpeechContext-*` mapping).
    pub fn build_speech_context_body(&self) -> Option<String> {
        if !self.has_speech_context() {
            return None;
        }

        let mut ctx = serde_json::Map::new();

        // --- phraseDetection ----------------------------------------------------------------
        let mut phrase_detection = serde_json::Map::new();

        // Disfluency / filler-word inclusion → phraseDetection.mode = "Dictation".
        // (Dictation keeps disfluencies; conversation mode drops them.)
        if self.dictation_mode {
            phrase_detection.insert("mode".into(), serde_json::Value::String("Dictation".into()));
        }

        // Speaker diarization → phraseDetection.speakerDiarization.mode = "Identity".
        if self.speaker_diarization {
            phrase_detection.insert(
                "speakerDiarization".into(),
                serde_json::json!({ "mode": "Identity" }),
            );
        }

        // Endpointing / segmentation silence timeout (ms) →
        // phraseDetection.<mode>.segmentation.{mode:"Custom",segmentationSilenceTimeoutMs:N}.
        if let Some(ms) = self.segmentation_silence_timeout_ms {
            phrase_detection.insert(
                self.phrase_detection_mode_key().into(),
                serde_json::json!({
                    "segmentation": {
                        "mode": "Custom",
                        "segmentationSilenceTimeoutMs": ms
                    }
                }),
            );
        }

        // Per-utterance sentiment analysis → phraseDetection.sentimentAnalysis.enabled = true.
        if self.sentiment_analysis {
            phrase_detection.insert(
                "sentimentAnalysis".into(),
                serde_json::json!({ "enabled": true }),
            );
        }

        if !phrase_detection.is_empty() {
            ctx.insert("phraseDetection".into(), serde_json::Value::Object(phrase_detection));
        }

        // --- languageId (continuous automatic language detection) ---------------------------
        if self.language_id_continuous {
            let mut language_id = serde_json::Map::new();
            language_id.insert("mode".into(), serde_json::Value::String("DetectContinuous".into()));
            if let Some(langs) = &self.auto_detect_languages
                && !langs.is_empty()
            {
                language_id.insert("languages".into(), serde_json::json!(langs));
            }
            ctx.insert("languageId".into(), serde_json::Value::Object(language_id));
        }

        // --- phraseOutput (N-best / detailed options) ---------------------------------------
        if self.nbest_count.is_some() || !self.phrase_output_options.is_empty() {
            let mut phrase_output = serde_json::Map::new();
            phrase_output.insert("format".into(), serde_json::Value::String("Detailed".into()));

            let mut detailed = serde_json::Map::new();
            // Word-level confidence / detailed options (e.g. "WordTimings"). Default to
            // "WordTimings" when only an N-best count was requested so detailed output is useful.
            let options: Vec<String> = if self.phrase_output_options.is_empty() {
                vec!["WordTimings".to_string()]
            } else {
                self.phrase_output_options.clone()
            };
            detailed.insert("options".into(), serde_json::json!(options));
            if let Some(n) = self.nbest_count {
                detailed.insert("maxNBest".into(), serde_json::json!(n));
            }
            phrase_output.insert("detailed".into(), serde_json::Value::Object(detailed));
            ctx.insert("phraseOutput".into(), serde_json::Value::Object(phrase_output));
        }

        // --- dgi (dynamic grammar / phrase list / keyterm boosting) -------------------------
        if !self.phrase_list.is_empty() {
            ctx.insert(
                "dgi".into(),
                serde_json::json!({
                    "Groups": [
                        {
                            "Type": "Generic",
                            "Items": self.phrase_list,
                        }
                    ]
                }),
            );
        }

        Some(serde_json::Value::Object(ctx).to_string())
    }

    /// Build from the standardized config (W1 keystone). Azure's advanced surface is now wired
    /// through the USP `speech.context` message (see `build_speech_context_body`): diarization,
    /// segmentation endpointing, continuous language-ID, N-best detailed output, phrase-list
    /// biasing, dictation/disfluency mode, and sentiment — in addition to the URL query-param
    /// features (interim results, word-level timing, profanity).
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let ex = &std.extras.0;
        let mut cfg = Self::from_base(std.base.clone());
        if let Some(i) = f.interim_results {
            cfg.interim_results = i;
        }
        if let Some(w) = f.word_timestamps {
            cfg.word_level_timing = w;
            // Azure only emits word offsets under the `detailed` output format; force it on so the
            // toggle is meaningful (otherwise word_level_timing is a no-op the wire never sees).
            // Review S5. (Azure cannot suppress word offsets when `false`; that is documented.)
            if w {
                cfg.output_format = AzureOutputFormat::Detailed;
            }
        }
        if let Some(p) = f.profanity_filter {
            // Azure masks profanity with asterisks when filtering is on, and returns the
            // raw words when it is off (it has no separate "remove" toggle in the standard set).
            cfg.profanity = if p {
                AzureProfanityOption::Masked
            } else {
                AzureProfanityOption::Raw
            };
        }

        // --- Advanced speech.context features ------------------------------------------------
        // Typed `diarization` → phraseDetection.speakerDiarization.mode.
        if let Some(d) = f.diarization {
            cfg.speaker_diarization = d;
        }
        // Typed `endpointing_ms` → phraseDetection.<mode>.segmentation.segmentationSilenceTimeoutMs.
        cfg.segmentation_silence_timeout_ms = f.endpointing_ms;
        // Typed `language_detection` → languageId.mode = DetectContinuous (continuous auto-detect).
        if let Some(ld) = f.language_detection {
            cfg.language_id_continuous = ld;
        }
        // Typed `alternatives` (N-best) → phraseOutput.detailed (Detailed format + maxNBest).
        cfg.nbest_count = f.alternatives;
        // Typed `keyterms` → dgi.Groups[].Items (dynamic-grammar phrase list / keyterm boosting).
        if let Some(terms) = &f.keyterms {
            cfg.phrase_list = terms.clone();
        }
        // Typed `filler_words` → phraseDetection.mode = Dictation (keeps disfluencies).
        if let Some(fw) = f.filler_words {
            cfg.dictation_mode = fw;
        }
        // Typed `sentiment` → phraseDetection.sentimentAnalysis.
        if let Some(s) = f.sentiment {
            cfg.sentiment_analysis = s;
        }
        // Extras: word-level confidence / detailed phrase-output options
        // → phraseOutput.detailed.options (provider-specific list, no typed field).
        cfg.phrase_output_options = match ex.get("phrase_output_options") {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            Some(serde_json::Value::String(s)) => {
                s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
            }
            _ => Vec::new(),
        };

        cfg
    }

    /// Create a new configuration from base STTConfig.
    ///
    /// Initializes Azure-specific settings with sensible defaults.
    pub fn from_base(base: STTConfig) -> Self {
        Self {
            base,
            ..Default::default()
        }
    }

    /// Create a configuration with a specific region.
    ///
    /// Convenience constructor for common use case of selecting a region.
    pub fn with_region(base: STTConfig, region: AzureRegion) -> Self {
        Self {
            base,
            region,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: maps the standardized features Azure can express (interim results +
    // word-level timing) onto its own config fields.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "azure".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg = AzureSTTConfig::from_standard(&std);
        assert!(!cfg.interim_results); // interim_results
        assert!(cfg.word_level_timing); // word_timestamps
    }

    // Note: Core AzureRegion tests are now in crate::core::providers::azure::region.
    // Tests here focus on STT-specific configuration and re-export verification.

    #[test]
    fn test_azure_region_reexport_works() {
        // Verify that AzureRegion is properly re-exported from the providers module
        let region = AzureRegion::default();
        assert_eq!(region, AzureRegion::EastUS);
        assert_eq!(region.as_str(), "eastus");
    }

    #[test]
    fn test_azure_region_stt_methods_available() {
        // Verify that STT-specific methods are available via re-export
        let region = AzureRegion::WestEurope;
        assert_eq!(region.stt_hostname(), "westeurope.stt.speech.microsoft.com");
        assert_eq!(
            region.stt_websocket_base_url(),
            "wss://westeurope.stt.speech.microsoft.com"
        );
    }

    #[test]
    fn test_azure_output_format() {
        assert_eq!(AzureOutputFormat::Simple.as_str(), "simple");
        assert_eq!(AzureOutputFormat::Detailed.as_str(), "detailed");
        assert_eq!(AzureOutputFormat::default(), AzureOutputFormat::Detailed);
    }

    #[test]
    fn test_azure_profanity_option() {
        assert_eq!(AzureProfanityOption::Masked.as_str(), "masked");
        assert_eq!(AzureProfanityOption::Removed.as_str(), "removed");
        assert_eq!(AzureProfanityOption::Raw.as_str(), "raw");
        assert_eq!(
            AzureProfanityOption::default(),
            AzureProfanityOption::Masked
        );
    }

    #[test]
    fn test_azure_stt_config_default() {
        let config = AzureSTTConfig::default();
        assert_eq!(config.region, AzureRegion::EastUS);
        assert_eq!(config.output_format, AzureOutputFormat::Detailed);
        assert_eq!(config.profanity, AzureProfanityOption::Masked);
        assert!(config.interim_results);
        assert!(!config.word_level_timing);
        assert!(config.endpoint_id.is_none());
        assert!(config.auto_detect_languages.is_none());
    }

    #[test]
    fn test_azure_stt_config_build_url() {
        let config = AzureSTTConfig {
            base: STTConfig {
                language: "en-US".to_string(),
                ..Default::default()
            },
            region: AzureRegion::WestEurope,
            output_format: AzureOutputFormat::Detailed,
            profanity: AzureProfanityOption::Raw,
            ..Default::default()
        };

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://westeurope.stt.speech.microsoft.com"));
        assert!(url.contains("language=en-US"));
        assert!(url.contains("format=detailed"));
        assert!(url.contains("profanity=raw"));
    }

    #[test]
    fn test_azure_stt_config_build_url_with_endpoint_id() {
        let config = AzureSTTConfig {
            base: STTConfig {
                language: "en-US".to_string(),
                ..Default::default()
            },
            endpoint_id: Some("custom-model-123".to_string()),
            ..Default::default()
        };

        let url = config.build_websocket_url();
        assert!(url.contains("cid=custom-model-123"));
    }

    #[test]
    fn test_azure_stt_config_build_url_with_auto_detect() {
        let config = AzureSTTConfig {
            base: STTConfig {
                language: "en-US".to_string(),
                ..Default::default()
            },
            auto_detect_languages: Some(vec!["en-US".to_string(), "es-ES".to_string()]),
            ..Default::default()
        };

        let url = config.build_websocket_url();
        assert!(url.contains("languages=en-US,es-ES"));
    }

    #[test]
    fn test_azure_stt_config_from_base() {
        let base = STTConfig {
            api_key: "test-key".to_string(),
            language: "de-DE".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let config = AzureSTTConfig::from_base(base.clone());
        assert_eq!(config.base.api_key, "test-key");
        assert_eq!(config.base.language, "de-DE");
        assert_eq!(config.region, AzureRegion::EastUS);
    }

    #[test]
    fn test_azure_stt_config_with_region() {
        let base = STTConfig::default();
        let config = AzureSTTConfig::with_region(base, AzureRegion::JapanEast);
        assert_eq!(config.region, AzureRegion::JapanEast);
    }

    // Note: Additional region tests (as_str, FromStr, case insensitivity) are now
    // in crate::core::providers::azure::region. Tests here focus on STT config.

    #[test]
    fn test_azure_stt_config_build_url_simple_format() {
        let config = AzureSTTConfig {
            base: STTConfig {
                language: "fr-FR".to_string(),
                ..Default::default()
            },
            output_format: AzureOutputFormat::Simple,
            profanity: AzureProfanityOption::Removed,
            ..Default::default()
        };

        let url = config.build_websocket_url();
        assert!(url.contains("format=simple"));
        assert!(url.contains("profanity=removed"));
        assert!(url.contains("language=fr-FR"));
    }

    #[test]
    fn test_azure_stt_config_build_url_empty_auto_detect() {
        // Empty auto_detect_languages should not add the parameter
        let config = AzureSTTConfig {
            base: STTConfig {
                language: "en-US".to_string(),
                ..Default::default()
            },
            auto_detect_languages: Some(vec![]),
            ..Default::default()
        };

        let url = config.build_websocket_url();
        assert!(!url.contains("languages="));
    }

    #[test]
    fn test_azure_stt_config_build_url_full_options() {
        let config = AzureSTTConfig {
            base: STTConfig {
                language: "en-GB".to_string(),
                ..Default::default()
            },
            region: AzureRegion::UKSouth,
            output_format: AzureOutputFormat::Detailed,
            profanity: AzureProfanityOption::Masked,
            interim_results: true,
            word_level_timing: true,
            endpoint_id: Some("custom-endpoint".to_string()),
            auto_detect_languages: Some(vec!["en-GB".to_string(), "en-US".to_string()]),
            ..Default::default()
        };

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://uksouth.stt.speech.microsoft.com"));
        assert!(url.contains("language=en-GB"));
        assert!(url.contains("format=detailed"));
        assert!(url.contains("profanity=masked"));
        assert!(url.contains("cid=custom-endpoint"));
        assert!(url.contains("languages=en-GB,en-US"));
    }

    // =========================================================================
    // WIRE-LEVEL tests for the advanced `speech.context` features. These assert
    // the api_param reaches the SERIALIZED `speech.context` JSON body (the exact
    // USP message bytes sent to Azure) — NOT merely the config struct (the
    // recurring "set on struct, never on wire" bug class). Each goes end-to-end
    // through `from_standard` → `build_speech_context_body`.
    // =========================================================================

    use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};

    fn ctx_json(features: SttFeatures, extras: serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "azure".into(),
                api_key: "k".into(),
                language: "en-US".into(),
                ..Default::default()
            },
            features,
            extras: ProviderExtras(extras),
        };
        let cfg = AzureSTTConfig::from_standard(&std);
        let body = cfg
            .build_speech_context_body()
            .expect("advanced features must produce a speech.context body");
        serde_json::from_str(&body).expect("speech.context body must be valid JSON")
    }

    /// No advanced features → no speech.context message at all (wire stays at Azure defaults).
    #[test]
    fn no_advanced_features_means_no_speech_context() {
        let cfg = AzureSTTConfig::from_standard(&StandardSTTConfig {
            base: STTConfig { provider: "azure".into(), api_key: "k".into(), ..Default::default() },
            features: SttFeatures::default(),
            extras: ProviderExtras::default(),
        });
        assert!(!cfg.has_speech_context());
        assert!(cfg.build_speech_context_body().is_none());
    }

    /// diarization (typed) → phraseDetection.speakerDiarization.mode.
    #[test]
    fn diarization_reaches_speech_context() {
        let v = ctx_json(SttFeatures { diarization: Some(true), ..Default::default() }, Default::default());
        assert_eq!(
            v["phraseDetection"]["speakerDiarization"]["mode"], "Identity",
            "speakerDiarization.mode must reach the speech.context body: {v}"
        );
    }

    /// endpointing_ms (typed) → phraseDetection.conversation.segmentation.segmentationSilenceTimeoutMs
    /// with segmentation.mode = "Custom".
    #[test]
    fn segmentation_timeout_reaches_speech_context() {
        let v = ctx_json(SttFeatures { endpointing_ms: Some(720), ..Default::default() }, Default::default());
        let seg = &v["phraseDetection"]["conversation"]["segmentation"];
        assert_eq!(seg["segmentationSilenceTimeoutMs"], 720, "timeout must reach the body: {v}");
        assert_eq!(seg["mode"], "Custom", "segmentation mode must be Custom to honor the timeout: {v}");
    }

    /// language_detection (typed) → languageId.mode = "DetectContinuous".
    #[test]
    fn language_id_continuous_reaches_speech_context() {
        let v = ctx_json(SttFeatures { language_detection: Some(true), ..Default::default() }, Default::default());
        assert_eq!(v["languageId"]["mode"], "DetectContinuous", "languageId.mode must reach the body: {v}");
    }

    /// alternatives (typed) → phraseOutput.format=Detailed + phraseOutput.detailed.maxNBest.
    #[test]
    fn nbest_reaches_speech_context_detailed_output() {
        let v = ctx_json(SttFeatures { alternatives: Some(5), ..Default::default() }, Default::default());
        assert_eq!(v["phraseOutput"]["format"], "Detailed", "phraseOutput.format must be Detailed: {v}");
        assert_eq!(v["phraseOutput"]["detailed"]["maxNBest"], 5, "maxNBest must reach the body: {v}");
    }

    /// keyterms (typed) → dgi.Groups[].Items (dynamic-grammar phrase list / keyterm boosting).
    #[test]
    fn keyterms_reach_speech_context_dgi() {
        let v = ctx_json(
            SttFeatures { keyterms: Some(vec!["WaaV".into(), "Cognitive Services".into()]), ..Default::default() },
            Default::default(),
        );
        let items = &v["dgi"]["Groups"][0]["Items"];
        assert_eq!(items[0], "WaaV", "phrase-list item must reach dgi.Groups[].Items: {v}");
        assert_eq!(items[1], "Cognitive Services");
    }

    /// filler_words (typed) → phraseDetection.mode = "Dictation" (keeps disfluencies), and the
    /// segmentation timeout then nests under "dictation".
    #[test]
    fn dictation_mode_reaches_speech_context() {
        let v = ctx_json(
            SttFeatures { filler_words: Some(true), endpointing_ms: Some(500), ..Default::default() },
            Default::default(),
        );
        assert_eq!(v["phraseDetection"]["mode"], "Dictation", "phraseDetection.mode must be Dictation: {v}");
        // Segmentation nests under the dictation mode key when dictation is active.
        assert_eq!(
            v["phraseDetection"]["dictation"]["segmentation"]["segmentationSilenceTimeoutMs"], 500,
            "segmentation must nest under dictation when dictation_mode is on: {v}"
        );
    }

    /// sentiment (typed) → phraseDetection.sentimentAnalysis.enabled = true.
    #[test]
    fn sentiment_reaches_speech_context() {
        let v = ctx_json(SttFeatures { sentiment: Some(true), ..Default::default() }, Default::default());
        assert_eq!(
            v["phraseDetection"]["sentimentAnalysis"]["enabled"], true,
            "sentimentAnalysis.enabled must reach the body: {v}"
        );
    }

    /// phrase_output_options (extras) → phraseOutput.detailed.options.
    #[test]
    fn phrase_output_options_reach_speech_context() {
        let mut ex = serde_json::Map::new();
        ex.insert("phrase_output_options".into(), serde_json::json!(["WordTimings", "SNR", "Pronunciation"]));
        let v = ctx_json(SttFeatures::default(), ex);
        let opts = &v["phraseOutput"]["detailed"]["options"];
        assert_eq!(opts[0], "WordTimings", "detailed options must reach the body: {v}");
        assert_eq!(opts[1], "SNR");
        assert_eq!(opts[2], "Pronunciation");
    }

    /// KEYSTONE: all eight features land on a single speech.context body together.
    #[test]
    fn from_standard_all_speech_context_features_reach_the_wire() {
        let mut ex = serde_json::Map::new();
        ex.insert("phrase_output_options".into(), serde_json::json!(["WordTimings"]));
        let v = ctx_json(
            SttFeatures {
                diarization: Some(true),
                endpointing_ms: Some(640),
                language_detection: Some(true),
                alternatives: Some(3),
                keyterms: Some(vec!["WaaV".into()]),
                filler_words: Some(true),
                sentiment: Some(true),
                ..Default::default()
            },
            ex,
        );
        // Dictation is on, so segmentation nests under "dictation".
        assert_eq!(v["phraseDetection"]["mode"], "Dictation");
        assert_eq!(v["phraseDetection"]["speakerDiarization"]["mode"], "Identity");
        assert_eq!(
            v["phraseDetection"]["dictation"]["segmentation"]["segmentationSilenceTimeoutMs"], 640
        );
        assert_eq!(v["phraseDetection"]["sentimentAnalysis"]["enabled"], true);
        assert_eq!(v["languageId"]["mode"], "DetectContinuous");
        assert_eq!(v["phraseOutput"]["format"], "Detailed");
        assert_eq!(v["phraseOutput"]["detailed"]["maxNBest"], 3);
        assert_eq!(v["phraseOutput"]["detailed"]["options"][0], "WordTimings");
        assert_eq!(v["dgi"]["Groups"][0]["Items"][0], "WaaV");
    }
}
