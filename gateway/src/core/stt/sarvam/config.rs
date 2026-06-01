//! Sarvam.ai STT Configuration
//!
//! Configuration structures for the Sarvam.ai Saarika speech-to-text model.
//! Sarvam specializes in Indian language STT with support for 11 languages.

use crate::core::stt::base::STTConfig;

/// Sarvam.ai STT WebSocket endpoint
pub const SARVAM_STT_WS_URL: &str = "wss://api.sarvam.ai/speech-to-text-translate";

/// Default STT model (Saarika v2.5)
pub const DEFAULT_MODEL: &str = "saarika:v2.5";

/// Default sample rate in Hz
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Keep-alive interval in seconds (Sarvam disconnects after 60s idle)
pub const KEEPALIVE_INTERVAL_SECS: u64 = 30;

/// Connection timeout in seconds
pub const CONNECTION_TIMEOUT_SECS: u64 = 10;

/// Message receive timeout in seconds
pub const MESSAGE_TIMEOUT_SECS: u64 = 60;

/// Supported languages for Sarvam STT
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "hi-IN", // Hindi
    "bn-IN", // Bengali
    "ta-IN", // Tamil
    "te-IN", // Telugu
    "gu-IN", // Gujarati
    "kn-IN", // Kannada
    "ml-IN", // Malayalam
    "mr-IN", // Marathi
    "od-IN", // Odia
    "pa-IN", // Punjabi
    "en-IN", // English (Indian)
];

/// Configuration specific to Sarvam STT
#[derive(Debug, Clone)]
pub struct SarvamSTTConfig {
    /// Model to use (default: saarika:v2.5)
    pub model: String,
    /// Language code (e.g., "hi-IN", "en-IN")
    pub language_code: String,
    /// Sample rate in Hz (8000 or 16000)
    pub sample_rate: u32,
    /// Audio input codec (wav, pcm_s16le)
    pub input_audio_codec: String,
    /// Enable enhanced VAD sensitivity
    pub high_vad_sensitivity: bool,
    /// Enable VAD signals (speech_start/speech_end events)
    pub vad_signals: bool,
    /// Enable manual flush signal support
    pub flush_signal: bool,
}

impl Default for SarvamSTTConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            language_code: "hi-IN".to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            input_audio_codec: "pcm_s16le".to_string(),
            high_vad_sensitivity: false,
            vad_signals: true,
            flush_signal: false,
        }
    }
}

impl SarvamSTTConfig {
    /// Create a SarvamSTTConfig from the base STTConfig
    pub fn from_base(config: &STTConfig) -> Self {
        Self {
            model: if config.model.is_empty() {
                DEFAULT_MODEL.to_string()
            } else {
                config.model.clone()
            },
            language_code: if config.language.is_empty() {
                "hi-IN".to_string()
            } else {
                config.language.clone()
            },
            sample_rate: if config.sample_rate == 0 {
                DEFAULT_SAMPLE_RATE
            } else {
                config.sample_rate
            },
            input_audio_codec: if config.encoding.is_empty() || config.encoding == "linear16" {
                "pcm_s16le".to_string()
            } else {
                config.encoding.clone()
            },
            high_vad_sensitivity: false,
            vad_signals: true,
            flush_signal: false,
        }
    }

    /// Build from the standardized config (W1 keystone). Sarvam's Saarika streaming surface is
    /// narrow, so this maps the one standardized feature it can actually express: explicit
    /// voice-activity events (`vad_events` -> `vad_signals`, the speech_start/speech_end signals).
    /// Features Sarvam cannot express (interim_results, diarization, word_timestamps, smart_format,
    /// profanity_filter, filler_words, endpointing/utterance_end, keyterms, redaction,
    /// entity/language detection) are capability gaps and stay at default.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base);
        if let Some(v) = f.vad_events {
            cfg.vad_signals = v;
        }
        cfg
    }

    /// Build the WebSocket URL with query parameters
    pub fn build_websocket_url(&self) -> String {
        let mut url = String::with_capacity(256);
        // NOTE: model/language_code/input_audio_codec are constrained identifiers (e.g.
        // "saarika:v2.5", "en-IN", "pcm_s16le") — they never contain spaces or query delimiters, and
        // the `:` in model ids is valid in a query and must stay literal, so they are NOT
        // percent-encoded here (unlike genuinely free-text values such as Deepgram keyterms).
        url.push_str(SARVAM_STT_WS_URL);
        url.push_str("?model=");
        url.push_str(&self.model);
        url.push_str("&language_code=");
        url.push_str(&self.language_code);
        url.push_str("&sample_rate=");
        url.push_str(&self.sample_rate.to_string());
        url.push_str("&input_audio_codec=");
        url.push_str(&self.input_audio_codec);

        if self.vad_signals {
            url.push_str("&vad_signals=true");
        }

        if self.high_vad_sensitivity {
            url.push_str("&high_vad_sensitivity=true");
        }

        if self.flush_signal {
            url.push_str("&flush_signal=true");
        }

        url
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate sample rate
        if self.sample_rate != 8000 && self.sample_rate != 16000 {
            return Err(format!(
                "Invalid sample rate: {}. Sarvam only supports 8000 or 16000 Hz",
                self.sample_rate
            ));
        }

        // Validate language code
        if !SUPPORTED_LANGUAGES.contains(&self.language_code.as_str()) {
            return Err(format!(
                "Unsupported language: {}. Supported languages: {:?}",
                self.language_code, SUPPORTED_LANGUAGES
            ));
        }

        // Validate audio codec
        let valid_codecs = ["wav", "pcm_s16le", "pcm"];
        if !valid_codecs.contains(&self.input_audio_codec.as_str()) {
            return Err(format!(
                "Invalid audio codec: {}. Supported: wav, pcm_s16le",
                self.input_audio_codec
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized `vad_events` feature unlocks Sarvam's voice-activity
    // signals (speech_start/speech_end), and the base (provider/api_key) carries through.
    #[test]
    fn from_standard_maps_vad_events() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "sarvam".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                vad_events: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg = SarvamSTTConfig::from_standard(&std);
        assert!(!cfg.vad_signals); // vad_events -> vad_signals
    }

    #[test]
    fn test_default_config() {
        let config = SarvamSTTConfig::default();
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.language_code, "hi-IN");
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.input_audio_codec, "pcm_s16le");
        assert!(config.vad_signals);
        assert!(!config.high_vad_sensitivity);
        assert!(!config.flush_signal);
    }

    #[test]
    fn test_from_base_config() {
        let base = STTConfig {
            model: "saarika:v2.5".to_string(),
            provider: "sarvam".to_string(),
            api_key: "test_key".to_string(),
            language: "ta-IN".to_string(),
            sample_rate: 8000,
            channels: 1,
            punctuation: true,
            encoding: "wav".to_string(),
        };

        let config = SarvamSTTConfig::from_base(&base);
        assert_eq!(config.model, "saarika:v2.5");
        assert_eq!(config.language_code, "ta-IN");
        assert_eq!(config.sample_rate, 8000);
        assert_eq!(config.input_audio_codec, "wav");
    }

    #[test]
    fn test_from_base_config_defaults() {
        let base = STTConfig {
            model: String::new(),
            provider: "sarvam".to_string(),
            api_key: "test_key".to_string(),
            language: String::new(),
            sample_rate: 0,
            channels: 1,
            punctuation: true,
            encoding: String::new(),
        };

        let config = SarvamSTTConfig::from_base(&base);
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.language_code, "hi-IN");
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.input_audio_codec, "pcm_s16le");
    }

    #[test]
    fn test_build_websocket_url() {
        let config = SarvamSTTConfig::default();
        let url = config.build_websocket_url();

        assert!(url.starts_with(SARVAM_STT_WS_URL));
        assert!(url.contains("model=saarika:v2.5"));
        assert!(url.contains("language_code=hi-IN"));
        assert!(url.contains("sample_rate=16000"));
        assert!(url.contains("input_audio_codec=pcm_s16le"));
        assert!(url.contains("vad_signals=true"));
    }

    #[test]
    fn test_validate_valid_config() {
        let config = SarvamSTTConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_sample_rate() {
        let mut config = SarvamSTTConfig::default();
        config.sample_rate = 44100;
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("sample rate"));
    }

    #[test]
    fn test_validate_invalid_language() {
        let mut config = SarvamSTTConfig::default();
        config.language_code = "fr-FR".to_string();
        assert!(config.validate().is_err());
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("Unsupported language")
        );
    }

    #[test]
    fn test_validate_invalid_codec() {
        let mut config = SarvamSTTConfig::default();
        config.input_audio_codec = "mp3".to_string();
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("audio codec"));
    }

    #[test]
    fn test_supported_languages() {
        assert!(SUPPORTED_LANGUAGES.contains(&"hi-IN"));
        assert!(SUPPORTED_LANGUAGES.contains(&"en-IN"));
        assert!(SUPPORTED_LANGUAGES.contains(&"ta-IN"));
        assert!(!SUPPORTED_LANGUAGES.contains(&"en-US"));
    }
}
