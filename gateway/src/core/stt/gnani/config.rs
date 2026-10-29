//! Gnani.ai STT configuration (Prisma realtime streaming).
//!
//! Gnani's realtime speech-to-text (docs.gnani.ai, "Realtime streaming") takes every setting as a
//! WebSocket upgrade header: the API key (`x-api-key-id`), `language_code`, `model`,
//! `x-sample-rate` and `format`. This module turns the gateway's config into those values and
//! refuses, before dialing, what Gnani does not take.

use crate::core::stt::base::STTConfig;
use serde::{Deserialize, Serialize};

/// Gnani's realtime speech-to-text endpoint (docs.gnani.ai, "Realtime streaming"). The former
/// gRPC service (`asr.gnani.ai`, `/Listener/DoSpeechToText`) is not in Gnani's docs, and a gRPC
/// call to it gets an HTML 403 from Cloudflare.
pub const GNANI_STT_URL: &str = "wss://api.vachana.ai/stt/v3/stream";

/// The path of [`GNANI_STT_URL`], appended to an `endpoint_override` that names only a host.
pub const GNANI_STT_PATH: &str = "/stt/v3/stream";

/// The sample rates `x-sample-rate` accepts.
pub const GNANI_STT_SAMPLE_RATES: &[u32] = &[8000, 16000, 44100, 48000];

fn validate_gnani_stt_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, &["ws", "wss"])
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

/// Gnani STT provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GnaniSTTConfig {
    /// Base STT configuration (api_key, language, sample_rate, etc.)
    #[serde(flatten)]
    pub base: STTConfig,

    /// Gnani API key, sent as `x-api-key-id`.
    #[serde(default, skip_serializing)]
    pub api_key: String,

    /// The Prisma model (`model` header).
    #[serde(default)]
    pub model: GnaniSTTModel,

    /// The output style (`format` header).
    #[serde(default)]
    pub format: GnaniSTTFormat,

    /// Test-only override of the endpoint (`ws://` or `wss://`); a bare host gets
    /// [`GNANI_STT_PATH`]. SSRF-validated.
    #[serde(default, skip_serializing)]
    pub endpoint_override: Option<String>,
}

impl Default for GnaniSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig {
                provider: "gnani".to_string(),
                api_key: String::new(),
                language: "en-IN".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "pcm16".to_string(),
                model: String::new(),
            },
            api_key: String::new(),
            model: GnaniSTTModel::default(),
            format: GnaniSTTFormat::default(),
            endpoint_override: None,
        }
    }
}

impl GnaniSTTConfig {
    /// Build from the standardized config (W1 keystone). Gnani's realtime API has none of the
    /// standardized features (no interim results: each `transcript` is a closed segment), so this
    /// is `from_base` plus the `format` extra (`verbatim`, `transcribe`, and on Prisma v3.0
    /// `codemix` or `transliterate`) and the endpoint override.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, String> {
        let mut cfg = Self::from_base(std.base.clone())?;
        if let Some(format) = std.extras.0.get("format").and_then(|v| v.as_str()) {
            cfg.format = GnaniSTTFormat::from_str(format)?;
        }
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        if let Some(endpoint) = &cfg.endpoint_override {
            validate_gnani_stt_endpoint("endpoint_override", endpoint)?;
        }
        Ok(cfg)
    }

    /// Create GnaniSTTConfig from base STTConfig. Nothing is refused here; [`Self::validate`]
    /// refuses at connect.
    pub fn from_base(base: STTConfig) -> Result<Self, String> {
        // One key, sent as `x-api-key-id`: the standardized `api_key` (the server's `GNANI_TOKEN`),
        // else `GNANI_TOKEN`. A `token|access_key` pair from the former API gives its first half.
        let api_key = crate::core::credentials::explicit_api_key(&base.api_key)
            .map(|key| match key.split_once('|') {
                Some((token, _)) => token.trim().to_string(),
                None => key,
            })
            .filter(|key| !key.is_empty())
            .or_else(|| crate::core::credentials::env_api_key("GNANI_TOKEN"))
            .unwrap_or_default();

        // An unrecognised model name is the default, as Gnani itself treats one.
        let model = GnaniSTTModel::from_str(&base.model).unwrap_or_default();

        Ok(Self {
            base,
            api_key,
            model,
            format: GnaniSTTFormat::default(),
            endpoint_override: None,
        })
    }

    /// The session's language, parsed from `base.language`.
    pub fn language(&self) -> Result<GnaniLanguage, String> {
        GnaniLanguage::from_str(&self.base.language)
    }

    /// The `language_code` sent: one of Gnani's ten languages, or `indic-auto`.
    pub fn language_code(&self) -> Result<&'static str, String> {
        let language = self.language()?;
        language.api_code().ok_or_else(|| {
            format!(
                "Gnani speech-to-text has no {} model; its languages are {} (or indic-auto)",
                language.display_name(),
                GnaniLanguage::all_codes().join(", ")
            )
        })
    }

    /// Validate the configuration: everything Gnani would refuse is refused here, before dialing.
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err(
                "Gnani API key is required. Set api_key, or the GNANI_TOKEN environment variable."
                    .to_string(),
            );
        }
        if !self.api_key.chars().all(|c| c.is_ascii_graphic()) {
            return Err("Gnani API key must be printable ASCII without spaces".to_string());
        }

        self.language_code()?;

        if !GNANI_STT_SAMPLE_RATES.contains(&self.base.sample_rate) {
            return Err(format!(
                "Gnani sample rate {} is not supported; use one of {GNANI_STT_SAMPLE_RATES:?}",
                self.base.sample_rate
            ));
        }

        if self.format.requires_v3() && self.model != GnaniSTTModel::PrismaV30 {
            return Err(format!(
                "Gnani format {} needs model prisma-v3.0 (the session's model is {})",
                self.format.as_str(),
                self.model.as_str()
            ));
        }

        if let Some(endpoint) = &self.endpoint_override {
            validate_gnani_stt_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// The URL dialed: [`GNANI_STT_URL`], or the override (a bare host gets [`GNANI_STT_PATH`]).
    pub fn ws_url(&self) -> String {
        match self
            .endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            Some(endpoint) => {
                let endpoint = endpoint.trim_end_matches('/');
                let has_path = endpoint
                    .split_once("://")
                    .is_some_and(|(_, rest)| rest.contains('/'));
                if has_path {
                    endpoint.to_string()
                } else {
                    format!("{endpoint}{GNANI_STT_PATH}")
                }
            }
            None => GNANI_STT_URL.to_string(),
        }
    }

    /// The WebSocket upgrade headers that carry the session's configuration. The VAD headers are
    /// left at Gnani's defaults.
    pub fn upgrade_headers(&self) -> Result<Vec<(&'static str, String)>, String> {
        Ok(vec![
            ("x-api-key-id", self.api_key.clone()),
            ("language_code", self.language_code()?.to_string()),
            ("model", self.model.as_str().to_string()),
            ("x-sample-rate", self.base.sample_rate.to_string()),
            ("format", self.format.as_str().to_string()),
        ])
    }
}

/// Gnani's speech-to-text models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GnaniSTTModel {
    /// Prisma v2.5, Gnani's default.
    #[default]
    #[serde(rename = "prisma-v2.5")]
    PrismaV25,
    /// Prisma v3.0: adds `detected_language_code`, `codemix` and `transliterate`.
    #[serde(rename = "prisma-v3.0")]
    PrismaV30,
}

impl GnaniSTTModel {
    /// The `model` header value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PrismaV25 => "prisma-v2.5",
            Self::PrismaV30 => "prisma-v3.0",
        }
    }

    /// Parse a model name; `None` for anything else (including `default` and the empty string).
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "prisma-v2.5" | "prisma-v2" | "v2.5" => Some(Self::PrismaV25),
            "prisma-v3.0" | "prisma-v3" | "v3.0" | "v3" => Some(Self::PrismaV30),
            _ => None,
        }
    }
}

/// Gnani's output styles (`format` header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GnaniSTTFormat {
    /// Raw spoken-form output (Gnani's default).
    #[default]
    Verbatim,
    /// Number formatting (ITN on Prisma v2.5; the model itself on v3.0).
    Transcribe,
    /// English words stay in Latin script (Prisma v3.0 only).
    Codemix,
    /// The whole transcript in Latin letters (Prisma v3.0 only).
    Transliterate,
}

impl GnaniSTTFormat {
    /// The `format` header value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Verbatim => "verbatim",
            Self::Transcribe => "transcribe",
            Self::Codemix => "codemix",
            Self::Transliterate => "transliterate",
        }
    }

    /// Parse a format name.
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "verbatim" => Ok(Self::Verbatim),
            "transcribe" => Ok(Self::Transcribe),
            "codemix" => Ok(Self::Codemix),
            "transliterate" => Ok(Self::Transliterate),
            _ => Err(format!(
                "Unsupported Gnani format: {s}. Supported: verbatim, transcribe, codemix, \
                 transliterate"
            )),
        }
    }

    /// Whether only Prisma v3.0 takes this format.
    pub fn requires_v3(&self) -> bool {
        matches!(self, Self::Codemix | Self::Transliterate)
    }
}

/// The languages a Gnani session may name: Gnani's ten, `indic-auto`, and the former API's codes
/// (accepted so existing configs parse; English variants are English, Urdu is refused).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GnaniLanguage {
    /// Kannada (kn-IN)
    #[serde(rename = "kn-IN")]
    Kannada,
    /// Hindi (hi-IN)
    #[serde(rename = "hi-IN")]
    Hindi,
    /// Tamil (ta-IN)
    #[serde(rename = "ta-IN")]
    Tamil,
    /// Telugu (te-IN)
    #[serde(rename = "te-IN")]
    Telugu,
    /// Gujarati (gu-IN)
    #[serde(rename = "gu-IN")]
    Gujarati,
    /// Marathi (mr-IN)
    #[serde(rename = "mr-IN")]
    Marathi,
    /// Bengali (bn-IN)
    #[serde(rename = "bn-IN")]
    Bengali,
    /// Malayalam (ml-IN)
    #[serde(rename = "ml-IN")]
    Malayalam,
    /// Punjabi (pa-IN; the former API's `pa-guru-IN`)
    #[serde(rename = "pa-IN", alias = "pa-guru-IN")]
    Punjabi,
    /// Urdu (ur-IN): the former API's; Gnani's current API has no Urdu, so it is refused.
    #[serde(rename = "ur-IN")]
    Urdu,
    /// English - India (en-IN)
    #[serde(rename = "en-IN")]
    #[default]
    EnglishIndia,
    /// English - UK (en-GB): sent as en-IN, Gnani's English.
    #[serde(rename = "en-GB")]
    EnglishUK,
    /// English - US (en-US): sent as en-IN, Gnani's English.
    #[serde(rename = "en-US")]
    EnglishUS,
    /// English - Singapore (en-SG): sent as en-IN, Gnani's English.
    #[serde(rename = "en-SG")]
    EnglishSG,
    /// The model identifies the language (`indic-auto`).
    #[serde(rename = "indic-auto")]
    IndicAuto,
}

impl GnaniLanguage {
    /// The language code as named.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kannada => "kn-IN",
            Self::Hindi => "hi-IN",
            Self::Tamil => "ta-IN",
            Self::Telugu => "te-IN",
            Self::Gujarati => "gu-IN",
            Self::Marathi => "mr-IN",
            Self::Bengali => "bn-IN",
            Self::Malayalam => "ml-IN",
            Self::Punjabi => "pa-IN",
            Self::Urdu => "ur-IN",
            Self::EnglishIndia => "en-IN",
            Self::EnglishUK => "en-GB",
            Self::EnglishUS => "en-US",
            Self::EnglishSG => "en-SG",
            Self::IndicAuto => "indic-auto",
        }
    }

    /// The `language_code` Gnani takes for this language: every English is `en-IN` (Gnani's
    /// English); Urdu has none.
    pub fn api_code(&self) -> Option<&'static str> {
        Some(match self {
            Self::EnglishIndia | Self::EnglishUK | Self::EnglishUS | Self::EnglishSG => "en-IN",
            Self::Urdu => return None,
            other => other.as_str(),
        })
    }

    /// Parse a language code (`hi-IN`, `hi`, `hi_IN`), a name (`hindi`), a former code
    /// (`pa-guru-IN`) or `indic-auto`.
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().replace('_', "-").as_str() {
            "kn-in" | "kn" | "kannada" => Ok(Self::Kannada),
            "hi-in" | "hi" | "hindi" => Ok(Self::Hindi),
            "ta-in" | "ta" | "tamil" => Ok(Self::Tamil),
            "te-in" | "te" | "telugu" => Ok(Self::Telugu),
            "gu-in" | "gu" | "gujarati" => Ok(Self::Gujarati),
            "mr-in" | "mr" | "marathi" => Ok(Self::Marathi),
            "bn-in" | "bn" | "bengali" => Ok(Self::Bengali),
            "ml-in" | "ml" | "malayalam" => Ok(Self::Malayalam),
            "pa-in" | "pa-guru-in" | "pa" | "punjabi" => Ok(Self::Punjabi),
            "ur-in" | "ur" | "urdu" => Ok(Self::Urdu),
            "en-in" | "en" | "english" | "english-india" => Ok(Self::EnglishIndia),
            "en-gb" | "english-uk" => Ok(Self::EnglishUK),
            "en-us" | "english-us" => Ok(Self::EnglishUS),
            "en-sg" | "english-sg" => Ok(Self::EnglishSG),
            "indic-auto" => Ok(Self::IndicAuto),
            _ => Err(format!(
                "Unsupported Gnani language: {s}. Supported: {} (or indic-auto)",
                Self::all_codes().join(", ")
            )),
        }
    }

    /// The ten language codes Gnani's speech-to-text takes.
    pub fn all_codes() -> &'static [&'static str] {
        &[
            "hi-IN", "en-IN", "ta-IN", "te-IN", "kn-IN", "ml-IN", "mr-IN", "pa-IN", "bn-IN",
            "gu-IN",
        ]
    }

    /// Get the display name for this language
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Kannada => "Kannada",
            Self::Hindi => "Hindi",
            Self::Tamil => "Tamil",
            Self::Telugu => "Telugu",
            Self::Gujarati => "Gujarati",
            Self::Marathi => "Marathi",
            Self::Bengali => "Bengali",
            Self::Malayalam => "Malayalam",
            Self::Punjabi => "Punjabi",
            Self::Urdu => "Urdu",
            Self::EnglishIndia => "English (India)",
            Self::EnglishUK => "English (UK)",
            Self::EnglishUS => "English (US)",
            Self::EnglishSG => "English (Singapore)",
            Self::IndicAuto => "automatic language identification",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(api_key: &str, language: &str, model: &str) -> STTConfig {
        STTConfig {
            provider: "gnani".to_string(),
            api_key: api_key.to_string(),
            language: language.to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm16".to_string(),
            model: model.to_string(),
        }
    }

    /// One key, from `api_key`; a former `token|access_key` pair gives its token.
    #[test]
    fn the_key_is_the_api_key_or_the_former_token() {
        let cfg = GnaniSTTConfig::from_base(base("key-123", "hi-IN", "")).unwrap();
        assert_eq!(cfg.api_key, "key-123");
        let cfg = GnaniSTTConfig::from_base(base(" tok-1 | access-1 ", "hi-IN", "")).unwrap();
        assert_eq!(cfg.api_key, "tok-1");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn a_missing_key_is_refused_at_validate() {
        let cfg = GnaniSTTConfig::default();
        let err = cfg.validate().unwrap_err();
        assert!(
            err.contains("API key") && err.contains("GNANI_TOKEN"),
            "{err}"
        );
    }

    #[test]
    fn the_model_is_parsed_and_defaults_to_prisma_v2_5() {
        let model = |m: &str| {
            GnaniSTTConfig::from_base(base("k", "hi-IN", m))
                .unwrap()
                .model
        };
        assert_eq!(model(""), GnaniSTTModel::PrismaV25);
        assert_eq!(model("default"), GnaniSTTModel::PrismaV25);
        assert_eq!(model("prisma-v2.5"), GnaniSTTModel::PrismaV25);
        assert_eq!(model("Prisma-V3.0"), GnaniSTTModel::PrismaV30);
        assert_eq!(model("prisma-v3"), GnaniSTTModel::PrismaV30);
        assert_eq!(model("nova-3"), GnaniSTTModel::PrismaV25);
        assert_eq!(GnaniSTTModel::PrismaV30.as_str(), "prisma-v3.0");
    }

    /// The former codes still parse and map onto Gnani's ten; Urdu is refused with the list.
    #[test]
    fn languages_map_onto_gnanis_ten() {
        let code = |l: &str| {
            GnaniSTTConfig::from_base(base("k", l, ""))
                .unwrap()
                .language_code()
        };
        assert_eq!(code("hi-IN").unwrap(), "hi-IN");
        assert_eq!(code("hindi").unwrap(), "hi-IN");
        assert_eq!(code("ta").unwrap(), "ta-IN");
        assert_eq!(code("kn_IN").unwrap(), "kn-IN");
        assert_eq!(code("pa-guru-IN").unwrap(), "pa-IN");
        assert_eq!(code("pa-IN").unwrap(), "pa-IN");
        assert_eq!(code("en-US").unwrap(), "en-IN");
        assert_eq!(code("en-GB").unwrap(), "en-IN");
        assert_eq!(code("en").unwrap(), "en-IN");
        assert_eq!(code("indic-auto").unwrap(), "indic-auto");
        for c in GnaniLanguage::all_codes() {
            assert_eq!(code(c).unwrap(), *c);
        }

        let urdu = GnaniSTTConfig::from_base(base("k", "ur-IN", "")).unwrap();
        let err = urdu.validate().unwrap_err();
        assert!(
            err.contains("no Urdu model") && err.contains("pa-IN"),
            "{err}"
        );
        let err = GnaniSTTConfig::from_base(base("k", "fr-FR", ""))
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(err.contains("Unsupported Gnani language: fr-FR"), "{err}");
        assert_eq!(
            serde_json::from_str::<GnaniLanguage>("\"pa-guru-IN\"").unwrap(),
            GnaniLanguage::Punjabi
        );
    }

    #[test]
    fn sample_rates_outside_gnanis_four_are_refused() {
        for rate in GNANI_STT_SAMPLE_RATES {
            let mut cfg = GnaniSTTConfig::from_base(base("k", "hi-IN", "")).unwrap();
            cfg.base.sample_rate = *rate;
            assert!(cfg.validate().is_ok(), "{rate}");
        }
        let mut cfg = GnaniSTTConfig::from_base(base("k", "hi-IN", "")).unwrap();
        cfg.base.sample_rate = 24000;
        assert!(cfg.validate().unwrap_err().contains("24000"));
    }

    #[test]
    fn v3_only_formats_need_prisma_v3() {
        use crate::core::stt::standard::StandardSTTConfig;
        let mut std = StandardSTTConfig::from_base(base("k", "hi-IN", ""));
        std.extras
            .0
            .insert("format".into(), serde_json::json!("codemix"));
        let cfg = GnaniSTTConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.format, GnaniSTTFormat::Codemix);
        assert!(cfg.validate().unwrap_err().contains("prisma-v3.0"));

        std.base.model = "prisma-v3.0".into();
        assert!(
            GnaniSTTConfig::from_standard(&std)
                .unwrap()
                .validate()
                .is_ok()
        );

        std.extras
            .0
            .insert("format".into(), serde_json::json!("bogus"));
        assert!(GnaniSTTConfig::from_standard(&std).is_err());
    }

    #[test]
    fn the_upgrade_headers_carry_the_session() {
        let mut cfg = GnaniSTTConfig::from_base(base("key-1", "en-US", "prisma-v3.0")).unwrap();
        cfg.base.sample_rate = 8000;
        let headers = cfg.upgrade_headers().unwrap();
        let get = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("x-api-key-id"), Some("key-1"));
        assert_eq!(get("language_code"), Some("en-IN"));
        assert_eq!(get("model"), Some("prisma-v3.0"));
        assert_eq!(get("x-sample-rate"), Some("8000"));
        assert_eq!(get("format"), Some("verbatim"));
    }

    #[test]
    fn the_url_is_gnanis_or_the_override_with_its_path() {
        let mut cfg = GnaniSTTConfig::default();
        assert_eq!(cfg.ws_url(), GNANI_STT_URL);
        cfg.endpoint_override = Some("ws://127.0.0.1:9000/".into());
        assert_eq!(cfg.ws_url(), "ws://127.0.0.1:9000/stt/v3/stream");
        cfg.endpoint_override = Some("wss://proxy.example.com/custom/path".into());
        assert_eq!(cfg.ws_url(), "wss://proxy.example.com/custom/path");
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = GnaniSTTConfig {
            api_key: "test-key".to_string(),
            endpoint_override: Some("wss://gnani-proxy.example.com".to_string()),
            ..Default::default()
        };

        assert!(config.validate().is_ok());

        config.endpoint_override = Some("ws://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("https://gnani-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("non-WebSocket endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");
    }
}
