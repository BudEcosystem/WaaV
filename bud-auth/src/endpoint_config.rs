//! The `config` block on a `voice_table` entry — a deployment's feature configuration.
//!
//! budapp's operator-facing settings form writes this; `PUT /endpoints/{id}/audio-config` is the
//! surface, `endpoint_ops/audio_config.py` is the vocabulary, and this is the consuming half of
//! the same wire contract. It carries what an operator chose at deploy time: a default voice and
//! language, synthesis settings, transcription features.
//!
//! Three properties are deliberate and each exists because of a specific failure:
//!
//! * **Unknown fields are accepted and logged, never fatal.** budgateway's provider config is
//!   `deny_unknown_fields` and dropped mismatched entries with no error at all — a documented
//!   silent-failure source. Tolerating the widening is also what lets budapp and WaaV ship in
//!   either order: budapp may publish a key this build does not model, and the deployment keeps
//!   serving.
//! * **Every field is `Option`.** Absent means "the vendor decides", which is not the same as any
//!   particular value. A `bool` defaulting to `false` would turn "not configured" into "explicitly
//!   off" and send a knob to the vendor the operator never touched.
//! * **Nothing here is validated against a vendor.** budapp validates shape; this crate parses;
//!   the gateway degrades what its provider cannot map, with a warning. A config typo must not
//!   become a serve-time outage.
//!
//! The types are plain data on purpose. `bud-auth` is a dependency *of* the gateway, so it cannot
//! reach the gateway's `EmotionConfig` / `SttFeatures` / `TtsFeatures`; the mapping onto those
//! lives in the gateway, and this is the one declaration of the wire shape on the Rust side.

use serde::Deserialize;

/// One spoken-form replacement, applied to the text before synthesis.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pronunciation {
    pub word: String,
    pub pronunciation: String,
}

/// Describe the voice rather than naming one; resolved per vendor against its catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct VoiceDescriptor {
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    /// Used only when no `locale` is given — a locale is the stronger statement of the two.
    #[serde(default)]
    pub accent: Option<String>,
    #[serde(default)]
    pub age: Option<String>,
    /// Timbre/style words matched against the vendor's own voice metadata.
    #[serde(default)]
    pub name_hint: Option<String>,
}

impl VoiceDescriptor {
    /// Whether anything at all was described. An all-`None` descriptor must not trigger a
    /// catalogue lookup: it would spend vendor I/O to decide nothing.
    pub fn is_empty(&self) -> bool {
        self.gender.is_none()
            && self.locale.is_none()
            && self.accent.is_none()
            && self.age.is_none()
            && self.name_hint.is_none()
    }
}

/// Synthesis defaults for a deployment.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct TtsSettings {
    // --- flat TTSConfig fields: reachable without the Standard plumbing ---
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub pronunciations: Option<Vec<Pronunciation>>,
    #[serde(default)]
    pub connection_timeout: Option<u64>,
    #[serde(default)]
    pub request_timeout: Option<u64>,

    // --- emotion: also on the flat config, which is why it is the cheapest to ship ---
    /// One of the 44 canonical tokens. Parsed by the gateway into its `Emotion` enum; a token
    /// this build does not know warns and synthesises normally rather than failing.
    #[serde(default)]
    pub emotion: Option<String>,
    /// A float 0.0-1.0 or one of `low` / `medium` / `high`. Untyped here because the two forms
    /// share a field on the wire and the gateway owns the mapping to `EmotionIntensity`.
    #[serde(default)]
    pub emotion_intensity: Option<serde_json::Value>,
    #[serde(default)]
    pub delivery_style: Option<String>,

    #[serde(default)]
    pub voice_descriptor: Option<VoiceDescriptor>,

    // --- TtsFeatures: needs the Standard config on the REST path (C3) ---
    #[serde(default)]
    pub pitch: Option<f32>,
    #[serde(default)]
    pub volume: Option<f32>,
    #[serde(default)]
    pub rate_percentage: Option<i32>,
    #[serde(default)]
    pub pitch_percentage: Option<i32>,
    #[serde(default)]
    pub stability: Option<f32>,
    #[serde(default)]
    pub similarity_boost: Option<f32>,
    /// ElevenLabs style *exaggeration*, a float — NOT `delivery_style`, which is a token.
    #[serde(default)]
    pub style: Option<f32>,
    #[serde(default)]
    pub use_speaker_boost: Option<bool>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub streaming: Option<bool>,
    #[serde(default)]
    pub optimize_streaming_latency: Option<u8>,
}

/// Transcription defaults for a deployment.
///
/// The five streaming-only canonical features are absent by construction: they need a continuous
/// stream, budapp refuses them at publish naming the transport, and a field here would suggest
/// otherwise to the next person reading this struct.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SttSettings {
    // --- the four the handler used to hardcode, plus model and prompt ---
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub punctuation: Option<bool>,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub channels: Option<u16>,
    /// Biasing context for the decoder. Several vendors take one.
    #[serde(default)]
    pub prompt: Option<String>,

    // --- the 13 batch-relevant SttFeatures ---
    #[serde(default)]
    pub diarization: Option<bool>,
    #[serde(default)]
    pub word_timestamps: Option<bool>,
    #[serde(default)]
    pub smart_format: Option<bool>,
    #[serde(default)]
    pub profanity_filter: Option<bool>,
    #[serde(default)]
    pub filler_words: Option<bool>,
    #[serde(default)]
    pub keyterms: Option<Vec<String>>,
    /// Canonical PII/PHI category names. An OPEN set — vendors interpret their own.
    #[serde(default)]
    pub redaction: Option<Vec<String>>,
    #[serde(default)]
    pub language_detection: Option<bool>,
    #[serde(default)]
    pub entity_detection: Option<bool>,
    #[serde(default)]
    pub numerals: Option<bool>,
    #[serde(default)]
    pub multichannel: Option<bool>,
    #[serde(default)]
    pub alternatives: Option<u8>,
    #[serde(default)]
    pub sentiment: Option<bool>,

    /// WaaV's own DSP, ahead of any provider: decode → denoise → transcribe. Costs CPU per
    /// request and changes the audio the vendor bills against, so it defaults off.
    #[serde(default)]
    pub noise_suppression: Option<bool>,
}

/// Translation defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TranslationSettings {
    /// Canonical BCP-47 targets. Class-A vendors take arbitrary targets through a side channel;
    /// Class-B vendors only translate to English and warn about the rest.
    #[serde(default)]
    pub target_languages: Option<Vec<String>>,
    #[serde(default)]
    pub translate_to_english: Option<bool>,
    #[serde(default)]
    pub partials: Option<bool>,
}

/// The whole `config` object on a voice entry.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct VoiceEndpointSettings {
    #[serde(default)]
    pub tts: Option<TtsSettings>,
    #[serde(default)]
    pub stt: Option<SttSettings>,
    #[serde(default)]
    pub translation: Option<TranslationSettings>,
}

impl VoiceEndpointSettings {
    pub fn is_empty(&self) -> bool {
        self.tts.is_none() && self.stt.is_none() && self.translation.is_none()
    }

    /// The tts block, or an all-`None` one. Saves every call site an `unwrap_or_default` clone.
    pub fn tts(&self) -> std::borrow::Cow<'_, TtsSettings> {
        match &self.tts {
            Some(t) => std::borrow::Cow::Borrowed(t),
            None => std::borrow::Cow::Owned(TtsSettings::default()),
        }
    }

    pub fn stt(&self) -> std::borrow::Cow<'_, SttSettings> {
        match &self.stt {
            Some(s) => std::borrow::Cow::Borrowed(s),
            None => std::borrow::Cow::Owned(SttSettings::default()),
        }
    }
}

/// Every key each section models, so a field budapp added and this build does not is *visible*.
///
/// Kept as literal lists rather than derived from the structs because serde offers no reflection
/// over field names, and the alternative — a `deny_unknown_fields` that fails the parse — is the
/// budgateway behaviour this whole module exists not to inherit.
const KNOWN_TTS: &[&str] = &[
    "sample_rate",
    "pronunciations",
    "connection_timeout",
    "request_timeout",
    "emotion",
    "emotion_intensity",
    "delivery_style",
    "voice_descriptor",
    "pitch",
    "volume",
    "rate_percentage",
    "pitch_percentage",
    "stability",
    "similarity_boost",
    "style",
    "use_speaker_boost",
    "language",
    "streaming",
    "optimize_streaming_latency",
];

const KNOWN_STT: &[&str] = &[
    "model",
    "language",
    "punctuation",
    "encoding",
    "channels",
    "prompt",
    "diarization",
    "word_timestamps",
    "smart_format",
    "profanity_filter",
    "filler_words",
    "keyterms",
    "redaction",
    "language_detection",
    "entity_detection",
    "numerals",
    "multichannel",
    "alternatives",
    "sentiment",
    "noise_suppression",
];

const KNOWN_TRANSLATION: &[&str] = &["target_languages", "translate_to_english", "partials"];

const KNOWN_SECTIONS: &[&str] = &["tts", "stt", "translation"];

/// Parse a `config` object, warning about anything this build does not model.
///
/// Never fails: a `config` that cannot be parsed at all leaves the endpoint serving with vendor
/// defaults, which is strictly better than an endpoint that does not resolve. The distinction
/// matters because the alternative is a whole deployment disappearing over a typo in one knob.
pub fn parse_endpoint_settings(
    endpoint_id: &str,
    raw: &serde_json::Value,
) -> VoiceEndpointSettings {
    if let serde_json::Value::Object(sections) = raw {
        warn_unmodelled(endpoint_id, "config", sections.keys(), KNOWN_SECTIONS);
        for (section, known) in [
            ("tts", KNOWN_TTS),
            ("stt", KNOWN_STT),
            ("translation", KNOWN_TRANSLATION),
        ] {
            if let Some(serde_json::Value::Object(fields)) = sections.get(section) {
                warn_unmodelled(
                    endpoint_id,
                    &format!("config.{section}"),
                    fields.keys(),
                    known,
                );
            }
        }
    }

    match serde_json::from_value::<VoiceEndpointSettings>(raw.clone()) {
        Ok(settings) => settings,
        Err(e) => {
            tracing::warn!(
                endpoint_id = %endpoint_id,
                error = %e,
                "voice_table config block could not be parsed; serving this endpoint with vendor \
                 defaults. The endpoint stays usable deliberately -- a typo in one knob must not \
                 take a whole deployment out of service"
            );
            VoiceEndpointSettings::default()
        }
    }
}

fn warn_unmodelled<'a>(
    endpoint_id: &str,
    path: &str,
    keys: impl Iterator<Item = &'a String>,
    known: &[&str],
) {
    for key in keys {
        if !known.contains(&key.as_str()) {
            tracing::warn!(
                endpoint_id = %endpoint_id,
                field = %format!("{path}.{key}"),
                "voice_table config carries a field this build does not model; accepting it, \
                 but budapp and WaaV may have drifted"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> VoiceEndpointSettings {
        parse_endpoint_settings("ep-1", &serde_json::from_str(json).unwrap())
    }

    #[test]
    fn every_section_round_trips() {
        let settings = parse(
            r#"{
              "tts": {"emotion": "calm", "emotion_intensity": 0.6, "sample_rate": 24000},
              "stt": {"diarization": true, "redaction": ["pii"], "smart_format": true},
              "translation": {"target_languages": ["es-ES", "de-DE"]}
            }"#,
        );

        let tts = settings.tts.as_ref().unwrap();
        assert_eq!(tts.emotion.as_deref(), Some("calm"));
        assert_eq!(tts.sample_rate, Some(24000));

        let stt = settings.stt.as_ref().unwrap();
        assert_eq!(stt.diarization, Some(true));
        assert_eq!(stt.redaction.as_deref(), Some(&["pii".to_string()][..]));

        assert_eq!(
            settings.translation.unwrap().target_languages.unwrap(),
            vec!["es-ES".to_string(), "de-DE".to_string()]
        );
    }

    #[test]
    fn an_unknown_field_is_kept_not_fatal() {
        // TC-CFG-06. This is the property that lets budapp and WaaV ship in either order.
        let settings = parse(r#"{"tts": {"emotion": "calm", "warp_factor": 9}}"#);

        assert_eq!(settings.tts.unwrap().emotion.as_deref(), Some("calm"));
    }

    #[test]
    fn an_unknown_section_does_not_lose_the_known_ones() {
        let settings = parse(r#"{"tts": {"emotion": "calm"}, "holodeck": {"on": true}}"#);

        assert_eq!(settings.tts.unwrap().emotion.as_deref(), Some("calm"));
    }

    #[test]
    fn a_wrongly_typed_field_degrades_to_defaults_rather_than_dropping_the_endpoint() {
        // `sample_rate` as a string fails the whole deserialize. The endpoint must still serve.
        let settings = parse(r#"{"tts": {"sample_rate": "twenty four thousand"}}"#);

        assert!(settings.is_empty());
    }

    #[test]
    fn absent_is_not_false() {
        // A `bool` (rather than `Option<bool>`) would make "not configured" indistinguishable
        // from "explicitly off", and send a knob to the vendor nobody chose.
        let settings = parse(r#"{"stt": {"diarization": true}}"#);
        let stt = settings.stt.unwrap();

        assert_eq!(stt.diarization, Some(true));
        assert_eq!(stt.punctuation, None, "untouched must stay untouched");
        assert_eq!(stt.noise_suppression, None);
    }

    #[test]
    fn a_named_intensity_and_a_float_both_survive() {
        assert_eq!(
            parse(r#"{"tts": {"emotion_intensity": "high"}}"#)
                .tts
                .unwrap()
                .emotion_intensity,
            Some(serde_json::json!("high"))
        );
        assert_eq!(
            parse(r#"{"tts": {"emotion_intensity": 0.42}}"#)
                .tts
                .unwrap()
                .emotion_intensity,
            Some(serde_json::json!(0.42))
        );
    }

    #[test]
    fn an_empty_descriptor_is_reported_as_empty() {
        let descriptor = VoiceDescriptor::default();
        assert!(descriptor.is_empty());

        let described = VoiceDescriptor {
            gender: Some("female".into()),
            ..Default::default()
        };
        assert!(!described.is_empty());
    }

    #[test]
    fn pronunciations_survive_in_order() {
        let settings = parse(
            r#"{"tts": {"pronunciations": [
                 {"word": "API", "pronunciation": "A P I"},
                 {"word": "WaaV", "pronunciation": "wave"}
               ]}}"#,
        );

        let p = settings.tts.unwrap().pronunciations.unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].word, "API");
        assert_eq!(p[1].pronunciation, "wave");
    }
}
