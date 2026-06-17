//! Sarvam.ai STT Configuration
//!
//! Configuration structures for the Sarvam.ai Saarika speech-to-text model.
//! Sarvam specializes in Indian language STT with support for 11 languages.

use crate::core::stt::base::STTConfig;
use url::form_urlencoded;

/// Sarvam.ai STT streaming WebSocket endpoint.
///
/// NOTE: `/speech-to-text-translate` (no `/ws`) is the BATCH REST (POST) endpoint and returns HTTP
/// 405 on a WS upgrade — the streaming WS lives at `/speech-to-text/ws`. (Found by live testing;
/// see <https://docs.sarvam.ai/api-reference-docs/speech-to-text/transcribe/ws>.)
pub const SARVAM_STT_WS_URL: &str = "wss://api.sarvam.ai/speech-to-text/ws";

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
    // -------------------------------------------------------------------------
    // Provider-extras passthrough params (Sarvam Saarika streaming query params not modeled by the
    // typed `SttFeatures` vocabulary). All optional; `None` => omitted from the URL (provider
    // default). Carried verbatim from `StandardSTTConfig::extras`.
    // -------------------------------------------------------------------------
    /// Transcription `mode` (e.g. transcription vs. translation behavior). Sarvam streaming `mode`
    /// query param.
    pub mode: Option<String>,
    /// ASR prompt / biasing text that nudges recognition toward expected vocabulary. Sarvam
    /// streaming `prompt` query param (free text — percent-encoded on the wire).
    pub prompt: Option<String>,
    /// VAD: positive speech probability threshold (0.0-1.0) above which a frame counts as speech.
    pub positive_speech_threshold: Option<f64>,
    /// VAD: negative speech probability threshold (0.0-1.0) below which a frame counts as silence.
    pub negative_speech_threshold: Option<f64>,
    /// VAD: minimum consecutive speech frames required to emit a speech segment.
    pub min_speech_frames: Option<u32>,
    /// VAD: minimum speech frames required specifically for the first turn of the session.
    pub first_turn_min_speech_frames: Option<u32>,
    /// VAD: number of negative (silence) frames that close a segment.
    pub negative_frames_count: Option<u32>,
    /// VAD: sliding window (in frames) over which negative frames are counted.
    pub negative_frames_window: Option<u32>,
    /// VAD: start-of-speech volume threshold (0.0-1.0) gating segment onset.
    pub start_speech_volume_threshold: Option<f64>,
    /// VAD: minimum speech frames required to treat speech as a barge-in / interrupt.
    pub interrupt_min_speech_frames: Option<u32>,
    /// VAD: number of audio frames pre-pended to a segment as lead-in padding.
    pub pre_speech_pad_frames: Option<u32>,
    /// VAD: number of initial frames to ignore at session start (warm-up).
    pub num_initial_ignored_frames: Option<u32>,
    /// Test-only base-URL override (scheme+host); redirects the WS dial, keeping the path/query.
    // NOTE: `#[serde(skip)]` is intentionally omitted — `SarvamSTTConfig` derives only
    // `Debug, Clone` (no `Serialize`/`Deserialize`), so a bare serde attribute here would be an
    // unregistered-attribute compile error. The field is non-serialized regardless.
    pub endpoint_override: Option<String>,
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
            mode: None,
            prompt: None,
            positive_speech_threshold: None,
            negative_speech_threshold: None,
            min_speech_frames: None,
            first_turn_min_speech_frames: None,
            negative_frames_count: None,
            negative_frames_window: None,
            start_speech_volume_threshold: None,
            interrupt_min_speech_frames: None,
            pre_speech_pad_frames: None,
            num_initial_ignored_frames: None,
            endpoint_override: None,
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
            // Extras-only params are absent on the flat path (no extras to read here).
            ..Default::default()
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
        // Provider extras → Sarvam streaming query params not modeled by the typed vocabulary:
        // transcription `mode`, ASR `prompt`/biasing, and the fine-grained VAD tuning knobs.
        // Keys not present are left as `None` and omitted from the URL (provider default).
        let e = &std.extras.0;
        if let Some(v) = e.get("mode").and_then(|v| v.as_str()) {
            cfg.mode = Some(v.to_string());
        }
        if let Some(v) = e.get("prompt").and_then(|v| v.as_str()) {
            cfg.prompt = Some(v.to_string());
        }
        if let Some(v) = e.get("positive_speech_threshold").and_then(|v| v.as_f64()) {
            cfg.positive_speech_threshold = Some(v);
        }
        if let Some(v) = e.get("negative_speech_threshold").and_then(|v| v.as_f64()) {
            cfg.negative_speech_threshold = Some(v);
        }
        if let Some(v) = e.get("min_speech_frames").and_then(|v| v.as_u64()) {
            cfg.min_speech_frames = Some(v as u32);
        }
        if let Some(v) = e
            .get("first_turn_min_speech_frames")
            .and_then(|v| v.as_u64())
        {
            cfg.first_turn_min_speech_frames = Some(v as u32);
        }
        if let Some(v) = e.get("negative_frames_count").and_then(|v| v.as_u64()) {
            cfg.negative_frames_count = Some(v as u32);
        }
        if let Some(v) = e.get("negative_frames_window").and_then(|v| v.as_u64()) {
            cfg.negative_frames_window = Some(v as u32);
        }
        if let Some(v) = e
            .get("start_speech_volume_threshold")
            .and_then(|v| v.as_f64())
        {
            cfg.start_speech_volume_threshold = Some(v);
        }
        if let Some(v) = e.get("interrupt_min_speech_frames").and_then(|v| v.as_u64()) {
            cfg.interrupt_min_speech_frames = Some(v as u32);
        }
        if let Some(v) = e.get("pre_speech_pad_frames").and_then(|v| v.as_u64()) {
            cfg.pre_speech_pad_frames = Some(v as u32);
        }
        if let Some(v) = e.get("num_initial_ignored_frames").and_then(|v| v.as_u64()) {
            cfg.num_initial_ignored_frames = Some(v as u32);
        }
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        cfg
    }

    /// Build the WebSocket URL with query parameters
    pub fn build_websocket_url(&self) -> String {
        let mut url = String::with_capacity(256);
        // NOTE: model/language_code/input_audio_codec are constrained identifiers (e.g.
        // "saarika:v2.5", "en-IN", "pcm_s16le") — they never contain spaces or query delimiters, and
        // the `:` in model ids is valid in a query and must stay literal, so they are NOT
        // percent-encoded here (unlike genuinely free-text values such as Deepgram keyterms).
        // Honor a base-URL override (scheme+host) for tests/regional dials, keeping the
        // `/speech-to-text/ws` path and the query below unchanged. Empty override => production base.
        match self.endpoint_override.as_deref().filter(|o| !o.is_empty()) {
            Some(o) => {
                url.push_str(o.trim_end_matches('/'));
                url.push_str("/speech-to-text/ws");
            }
            None => url.push_str(SARVAM_STT_WS_URL),
        }
        url.push_str("?model=");
        url.push_str(&self.model);
        url.push_str("&language-code=");
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

        // Provider-extras passthrough params. `mode`/`prompt` are free text (the `prompt` biasing
        // string may contain spaces and delimiters) and so ARE percent-encoded here, unlike the
        // constrained identifiers above.
        let encode =
            |s: &str| -> String { form_urlencoded::byte_serialize(s.as_bytes()).collect() };
        if let Some(ref mode) = self.mode {
            url.push_str("&mode=");
            url.push_str(&encode(mode));
        }
        if let Some(ref prompt) = self.prompt {
            url.push_str("&prompt=");
            url.push_str(&encode(prompt));
        }
        if let Some(v) = self.positive_speech_threshold {
            url.push_str(&format!("&positive_speech_threshold={v}"));
        }
        if let Some(v) = self.negative_speech_threshold {
            url.push_str(&format!("&negative_speech_threshold={v}"));
        }
        if let Some(v) = self.min_speech_frames {
            url.push_str(&format!("&min_speech_frames={v}"));
        }
        if let Some(v) = self.first_turn_min_speech_frames {
            url.push_str(&format!("&first_turn_min_speech_frames={v}"));
        }
        if let Some(v) = self.negative_frames_count {
            url.push_str(&format!("&negative_frames_count={v}"));
        }
        if let Some(v) = self.negative_frames_window {
            url.push_str(&format!("&negative_frames_window={v}"));
        }
        if let Some(v) = self.start_speech_volume_threshold {
            url.push_str(&format!("&start_speech_volume_threshold={v}"));
        }
        if let Some(v) = self.interrupt_min_speech_frames {
            url.push_str(&format!("&interrupt_min_speech_frames={v}"));
        }
        if let Some(v) = self.pre_speech_pad_frames {
            url.push_str(&format!("&pre_speech_pad_frames={v}"));
        }
        if let Some(v) = self.num_initial_ignored_frames {
            url.push_str(&format!("&num_initial_ignored_frames={v}"));
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
            translation: None,
        };
        let cfg = SarvamSTTConfig::from_standard(&std);
        assert!(!cfg.vad_signals); // vad_events -> vad_signals
    }

    // WIRE-LEVEL: `mode`, `prompt` and the 10 VAD tuning extras must travel from the standardized
    // `extras` passthrough onto the streaming WebSocket URL — the bytes that actually reach Sarvam,
    // not merely the config struct. Guards the recurring "set on the struct but never emitted to
    // the wire" gap class.
    #[test]
    fn extras_mode_prompt_and_vad_params_reach_websocket_url() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("mode".into(), serde_json::json!("transcription"));
        extras.insert("prompt".into(), serde_json::json!("WaaV gateway demo"));
        extras.insert("positive_speech_threshold".into(), serde_json::json!(0.6));
        extras.insert("negative_speech_threshold".into(), serde_json::json!(0.35));
        extras.insert("min_speech_frames".into(), serde_json::json!(3));
        extras.insert("first_turn_min_speech_frames".into(), serde_json::json!(5));
        extras.insert("negative_frames_count".into(), serde_json::json!(8));
        extras.insert("negative_frames_window".into(), serde_json::json!(16));
        extras.insert("start_speech_volume_threshold".into(), serde_json::json!(0.2));
        extras.insert("interrupt_min_speech_frames".into(), serde_json::json!(4));
        extras.insert("pre_speech_pad_frames".into(), serde_json::json!(2));
        extras.insert("num_initial_ignored_frames".into(), serde_json::json!(10));

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "sarvam".into(),
                api_key: "test-key".into(),
                language: "hi-IN".into(),
                ..Default::default()
            },
            features: Default::default(),
            extras: ProviderExtras(extras),
            translation: None,
        };
        let cfg = SarvamSTTConfig::from_standard(&std);
        let url = cfg.build_websocket_url();

        // mode + prompt (prompt is free text -> form-urlencoded; spaces become `+`, the same
        // x-www-form-urlencoded convention `url::form_urlencoded` and the Rev AI builder use).
        assert!(url.contains("mode=transcription"), "mode missing: {url}");
        assert!(
            url.contains("prompt=WaaV+gateway+demo"),
            "prompt must be url-encoded on the wire (spaces -> '+'): {url}"
        );
        // 10 VAD params.
        for needle in [
            "positive_speech_threshold=0.6",
            "negative_speech_threshold=0.35",
            "min_speech_frames=3",
            "first_turn_min_speech_frames=5",
            "negative_frames_count=8",
            "negative_frames_window=16",
            "start_speech_volume_threshold=0.2",
            "interrupt_min_speech_frames=4",
            "pre_speech_pad_frames=2",
            "num_initial_ignored_frames=10",
        ] {
            assert!(url.contains(needle), "VAD param `{needle}` missing from URL: {url}");
        }
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
        assert!(url.contains("language-code=hi-IN"));
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
