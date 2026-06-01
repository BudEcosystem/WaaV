use std::time::Duration;

use crate::core::stt::base::STTConfig;

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
}

impl Default for GoogleSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig {
                model: "latest_long".to_string(),
                ..STTConfig::default()
            },
            project_id: String::new(),
            location: "global".to_string(),
            recognizer_id: None,
            interim_results: true,
            enable_voice_activity_events: true,
            speech_start_timeout: None,
            speech_end_timeout: None,
            single_utterance: false,
        }
    }
}

impl GoogleSTTConfig {
    /// Build from the standardized config (W1 keystone). Google's v2 streaming config models only
    /// a small advanced surface, so this maps the two standardized features it can express:
    /// interim results (`interim_results`) and explicit voice-activity events (`vad_events` ->
    /// `enable_voice_activity_events`). Google's constructor needs a non-standard `project_id`,
    /// which is read from the `provider_extras` passthrough. Features Google cannot express here
    /// (diarization, smart_format, profanity_filter, word_timestamps, redaction, keyterms,
    /// language/entity detection) are capability gaps and stay at default.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let project_id = std
            .extras
            .0
            .get("project_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let mut cfg =
            crate::core::stt::google::GoogleSTT::create_google_config(std.base.clone(), project_id);
        if let Some(i) = f.interim_results {
            cfg.interim_results = i;
        }
        if let Some(v) = f.vad_events {
            cfg.enable_voice_activity_events = v;
        }
        cfg
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
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
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
        };
        let cfg = GoogleSTTConfig::from_standard(&std);
        assert!(!cfg.interim_results);
        assert!(!cfg.enable_voice_activity_events);
        assert_eq!(cfg.project_id, "proj-123"); // from provider_extras passthrough
    }
}
