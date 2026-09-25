use std::time::Duration;

use crate::core::stt::base::STTConfig;

/// The Speech-to-Text v2 location used when the caller names none. `global` is served by the
/// un-prefixed `speech.googleapis.com`; every other location needs its regional endpoint.
pub(crate) const GOOGLE_STT_DEFAULT_LOCATION: &str = "global";

/// Error text for a malformed location. Deliberately does not echo the value.
const INVALID_LOCATION_MESSAGE: &str = "Google STT location must match ^[a-z0-9-]{1,40}$ \
     (e.g. 'global', 'us', 'eu', 'us-central1')";

/// `^[a-z0-9-]{1,40}$`. The location is interpolated into the recognizer resource name AND the
/// regional hostname (`{location}-speech.googleapis.com`), so anything wider (a `.`, `/`, `@`,
/// `:`) could redirect the host.
pub(crate) fn is_valid_google_stt_location(location: &str) -> bool {
    (1..=40).contains(&location.len())
        && location
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn validate_google_stt_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }

    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|e| format!("{source} rejected (SSRF protection): {e}"))
}

/// A single transcript-normalization replacement entry (Google v2
/// `TranscriptNormalization.Entry`): replace `search` with `replace`, optionally case-sensitive.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TranscriptNormEntry {
    /// Text to search for (max 100 chars).
    pub search: String,
    /// Replacement text (max 100 chars).
    pub replace: String,
    /// Whether the search is case sensitive.
    #[serde(default)]
    pub case_sensitive: bool,
}

/// Configuration specific to Google Speech-to-Text v2 API.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GoogleSTTConfig {
    pub base: STTConfig,
    pub project_id: String,
    pub location: String,
    pub recognizer_id: Option<String>,
    pub interim_results: bool,
    pub enable_voice_activity_events: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_duration_serde"
    )]
    pub speech_start_timeout: Option<Duration>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_duration_serde"
    )]
    pub speech_end_timeout: Option<Duration>,
    pub single_utterance: bool,

    // --- Advanced RecognitionFeatures (v2 RecognitionConfig.features), wired by W2 -------------
    /// Replace profanities with asterisks (`features.profanity_filter`).
    #[serde(default)]
    pub profanity_filter: bool,
    /// Per-word start/end time offsets (`features.enable_word_time_offsets`).
    #[serde(default)]
    pub enable_word_time_offsets: bool,
    /// Per-word confidence scores (`features.enable_word_confidence`).
    #[serde(default)]
    pub enable_word_confidence: bool,
    /// Replace spoken punctuation with symbols (`features.enable_spoken_punctuation`).
    #[serde(default)]
    pub enable_spoken_punctuation: bool,
    /// Replace spoken emojis with Unicode (`features.enable_spoken_emojis`).
    #[serde(default)]
    pub enable_spoken_emojis: bool,
    /// Transcribe each channel independently (`features.multi_channel_mode`).
    #[serde(default)]
    pub multichannel: bool,
    /// Maximum N-best hypotheses (`features.max_alternatives`, valid 0-30).
    #[serde(default)]
    pub max_alternatives: i32,
    /// Enable speaker diarization (`features.diarization_config`).
    #[serde(default)]
    pub diarization: bool,
    /// Min speaker count for diarization (only used when `diarization` is true).
    #[serde(default)]
    pub diarization_min_speakers: i32,
    /// Max speaker count for diarization (only used when `diarization` is true).
    #[serde(default)]
    pub diarization_max_speakers: i32,
    /// Phrase-set adaptation / keyterm boosting (`config.adaptation`, inline PhraseSet).
    #[serde(default)]
    pub adaptation_phrases: Vec<String>,
    /// Transcript normalization replacement entries (`config.transcript_normalization`).
    #[serde(default)]
    pub transcript_normalization: Vec<TranscriptNormEntry>,

    // --- Test/operational endpoint + auth overrides (mirrors GoogleTTSConfig) -------------------
    /// Override the gRPC endpoint the provider connects to instead of the production Speech
    /// endpoint. An `http://` value selects a PLAINTEXT channel (a localhost tonic mock for e2e).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_override: Option<String>,
    /// A pre-minted OAuth access token to use verbatim as the bearer credential, bypassing the
    /// network OAuth fetch. Lets a mock e2e test authenticate with no Google network round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_access_token: Option<String>,
}

impl Default for GoogleSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig {
                model: "latest_long".to_string(),
                ..STTConfig::default()
            },
            project_id: String::new(),
            location: GOOGLE_STT_DEFAULT_LOCATION.to_string(),
            recognizer_id: None,
            interim_results: true,
            enable_voice_activity_events: true,
            speech_start_timeout: None,
            speech_end_timeout: None,
            single_utterance: false,
            profanity_filter: false,
            enable_word_time_offsets: false,
            enable_word_confidence: false,
            enable_spoken_punctuation: false,
            enable_spoken_emojis: false,
            multichannel: false,
            max_alternatives: 0,
            diarization: false,
            diarization_min_speakers: 0,
            diarization_max_speakers: 0,
            adaptation_phrases: Vec::new(),
            transcript_normalization: Vec::new(),
            endpoint_override: None,
            static_access_token: None,
        }
    }
}

impl GoogleSTTConfig {
    /// Build from the standardized config (W1 keystone). Google's v2 streaming config models only
    /// a small advanced surface, so this maps the two standardized features it can express:
    /// interim results (`interim_results`) and explicit voice-activity events (`vad_events` ->
    /// `enable_voice_activity_events`). Google's constructor needs a non-standard `project_id`,
    /// which is read from the `provider_extras` passthrough; when that is absent it is left empty
    /// here and `GoogleSTT::new_standard` fills it from the service-account credential. The
    /// recognizer `location` is read from `extras["location"]` (default `global`); a malformed
    /// value is ignored here and rejected by `GoogleSTT::new_standard` (via
    /// `location_from_extras`). Features Google cannot express here
    /// (diarization, smart_format, profanity_filter, word_timestamps, redaction, keyterms,
    /// language/entity detection) are capability gaps and stay at default.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let ex = &std.extras.0;
        let project_id = ex
            .get("project_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let mut cfg =
            crate::core::stt::google::GoogleSTT::create_google_config(std.base.clone(), project_id);
        // Recognizer location (e.g. `us`/`eu`, which chirp_3 requires). It selects both the
        // `locations/{location}` segment of the recognizer path and the regional endpoint.
        if let Ok(Some(location)) = Self::location_from_extras(ex) {
            cfg.location = location;
        }
        // Endpoint + static-token overrides (mirrors GoogleTTSConfig::from_standard): the endpoint
        // override rides the open `endpoint_override` passthrough; the static access token comes
        // from `extras["access_token"]` (a pre-minted bearer that bypasses the OAuth network fetch).
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        cfg.static_access_token = std
            .extras
            .0
            .get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from);
        if let Some(i) = f.interim_results {
            cfg.interim_results = i;
        }
        if let Some(v) = f.vad_events {
            cfg.enable_voice_activity_events = v;
        }
        // --- Newly-wired typed advanced features (v2 RecognitionConfig.features) ---------------
        if let Some(p) = f.profanity_filter {
            cfg.profanity_filter = p;
        }
        if let Some(w) = f.word_timestamps {
            cfg.enable_word_time_offsets = w;
        }
        if let Some(n) = f.alternatives {
            cfg.max_alternatives = n as i32;
        }
        if let Some(m) = f.multichannel {
            cfg.multichannel = m;
        }
        if let Some(d) = f.diarization {
            cfg.diarization = d;
        }
        // keyterms -> inline phrase-set adaptation (phrase/keyterm boosting).
        if let Some(k) = &f.keyterms {
            cfg.adaptation_phrases = k.clone();
        }
        // --- Provider-extras passthrough (no shared SttFeatures field) -------------------------
        if let Some(b) = ex
            .get("enable_spoken_punctuation")
            .and_then(|v| v.as_bool())
        {
            cfg.enable_spoken_punctuation = b;
        }
        if let Some(b) = ex.get("enable_spoken_emojis").and_then(|v| v.as_bool()) {
            cfg.enable_spoken_emojis = b;
        }
        if let Some(b) = ex.get("enable_word_confidence").and_then(|v| v.as_bool()) {
            cfg.enable_word_confidence = b;
        }
        // transcript_normalization: array of {search, replace, case_sensitive}.
        if let Some(arr) = ex
            .get("transcript_normalization")
            .and_then(|v| v.as_array())
        {
            cfg.transcript_normalization = arr
                .iter()
                .filter_map(|e| serde_json::from_value::<TranscriptNormEntry>(e.clone()).ok())
                .collect();
        }
        cfg
    }

    /// Reads `extras["location"]`. Absent, `null` or `""` → `Ok(None)` (keep the `global`
    /// default); a string matching `^[a-z0-9-]{1,40}$` → `Ok(Some(location))`; anything else
    /// (wrong case, a dot or slash, too long, a non-string) → `Err`.
    ///
    /// Docs: https://docs.cloud.google.com/speech-to-text/docs/reference/rest/v2/projects.locations.recognizers/recognize
    pub(crate) fn location_from_extras(
        extras: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<String>, String> {
        match extras.get("location") {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(s)) if s.is_empty() => Ok(None),
            Some(serde_json::Value::String(s)) if is_valid_google_stt_location(s) => {
                Ok(Some(s.clone()))
            }
            Some(_) => Err(INVALID_LOCATION_MESSAGE.to_string()),
        }
    }

    pub(crate) fn validate_endpoint_override(&self) -> Result<(), String> {
        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_google_stt_endpoint("endpoint_override", endpoint)?;
        }
        Ok(())
    }

    /// Rejects a `location` that is not `^[a-z0-9-]{1,40}$` (it is interpolated into a hostname).
    pub(crate) fn validate_location(&self) -> Result<(), String> {
        if is_valid_google_stt_location(&self.location) {
            Ok(())
        } else {
            Err(INVALID_LOCATION_MESSAGE.to_string())
        }
    }

    /// Every pre-connect check on the resolved config: endpoint override (SSRF) and location.
    pub(crate) fn validate_runtime_config(&self) -> Result<(), String> {
        self.validate_endpoint_override()?;
        self.validate_location()
    }

    pub fn recognizer_path(&self) -> String {
        let recognizer = self.recognizer_id.as_deref().unwrap_or("_");
        format!(
            "projects/{}/locations/{}/recognizers/{}",
            self.project_id, self.location, recognizer
        )
    }

    /// Maps the base config encoding to Google's encoding name.
    pub fn google_encoding(&self) -> &'static str {
        match self.base.encoding.to_lowercase().as_str() {
            "linear16" | "pcm" => "LINEAR16",
            "flac" => "FLAC",
            "mulaw" | "ulaw" => "MULAW",
            "amr" => "AMR",
            "amr_wb" | "amr-wb" => "AMR_WB",
            "ogg_opus" | "ogg-opus" | "opus" => "OGG_OPUS",
            "webm_opus" | "webm-opus" => "WEBM_OPUS",
            _ => "LINEAR16",
        }
    }
}

/// Serde helper module for optional Duration serialization.
mod optional_duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(value: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(duration) => {
                let millis = duration.as_millis() as u64;
                millis.serialize(serializer)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<u64> = Option::deserialize(deserializer)?;
        Ok(opt.map(Duration::from_millis))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features unlock Google's interim-results and voice-activity
    // event flags, and the non-standard `project_id` is read from the provider_extras passthrough.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("project_id".into(), serde_json::json!("proj-123"));
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "google".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                vad_events: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
            translation: None,
        };
        let cfg = GoogleSTTConfig::from_standard(&std);
        assert!(!cfg.interim_results);
        assert!(!cfg.enable_voice_activity_events);
        assert_eq!(cfg.project_id, "proj-123"); // from provider_extras passthrough
        assert_eq!(cfg.location, "global"); // no extras.location → default
    }

    #[test]
    fn location_from_extras_accepts_only_google_location_ids() {
        let read = |v: serde_json::Value| {
            let mut extras = serde_json::Map::new();
            extras.insert("location".into(), v);
            GoogleSTTConfig::location_from_extras(&extras)
        };
        assert_eq!(
            GoogleSTTConfig::location_from_extras(&serde_json::Map::new()),
            Ok(None)
        );
        assert_eq!(read(serde_json::Value::Null), Ok(None));
        assert_eq!(read(serde_json::json!("")), Ok(None));
        for ok in ["global", "us", "eu", "us-central1", "asia-northeast1"] {
            assert_eq!(read(serde_json::json!(ok)), Ok(Some(ok.to_string())));
        }
        assert_eq!(
            read(serde_json::json!("a".repeat(40))),
            Ok(Some("a".repeat(40)))
        );
        for bad in [
            serde_json::json!("US"),
            serde_json::json!("us_central1"),
            serde_json::json!("us.evil.com"),
            serde_json::json!("us/x"),
            serde_json::json!("us:443"),
            serde_json::json!(" us"),
            serde_json::json!("a".repeat(41)),
            serde_json::json!(1),
            serde_json::json!(false),
            serde_json::json!({"region": "us"}),
        ] {
            match read(bad.clone()) {
                Err(err) => assert!(err.contains("^[a-z0-9-]{1,40}$"), "{err}"),
                Ok(v) => panic!("{bad} must be rejected, got {v:?}"),
            }
        }
    }

    #[test]
    fn from_standard_maps_location_and_validate_location_rejects_bad_values() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let mk = |location: serde_json::Value| {
            let mut extras = serde_json::Map::new();
            extras.insert("location".into(), location);
            StandardSTTConfig {
                extras: ProviderExtras(extras),
                ..StandardSTTConfig::from_base(STTConfig {
                    provider: "google".into(),
                    ..Default::default()
                })
            }
        };
        let cfg = GoogleSTTConfig::from_standard(&mk(serde_json::json!("us")));
        assert_eq!(cfg.location, "us");
        assert_eq!(
            cfg.recognizer_path(),
            "projects//locations/us/recognizers/_" // project is resolved by GoogleSTT::new_standard
        );
        assert!(cfg.validate_runtime_config().is_ok());

        // A malformed value is ignored by the infallible mapper (new_standard rejects it).
        let cfg = GoogleSTTConfig::from_standard(&mk(serde_json::json!("US")));
        assert_eq!(cfg.location, "global");

        let mut cfg = GoogleSTTConfig::default();
        assert!(cfg.validate_location().is_ok());
        for bad in ["", "US", "us.evil.com", "us/x"] {
            cfg.location = bad.to_string();
            assert!(cfg.validate_location().is_err(), "{bad:?}");
            assert!(cfg.validate_runtime_config().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let mut config = GoogleSTTConfig {
            endpoint_override: Some("https://google-stt-proxy.example.com".to_string()),
            ..Default::default()
        };
        assert!(config.validate_endpoint_override().is_ok());

        config.endpoint_override = Some("http://google-stt-proxy.example.com".to_string());
        assert!(config.validate_endpoint_override().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("file endpoint_override must be rejected");
        assert!(err.contains("not allowed"), "{err}");

        config.endpoint_override = Some("ws://google-stt-proxy.example.com".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("WebSocket endpoint_override must be rejected for Google STT gRPC");
        assert!(err.contains("not allowed"), "{err}");

        config.endpoint_override = Some("   ".to_string());
        assert!(config.validate_endpoint_override().is_ok());

        // SAFETY: restore the process env before releasing the test env lock.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
            } else {
                std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
            }
        }
    }
}
