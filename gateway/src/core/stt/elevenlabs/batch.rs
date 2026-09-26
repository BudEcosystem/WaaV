//! ElevenLabs BATCH (prerecorded) speech-to-text — `POST /v1/speech-to-text`.
//!
//! ElevenLabs ships its speech-to-text over two different transports, and until now WaaV spoke
//! only one of them:
//!
//! | model id             | transport | wire                                    |
//! |----------------------|-----------|-----------------------------------------|
//! | `scribe_v2`          | batch     | `POST https://api.elevenlabs.io/v1/speech-to-text` |
//! | `scribe_v2_medical`  | batch     | same                                    |
//! | `scribe_v2_realtime` | realtime  | `wss://…/v1/speech-to-text/realtime` ([`super::client`]) |
//!
//! A file uploaded to `/v1/audio/transcriptions` is a prerecorded request in every sense that
//! matters, so pointing it at the realtime socket was never right — it worked only because
//! `scribe_v2_realtime` happens to accept a replayed file. Any operator who read ElevenLabs'
//! own docs, picked `scribe_v2` (the model those docs put on this exact endpoint), and uploaded
//! a file got a 502 whose text named a WebSocket policy close. This module is the missing half.
//!
//! # Shape
//!
//! This module is the WIRE SURFACE only — the config and the exact multipart fields. The
//! buffering, the HTTP and the response parsing live in [`crate::core::stt::prerecorded`], the
//! shared driver every prerecorded vendor uses, reached through
//! [`crate::core::stt::batch::build_elevenlabs_transcription`].
//!
//! [`ElevenLabsBatchConfig::multipart_fields`] is a pure function, so a test asserts what is sent
//! without a credential. That matters more here than usual: this vendor rejects an unknown field
//! with a 422 whose body names only the field, so a knob that silently never reached the request
//! would look exactly like a knob the vendor ignores.
//!
//! # What batch reaches that realtime cannot
//!
//! The batch API is the larger of the two surfaces. `multichannel`, `num_speakers` beyond the
//! realtime cap, audio-event tagging, `temperature`/`seed`, speaker-role detection and the
//! 1000-term keyterm list exist only here.

use super::super::base::STTConfig;
use super::config::ElevenLabsRegion;

// =============================================================================
// Model vocabulary
// =============================================================================

/// The model ElevenLabs' own documentation puts on `POST /v1/speech-to-text`.
///
/// Used when a deployment names no model at all. `scribe_v1` is deliberately absent: ElevenLabs
/// removed it on 2026-07-09, so defaulting to it would produce a 422 on every request.
pub const DEFAULT_BATCH_MODEL: &str = "scribe_v2";

/// Maximum keyterms on the BATCH endpoint (the realtime socket caps at 50; the realtime
/// client's `MAX_KEYTERMS` is deliberately not reused here).
pub const MAX_BATCH_KEYTERMS: usize = 1000;

// =============================================================================
// Configuration
// =============================================================================

/// Which of ElevenLabs' two transports serves a given model id.
///
/// The realtime list is the closed one — a model is realtime only if ElevenLabs publishes it as
/// such — and everything else is batch. That asymmetry is deliberate: ElevenLabs adds batch models
/// (`scribe_v2_medical` arrived after `scribe_v2`) far more often than realtime ones, and a closed
/// batch list would turn each new model into a WaaV release.
pub fn model_is_realtime(model: &str) -> bool {
    super::config::REALTIME_MODELS.contains(&model.trim())
}

/// The redaction mode ElevenLabs applies when `entity_redaction` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntityRedactionMode {
    /// Replace the span with `{REDACTED}`.
    #[default]
    Redacted,
    /// Replace the span with the entity's type.
    EntityType,
    /// Replace the span with the entity's type plus an occurrence number.
    EnumeratedEntityType,
}

impl EntityRedactionMode {
    /// The wire token.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Redacted => "redacted",
            Self::EntityType => "entity_type",
            Self::EnumeratedEntityType => "enumerated_entity_type",
        }
    }
}

/// Configuration for `POST /v1/speech-to-text`.
///
/// Deliberately a separate type from [`super::config::ElevenLabsSTTConfig`] rather than a superset
/// of it. The two transports share a vendor, not a configuration: half of the realtime struct
/// (commit strategy, VAD thresholds, audio format, minimum speech duration) has no meaning on a
/// file upload, and the realtime struct's `validate` rejects exactly the model ids this one
/// requires. Folding them together would make every field ambiguous about which transport it
/// applies to.
#[derive(Debug, Clone)]
pub struct ElevenLabsBatchConfig {
    /// Base STT configuration (api key, language, sample rate, channels, model).
    pub base: STTConfig,

    /// ElevenLabs model identifier (`scribe_v2`, `scribe_v2_medical`, …).
    pub model_id: String,

    /// Regional endpoint (latency or data residency).
    pub region: ElevenLabsRegion,

    /// Annotate which speaker is talking (`diarize`).
    pub diarize: Option<bool>,

    /// Maximum speakers to separate (`num_speakers`). `None` lets the model decide, up to 32.
    pub num_speakers: Option<u8>,

    /// Speaker-overlap sensitivity (`diarization_threshold`). Only valid with `diarize=true` and
    /// `num_speakers` unset — ElevenLabs 422s the combination, so the builder drops it rather
    /// than sending a request that cannot succeed.
    pub diarization_threshold: Option<f64>,

    /// Word- or character-level timings (`timestamps_granularity`).
    pub timestamps_granularity: Option<TimestampsGranularity>,

    /// Tag non-speech sounds — "(laughter)", "(footsteps)" (`tag_audio_events`).
    pub tag_audio_events: Option<bool>,

    /// Transcribe each channel independently (`use_multi_channel`).
    ///
    /// Deliberately NOT mapped from the canonical `SttFeatures::multichannel`, and cleared
    /// outright by [`ElevenLabsBatchSTT`]. WaaV's upload path decodes every file to mono
    /// (`waav_openai_audio::pcm::decode` downmixes by averaging), so on that transport asking a
    /// vendor to separate channels asks it to separate one channel — inert at best. Offering it
    /// as a deployment setting would put a control on the settings page that can never do
    /// anything, which is the defect the feature matrix exists to remove.
    ///
    /// It is real on `/transcribe/batch`, where the audio reaches ElevenLabs as the caller's own
    /// bytes or a URL and is never downmixed. That route builds this config directly.
    pub use_multi_channel: Option<bool>,

    /// Domain vocabulary to boost (`keyterms`). Up to [`MAX_BATCH_KEYTERMS`].
    pub keyterms: Option<Vec<String>>,

    /// Entity categories to DETECT (`entity_detection`): `all`, a type, or a category
    /// (`pii` / `phi` / `pci` / `other` / `offensive_language`).
    pub entity_detection: Option<Vec<String>>,

    /// Entity categories to REDACT (`entity_redaction`). Must be a subset of what is detected;
    /// the builder unions them so a caller who asked only to redact still gets a valid request.
    pub entity_redaction: Option<Vec<String>>,

    /// How a redacted span is rendered (`entity_redaction_mode`).
    pub entity_redaction_mode: Option<EntityRedactionMode>,

    /// Drop filler words (`no_verbatim`). The INVERSE of the canonical `filler_words`.
    pub no_verbatim: Option<bool>,

    /// Sampling randomness (`temperature`, 0.0–2.0).
    pub temperature: Option<f64>,

    /// Deterministic sampling (`seed`).
    pub seed: Option<i64>,

    /// Label speakers as "agent" / "customer" (`detect_speaker_roles`).
    pub detect_speaker_roles: Option<bool>,

    /// Match speakers against the workspace speaker library (`use_speaker_library`).
    pub use_speaker_library: Option<bool>,

    /// Whether ElevenLabs may retain the request (`enable_logging`, a QUERY parameter).
    pub enable_logging: bool,

    /// Test/diagnostic HTTP endpoint override (a localhost mock), replacing the region base URL.
    pub endpoint_override: Option<String>,
}

/// `timestamps_granularity` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampsGranularity {
    /// One timing per word.
    Word,
    /// One timing per character.
    Character,
}

impl TimestampsGranularity {
    /// The wire token.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Character => "character",
        }
    }
}

impl Default for ElevenLabsBatchConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            model_id: DEFAULT_BATCH_MODEL.to_string(),
            region: ElevenLabsRegion::default(),
            diarize: None,
            num_speakers: None,
            diarization_threshold: None,
            timestamps_granularity: None,
            tag_audio_events: None,
            use_multi_channel: None,
            keyterms: None,
            entity_detection: None,
            entity_redaction: None,
            entity_redaction_mode: None,
            no_verbatim: None,
            temperature: None,
            seed: None,
            detect_speaker_roles: None,
            use_speaker_library: None,
            enable_logging: true,
            endpoint_override: None,
        }
    }
}

impl ElevenLabsBatchConfig {
    /// Build from the flat base config.
    pub fn from_base(base: STTConfig) -> Self {
        let mut cfg = Self::default();
        if !base.model.trim().is_empty() {
            cfg.model_id = base.model.trim().to_string();
        }
        cfg.base = base;
        cfg
    }

    /// Build from the standardized config — the path the `/v1/audio/transcriptions` handler takes.
    ///
    /// Every canonical feature the realtime mapper honours is honoured here too, plus the ones
    /// only batch can express. Features neither transport has (`smart_format`,
    /// `profanity_filter`, `numerals`, `alternatives`, `sentiment`) stay at provider defaults, and
    /// the streaming-only ones (`interim_results`, `vad_events`, `endpointing_ms`,
    /// `utterance_end_ms`, `speech_begin_event`) are meaningless on an upload.
    pub fn from_standard(std: &crate::core::stt::standard::StandardSTTConfig) -> Self {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone());

        if let Some(d) = f.diarization {
            cfg.diarize = Some(d);
        }
        // Word timestamps are a granularity here, not a boolean. `false` is not "character" — it
        // is "do not ask", so the field is omitted and the vendor's default applies.
        if f.word_timestamps == Some(true) {
            cfg.timestamps_granularity = Some(TimestampsGranularity::Word);
        }
        if let Some(k) = &f.keyterms {
            cfg.keyterms = Some(k.clone());
        }
        if let Some(e) = f.entity_detection {
            // `all` rather than a category list: the canonical flag is a boolean, and narrowing it
            // to one category here would silently drop the other 64 types the caller asked for.
            cfg.entity_detection = e.then(|| vec!["all".to_string()]);
        }
        if let Some(r) = &f.redaction
            && !r.is_empty()
        {
            cfg.entity_redaction = Some(map_redaction_categories(r));
        }
        // Filler words (canonical) -> `no_verbatim` (INVERTED), matching the realtime mapper.
        if let Some(keep_fillers) = f.filler_words {
            cfg.no_verbatim = Some(!keep_fillers);
        }
        // `language_detection` is expressed by OMITTING `language_code`; handled in the builder,
        // which is the only place that can see both the flag and the language together.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());

        // Open passthrough for the batch-only knobs with no canonical name. Applied last so an
        // operator can override anything the typed mapping decided.
        cfg.apply_extras(&std.extras);
        cfg
    }

    /// Fold `extras` over the typed mapping.
    ///
    /// The batch API has a dozen knobs with no canonical equivalent (`temperature`, `seed`,
    /// `tag_audio_events`, `detect_speaker_roles`, `use_speaker_library`, `num_speakers`,
    /// `diarization_threshold`, `entity_redaction_mode`, `enable_logging`). Rather than inventing
    /// canonical names that would reach exactly one vendor, they are read from the passthrough an
    /// operator already has.
    fn apply_extras(&mut self, extras: &crate::core::stt::standard::ProviderExtras) {
        let get = |k: &str| extras.0.get(k);
        if let Some(v) = get("tag_audio_events").and_then(|v| v.as_bool()) {
            self.tag_audio_events = Some(v);
        }
        if let Some(v) = get("num_speakers").and_then(|v| v.as_u64()) {
            self.num_speakers = u8::try_from(v).ok();
        }
        if let Some(v) = get("diarization_threshold").and_then(|v| v.as_f64()) {
            self.diarization_threshold = Some(v);
        }
        if let Some(v) = get("temperature").and_then(|v| v.as_f64()) {
            self.temperature = Some(v);
        }
        if let Some(v) = get("seed").and_then(|v| v.as_i64()) {
            self.seed = Some(v);
        }
        if let Some(v) = get("detect_speaker_roles").and_then(|v| v.as_bool()) {
            self.detect_speaker_roles = Some(v);
        }
        if let Some(v) = get("use_speaker_library").and_then(|v| v.as_bool()) {
            self.use_speaker_library = Some(v);
        }
        if let Some(v) = get("enable_logging").and_then(|v| v.as_bool()) {
            self.enable_logging = v;
        }
        if let Some(v) = get("entity_redaction_mode").and_then(|v| v.as_str()) {
            self.entity_redaction_mode = match v {
                "entity_type" => Some(EntityRedactionMode::EntityType),
                "enumerated_entity_type" => Some(EntityRedactionMode::EnumeratedEntityType),
                _ => Some(EntityRedactionMode::Redacted),
            };
        }
        if let Some(v) = get("timestamps_granularity").and_then(|v| v.as_str()) {
            self.timestamps_granularity = match v {
                "character" => Some(TimestampsGranularity::Character),
                _ => Some(TimestampsGranularity::Word),
            };
        }
        if let Some(arr) = get("entity_detection").and_then(|v| v.as_array()) {
            self.entity_detection = Some(
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
            );
        }
    }

    /// Validate before a byte is sent.
    pub fn validate(&self) -> Result<(), String> {
        if self.base.api_key.is_empty() {
            return Err("API key is required".to_string());
        }
        // The mirror of the realtime client's check, and needed for the same reason: sent a
        // realtime-only id, the batch endpoint answers 422 with a body that names the field and
        // not the value, so the one fact needed to fix it is the one fact missing.
        if model_is_realtime(&self.model_id) {
            return Err(format!(
                "model '{}' is a realtime-only model for ElevenLabs speech-to-text. This request \
                 is a prerecorded upload, which ElevenLabs serves over `POST /v1/speech-to-text` \
                 with a batch model. Use '{}' (or another batch Scribe model, e.g. \
                 'scribe_v2_medical').",
                self.model_id, DEFAULT_BATCH_MODEL
            ));
        }
        if let Some(terms) = &self.keyterms
            && terms.len() > MAX_BATCH_KEYTERMS
        {
            return Err(format!(
                "Too many keyterms: {} provided, maximum is {MAX_BATCH_KEYTERMS}",
                terms.len()
            ));
        }
        if let Some(t) = self.temperature
            && !(0.0..=2.0).contains(&t)
        {
            return Err(format!("temperature must be between 0.0 and 2.0, got {t}"));
        }
        Ok(())
    }

    /// The URL to POST to, honouring the endpoint override and the `enable_logging` query flag.
    pub fn api_url(&self) -> String {
        let base = self
            .endpoint_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| region_http_base(self.region))
            .trim_end_matches('/');
        let url = format!("{base}/v1/speech-to-text");
        if self.enable_logging {
            url
        } else {
            format!("{url}?enable_logging=false")
        }
    }

    /// Build the multipart TEXT fields that accompany the audio file part.
    ///
    /// The whole wire surface in one pure function, for the reason the OpenAI provider has the
    /// same shape: a field that lives on the struct and never reaches the request is invisible
    /// in review, invisible in the logs, and looks from the outside exactly like a vendor that
    /// ignores the setting. A test asserts each one arrives.
    ///
    /// `language_detection` is expressed here rather than as a field, because it is the ABSENCE
    /// of `language_code` — the two cannot both be sent, and only this function sees both.
    pub fn multipart_fields(&self, detect_language: bool) -> Vec<(String, String)> {
        let mut fields: Vec<(String, String)> = vec![("model_id".into(), self.model_id.clone())];

        // ElevenLabs takes ISO-639-1/639-3, not BCP-47. The canonical mapper already downgrades
        // `en-US` to `en` for this vendor, but the flat path can hand over anything, and a
        // region-tagged value is a 422 rather than a degrade.
        //
        // Vendor contract: `language_code` is OPTIONAL and its absence means ElevenLabs detects
        // the language. So an unset language is omitted — never sent empty, never replaced.
        if !detect_language && !self.base.language.trim().is_empty() {
            let mapped = crate::core::lang::map_language(
                self.base.language.trim(),
                "elevenlabs",
                &self.model_id,
            );
            if !mapped.omit && !mapped.native.is_empty() {
                fields.push(("language_code".into(), mapped.native));
            }
        }

        if let Some(v) = self.diarize {
            fields.push(("diarize".into(), v.to_string()));
        }
        if let Some(v) = self.num_speakers {
            fields.push(("num_speakers".into(), v.to_string()));
        }
        // ElevenLabs accepts `diarization_threshold` only when it is choosing the speaker count
        // itself. Sending both is a 422, so the narrower setting wins and the other is dropped.
        if let Some(v) = self.diarization_threshold
            && self.diarize == Some(true)
            && self.num_speakers.is_none()
        {
            fields.push(("diarization_threshold".into(), v.to_string()));
        }
        if let Some(g) = self.timestamps_granularity {
            fields.push(("timestamps_granularity".into(), g.as_str().into()));
        }
        if let Some(v) = self.tag_audio_events {
            fields.push(("tag_audio_events".into(), v.to_string()));
        }
        if let Some(v) = self.use_multi_channel {
            fields.push(("use_multi_channel".into(), v.to_string()));
        }
        if let Some(terms) = &self.keyterms {
            for term in terms.iter().take(MAX_BATCH_KEYTERMS) {
                fields.push(("keyterms".into(), term.clone()));
            }
        }
        // Redaction implies detection: ElevenLabs requires `entity_redaction` to be a subset of
        // `entity_detection`, so asking only to redact would otherwise 422. Union them.
        let detection = match (&self.entity_detection, &self.entity_redaction) {
            (Some(d), Some(r)) => {
                let mut all = d.clone();
                for c in r {
                    if !all.iter().any(|x| x == c) {
                        all.push(c.clone());
                    }
                }
                Some(all)
            }
            (Some(d), None) => Some(d.clone()),
            (None, Some(r)) => Some(r.clone()),
            (None, None) => None,
        };
        if let Some(d) = detection {
            for c in d {
                fields.push(("entity_detection".into(), c));
            }
        }
        if let Some(r) = &self.entity_redaction {
            for c in r {
                fields.push(("entity_redaction".into(), c.clone()));
            }
            fields.push((
                "entity_redaction_mode".into(),
                self.entity_redaction_mode
                    .unwrap_or_default()
                    .as_str()
                    .into(),
            ));
        }
        if let Some(v) = self.no_verbatim {
            fields.push(("no_verbatim".into(), v.to_string()));
        }
        if let Some(v) = self.temperature {
            fields.push(("temperature".into(), v.to_string()));
        }
        if let Some(v) = self.seed {
            fields.push(("seed".into(), v.to_string()));
        }
        if let Some(v) = self.detect_speaker_roles {
            fields.push(("detect_speaker_roles".into(), v.to_string()));
        }
        if let Some(v) = self.use_speaker_library {
            fields.push(("use_speaker_library".into(), v.to_string()));
        }
        // `other`, always — and deliberately not `pcm_s16le_16`.
        //
        // This is a container, not a sample stream: the client wraps the decoded PCM in a 44-byte
        // RIFF header before uploading. ElevenLabs describes `pcm_s16le_16` as the lower-latency
        // alternative to "passing an encoded waveform", i.e. raw headerless bytes — so declaring
        // it for a WAV would hand them a header to interpret as audio.
        //
        // The fast path is real and worth taking one day: the raw PCM is already in hand at
        // exactly the geometry it asks for (16-bit LE mono) whenever the upload decoded to
        // 16 kHz. It is not taken here because the docs do not say in so many words that the
        // part must be headerless, and guessing wrong would corrupt the common case rather than
        // the rare one. Confirm against the live API before switching.
        fields.push(("file_format".into(), "other".into()));
        fields
    }
}

/// The HTTP host for a region. Separate from [`ElevenLabsRegion::host`] (a bare hostname for
/// headers) and from `websocket_base_url` (the `wss://` form).
pub fn region_http_base(region: ElevenLabsRegion) -> &'static str {
    match region {
        ElevenLabsRegion::Default => "https://api.elevenlabs.io",
        ElevenLabsRegion::Us => "https://api.us.elevenlabs.io",
        ElevenLabsRegion::Eu => "https://api.eu.residency.elevenlabs.io",
        ElevenLabsRegion::India => "https://api.in.residency.elevenlabs.io",
    }
}

/// Map canonical redaction categories onto ElevenLabs' entity categories.
///
/// The canonical vocabulary is `pii` / `phi` / free-form category names; ElevenLabs takes
/// `pii` / `phi` / `pci` / `other` / `offensive_language` or a specific entity type. Anything
/// unrecognized is passed through verbatim — the vendor is a better judge of its own 65 entity
/// types than a hardcoded list that would go stale.
fn map_redaction_categories(categories: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(categories.len());
    for c in categories {
        let lower = c.trim().to_lowercase();
        let mapped = if lower.contains("health") || lower == "phi" {
            "phi".to_string()
        } else if lower == "pii" || lower == "personal" {
            "pii".to_string()
        } else {
            lower
        };
        if !mapped.is_empty() && !out.contains(&mapped) {
            out.push(mapped);
        }
    }
    out
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stt::standard::{StandardSTTConfig, SttFeatures};

    fn base(model: &str) -> STTConfig {
        STTConfig {
            provider: "elevenlabs".to_string(),
            api_key: "xi-test".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16_000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: model.to_string(),
        }
    }

    /// `StandardSTTConfig` has no `Default` (its `base` carries required identity), so build the
    /// standardized fixture the way the handler does.
    fn standard(model: &str, features: SttFeatures) -> StandardSTTConfig {
        let mut cfg = StandardSTTConfig::from_base(base(model));
        cfg.features = features;
        cfg
    }

    fn field<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn all<'a>(fields: &'a [(String, String)], key: &str) -> Vec<&'a str> {
        fields
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    // -------------------------------------------------------------------------------------
    // The transport split
    // -------------------------------------------------------------------------------------

    #[test]
    fn the_realtime_list_is_closed_and_everything_else_is_batch() {
        // The asymmetry is the point: ElevenLabs ships batch models far more often than realtime
        // ones, so a closed batch list would turn every new Scribe model into a WaaV release.
        assert!(model_is_realtime("scribe_v2_realtime"));
        assert!(!model_is_realtime("scribe_v2"));
        assert!(!model_is_realtime("scribe_v2_medical"));
        assert!(!model_is_realtime("scribe_v3_whatever_they_ship_next"));
    }

    #[test]
    fn an_unset_model_defaults_to_the_documented_batch_model() {
        // Not `scribe_v1`: ElevenLabs removed it on 2026-07-09, so defaulting to it would 422
        // every request from a deployment that simply never named a model.
        let cfg = ElevenLabsBatchConfig::from_base(base(""));
        assert_eq!(cfg.model_id, "scribe_v2");
        assert_eq!(DEFAULT_BATCH_MODEL, "scribe_v2");
    }

    #[test]
    fn a_realtime_model_on_the_batch_transport_is_refused_by_name() {
        // The mirror of the realtime client's check. ElevenLabs' own 422 names the field and not
        // the value, so the one fact needed to fix it would be the one fact missing.
        let mut cfg = ElevenLabsBatchConfig::from_base(base("scribe_v2_realtime"));
        cfg.base.api_key = "xi-test".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("scribe_v2_realtime"), "{err}");
        assert!(err.contains("scribe_v2"), "names the alternative: {err}");
    }

    // -------------------------------------------------------------------------------------
    // The wire surface
    // -------------------------------------------------------------------------------------

    #[test]
    fn the_model_and_file_format_always_reach_the_wire() {
        let fields = ElevenLabsBatchConfig::from_base(base("scribe_v2")).multipart_fields(false);
        assert_eq!(field(&fields, "model_id"), Some("scribe_v2"));
        assert_eq!(field(&fields, "file_format"), Some("other"));
    }

    #[test]
    fn the_upload_is_never_declared_as_raw_pcm() {
        // The client uploads a WAV — a container with a 44-byte header. `pcm_s16le_16` is
        // ElevenLabs' headerless fast path, and declaring it here would hand them a RIFF header
        // to read as audio. True at every geometry, including the one that tempts it.
        let mut cfg = ElevenLabsBatchConfig::from_base(base("scribe_v2"));
        cfg.base.sample_rate = 16_000;
        cfg.base.channels = 1;
        assert_eq!(
            field(&cfg.multipart_fields(false), "file_format"),
            Some("other")
        );

        cfg.base.sample_rate = 48_000;
        assert_eq!(
            field(&cfg.multipart_fields(false), "file_format"),
            Some("other")
        );
    }

    #[test]
    fn the_language_is_downgraded_to_iso_639_1() {
        // ElevenLabs takes ISO-639-1/639-3. `en-US` is a 422, and the flat path can hand over
        // exactly that.
        let fields = ElevenLabsBatchConfig::from_base(base("scribe_v2")).multipart_fields(false);
        assert_eq!(field(&fields, "language_code"), Some("en"));
    }

    #[test]
    fn an_unset_language_is_omitted_rather_than_defaulted() {
        // The upload route hands over an empty language when neither the request nor the
        // deployment names one. `language_code` is optional — absent, ElevenLabs detects — so
        // nothing is sent, and certainly not an empty value or a substituted `en`.
        let mut b = base("scribe_v2");
        b.language = String::new();
        let fields = ElevenLabsBatchConfig::from_base(b).multipart_fields(false);
        assert_eq!(field(&fields, "language_code"), None);
    }

    #[test]
    fn automatic_detection_omits_the_language_entirely() {
        // `language_detection` is the ABSENCE of `language_code` — the two cannot both be sent.
        let fields = ElevenLabsBatchConfig::from_base(base("scribe_v2")).multipart_fields(true);
        assert_eq!(field(&fields, "language_code"), None);
    }

    #[test]
    fn every_canonical_feature_reaches_the_request() {
        // The bug class this shape exists to rule out: a field that lives on the struct, never
        // reaches the wire, and looks from the outside exactly like a vendor ignoring a setting.
        let std = standard(
            "scribe_v2",
            SttFeatures {
                diarization: Some(true),
                word_timestamps: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Deepgram".into()]),
                entity_detection: Some(true),
                redaction: Some(vec!["pii".into()]),
                filler_words: Some(false),
                ..Default::default()
            },
        );
        let fields = ElevenLabsBatchConfig::from_standard(&std).multipart_fields(false);

        assert_eq!(field(&fields, "diarize"), Some("true"));
        assert_eq!(field(&fields, "timestamps_granularity"), Some("word"));
        assert_eq!(all(&fields, "keyterms"), vec!["WaaV", "Deepgram"]);
        // filler_words=false means DROP them, which ElevenLabs spells as the inverse.
        assert_eq!(field(&fields, "no_verbatim"), Some("true"));
        assert!(all(&fields, "entity_detection").contains(&"all"));
        assert_eq!(all(&fields, "entity_redaction"), vec!["pii"]);
        assert_eq!(field(&fields, "entity_redaction_mode"), Some("redacted"));
    }

    #[test]
    fn word_timestamps_false_omits_the_granularity_rather_than_asking_for_characters() {
        // `false` is not "character" — it is "do not ask". Mapping a boolean onto a two-valued
        // enum is exactly where a silently wrong request comes from.
        let std = standard(
            "scribe_v2",
            SttFeatures {
                word_timestamps: Some(false),
                ..Default::default()
            },
        );
        let fields = ElevenLabsBatchConfig::from_standard(&std).multipart_fields(false);
        assert_eq!(field(&fields, "timestamps_granularity"), None);
    }

    #[test]
    fn asking_only_to_redact_still_produces_a_valid_request() {
        // ElevenLabs requires `entity_redaction` to be a SUBSET of `entity_detection`. A caller
        // who asked only to redact would otherwise get a 422 for a request WaaV built.
        let std = standard(
            "scribe_v2",
            SttFeatures {
                redaction: Some(vec!["phi".into()]),
                ..Default::default()
            },
        );
        let fields = ElevenLabsBatchConfig::from_standard(&std).multipart_fields(false);
        assert_eq!(all(&fields, "entity_detection"), vec!["phi"]);
        assert_eq!(all(&fields, "entity_redaction"), vec!["phi"]);
    }

    #[test]
    fn health_flavoured_redaction_categories_map_onto_phi() {
        assert_eq!(
            map_redaction_categories(&["health_information".into(), "PII".into()]),
            vec!["phi".to_string(), "pii".to_string()]
        );
        // Anything unrecognized rides through: ElevenLabs is a better judge of its own 65 entity
        // types than a hardcoded list that would go stale.
        assert_eq!(
            map_redaction_categories(&["credit_card_number".into()]),
            vec!["credit_card_number".to_string()]
        );
    }

    #[test]
    fn the_diarization_threshold_is_dropped_when_it_cannot_be_honoured() {
        // ElevenLabs accepts it only when IT is choosing the speaker count. Sending both is a
        // 422, so the narrower setting wins and the other is dropped rather than shipped.
        let mut cfg = ElevenLabsBatchConfig::from_base(base("scribe_v2"));
        cfg.diarize = Some(true);
        cfg.diarization_threshold = Some(0.2);
        assert_eq!(
            field(&cfg.multipart_fields(false), "diarization_threshold"),
            Some("0.2")
        );

        cfg.num_speakers = Some(3);
        assert_eq!(
            field(&cfg.multipart_fields(false), "diarization_threshold"),
            None
        );
        assert_eq!(
            field(&cfg.multipart_fields(false), "num_speakers"),
            Some("3")
        );
    }

    #[test]
    fn extras_reach_the_batch_only_knobs() {
        // A dozen batch knobs have no canonical name and reach exactly one vendor. Inventing
        // canonical names for them would put controls on every deployment's settings page that
        // 30 of 31 vendors ignore.
        let mut extras = serde_json::Map::new();
        extras.insert("tag_audio_events".into(), serde_json::json!(true));
        extras.insert("num_speakers".into(), serde_json::json!(4));
        extras.insert("temperature".into(), serde_json::json!(0.3));
        extras.insert("seed".into(), serde_json::json!(42));
        extras.insert("detect_speaker_roles".into(), serde_json::json!(true));
        extras.insert("use_speaker_library".into(), serde_json::json!(true));
        extras.insert(
            "entity_redaction_mode".into(),
            serde_json::json!("entity_type"),
        );

        let mut std = standard(
            "scribe_v2_medical",
            SttFeatures {
                redaction: Some(vec!["phi".into()]),
                ..Default::default()
            },
        );
        std.extras = crate::core::stt::standard::ProviderExtras(extras);
        let fields = ElevenLabsBatchConfig::from_standard(&std).multipart_fields(false);

        assert_eq!(field(&fields, "model_id"), Some("scribe_v2_medical"));
        assert_eq!(field(&fields, "tag_audio_events"), Some("true"));
        assert_eq!(field(&fields, "num_speakers"), Some("4"));
        assert_eq!(field(&fields, "temperature"), Some("0.3"));
        assert_eq!(field(&fields, "seed"), Some("42"));
        assert_eq!(field(&fields, "detect_speaker_roles"), Some("true"));
        assert_eq!(field(&fields, "use_speaker_library"), Some("true"));
        assert_eq!(field(&fields, "entity_redaction_mode"), Some("entity_type"));
    }

    #[test]
    fn an_out_of_range_temperature_is_refused_before_the_round_trip() {
        let mut cfg = ElevenLabsBatchConfig::from_base(base("scribe_v2"));
        cfg.temperature = Some(3.0);
        assert!(cfg.validate().unwrap_err().contains("temperature"));
    }

    // -------------------------------------------------------------------------------------
    // The URL
    // -------------------------------------------------------------------------------------

    #[test]
    fn the_url_is_the_batch_endpoint_and_honours_the_region() {
        let mut cfg = ElevenLabsBatchConfig::from_base(base("scribe_v2"));
        assert_eq!(cfg.api_url(), "https://api.elevenlabs.io/v1/speech-to-text");

        cfg.region = ElevenLabsRegion::Eu;
        assert_eq!(
            cfg.api_url(),
            "https://api.eu.residency.elevenlabs.io/v1/speech-to-text"
        );
    }

    #[test]
    fn zero_retention_rides_the_query_string_not_the_form() {
        let mut cfg = ElevenLabsBatchConfig::from_base(base("scribe_v2"));
        cfg.enable_logging = false;
        assert!(
            cfg.api_url()
                .ends_with("/v1/speech-to-text?enable_logging=false")
        );
        // and never as a form field, where ElevenLabs would reject it
        assert_eq!(field(&cfg.multipart_fields(false), "enable_logging"), None);
    }

    #[test]
    fn an_endpoint_override_redirects_the_post() {
        let mut cfg = ElevenLabsBatchConfig::from_base(base("scribe_v2"));
        cfg.endpoint_override = Some("http://127.0.0.1:9999/".into());
        assert_eq!(cfg.api_url(), "http://127.0.0.1:9999/v1/speech-to-text");
    }
}
